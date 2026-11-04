// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lab-only same-widget policy control. Shares the Mix listing worker; this
//! isolates controller/runtime costs, not Rust-versus-Mix filesystem engines.
use bus::{SupervisedClient, native_client::BoundedIncomingEvent};
use serde_json::{Value, json};
use std::{io::{BufRead, BufReader, Write}, path::PathBuf, process::{Command,Stdio}, sync::mpsc};

const NAME:&str="dopus-rust-policy-lab";
type Result<T>=std::result::Result<T,Box<dyn std::error::Error+Send+Sync>>;

struct Pane {
    path:String,key:String,ascending:bool,generation:u64,revision:u64,
    loading:bool,total:usize,source:Value,error:Option<String>,
    selection:Value,selected:Vec<String>,focus:Value,focus_id:Value,ack:u64,epoch:u64,
}
impl Pane {
    fn new(path:String)->Self { Self {path,key:"name".into(),ascending:true,generation:1,revision:0,
        loading:false,total:0,source:Value::Null,error:None,selection:json!({"ranges":[]}),selected:vec![],
        focus:Value::Null,focus_id:Value::Null,ack:0,epoch:0} }
    fn count(&self)->usize { self.selection["ranges"].as_array().map_or(0,|ranges|ranges.iter()
        .map(|r|r[1].as_u64().unwrap_or(0).saturating_sub(r[0].as_u64().unwrap_or(0)) as usize).sum()) }
}
struct Policy {
    root:PathBuf,session:String,host:String,panes:[Pane;2],active:usize,status:String,
    pending:[Option<Value>;2],busy:Option<Value>,turn:usize,worker_bytes:u64,serial:u64,last_ui:Value,
    jobs:mpsc::SyncSender<Value>,
}
impl Policy {
    fn heading(&self,i:usize)->String {
        let p=&self.panes[i];
        if p.loading {"Loading…".into()} else if let Some(e)=&p.error {format!("Error: {e}")}
        else {format!("{} entries · {} selected",p.total,p.count())}
    }
    fn view(&self,i:usize)->Value {
        let p=&self.panes[i];let mut v=json!({"session":self.session,"generation":p.generation,
            "revision":p.revision,"ack":p.ack,"selection_epoch":p.epoch,
            "total":if p.loading {0}else{p.total},"offset":0,"rows":[],
            "focus":p.focus,"selection":if p.loading {json!({"ranges":[]})} else {p.selection.clone()}});
        if !p.loading && p.source.is_object() {v["source"]=p.source.clone()}
        json!({"path":p.path,"active":self.active==i,"heading":self.heading(i),"view":v})
    }
    fn ui(&self)->Value {json!({"left_heading":self.heading(0),"right_heading":self.heading(1),
        "left_active":self.active==0,"right_active":self.active==1,"status":self.status,"has_drop":false})}
    async fn patch(&self,client:&SupervisedClient,path:&str,value:Value)->Result<()> {
        client.call(&self.host,"shell.scene.patch",json!({"scene":"dopus-lab","path":path,"value":value})).await?;Ok(())
    }
    async fn metadata(&mut self,client:&SupervisedClient)->Result<()> {
        let next=self.ui();if self.last_ui!=next {self.patch(client,"model.meta",next.clone()).await?;self.last_ui=next}Ok(())
    }
    async fn paint(&mut self,client:&SupervisedClient,i:usize)->Result<()> {
        self.panes[i].revision+=1;
        self.patch(client,if i==0 {"model.left"}else{"model.right"},self.view(i)).await?;
        self.metadata(client).await
    }
    fn dispatch(&mut self)->Result<()> {
        if self.busy.is_some() {return Ok(())}
        for i in [self.turn,1-self.turn] {
            if let Some(mut q)=self.pending[i].take() {
                q["keep"]=json!(self.panes.iter().filter_map(|p|p.source["path"].as_str()).collect::<Vec<_>>());
                self.jobs.try_send(q.clone())?;self.busy=Some(q);self.turn=1-i;break;
            }
        }Ok(())
    }
    async fn request(&mut self,client:&SupervisedClient,i:usize,kind:&str)->Result<()> {
        let p=&mut self.panes[i];p.generation+=1;p.loading=true;p.error=None;
        let q=json!({"owner":if i==0 {"dopus-lab.list_left"}else{"dopus-lab.list_right"},
            "token":format!("rust-{i}-{}",p.generation),"session":self.session,"pane":i,
            "generation":p.generation,"path":p.path,"root":self.root,"hidden":false,"expanded":{},
            "selection":p.selected,"focus":p.focus_id,"key":p.key,"ascending":p.ascending,"kind":kind});
        self.pending[i]=Some(q);self.status="Loading…".into();self.paint(client,i).await?;self.dispatch()
    }
    async fn complete(&mut self,client:&SupervisedClient,r:Value)->Result<()> {
        let Some(q)=self.busy.take() else{return Ok(())};
        if r["token"]!=q["token"] || r["generation"]!=q["generation"] || r["pane"]!=q["pane"] {
            return Err("worker correlation mismatch".into())
        }
        let i=q["pane"].as_u64().ok_or("worker pane missing")? as usize;
        if r["generation"].as_u64()==Some(self.panes[i].generation) {
            let p=&mut self.panes[i];p.loading=false;
            if !r["error"].is_null() {p.error=Some(r["message"].as_str().unwrap_or("worker error").into());p.total=0;p.source=Value::Null}
            else {
                if r["path"].as_str()!=Some(&p.path) || r["schema"]!="native-model-v1" {return Err("worker model mismatch".into())}
                p.source=r["model"].clone();p.total=r["total"].as_u64().ok_or("worker total missing")? as usize;
                p.selection=r["selection"].clone();p.selected=serde_json::from_value(r["selected_ids"].clone())?;
                p.focus=if r["focus_index"].as_i64().unwrap_or(-1)<0 {Value::Null}else{r["focus_index"].clone()};p.epoch+=1;
            }
            self.serial+=1;self.status=p.error.clone().unwrap_or_else(||"Ready".into());self.paint(client,i).await?;
        }self.dispatch()
    }
    async fn apply(&mut self,client:&SupervisedClient,command:&str,args:&Value)->Result<()> {
        let i=args["pane"].as_u64().unwrap_or(self.active as u64) as usize;if i>1 {return Err("invalid pane".into())}
        match command {
            "open"=>{
                let path=std::fs::canonicalize(args["path"].as_str().ok_or("path missing")?)?;
                if !path.starts_with(&self.root) || !path.is_dir() {return Err("outside fixture or not directory".into())}
                let p=&mut self.panes[i];p.path=path.to_str().ok_or("non UTF-8 path")?.into();
                p.selected.clear();p.selection=json!({"ranges":[]});p.focus=Value::Null;p.focus_id=Value::Null;
                self.request(client,i,"scan").await?
            },
            "refresh"=>self.request(client,i,"scan").await?,
            "sort"=>{
                let key=args["key"].as_str().ok_or("sort key missing")?;
                if !["name","size","modified"].contains(&key) {return Err("invalid sort key".into())}
                let p=&mut self.panes[i];p.ascending=if p.key==key {!p.ascending}else{true};p.key=key.into();
                self.request(client,i,"sort").await?
            },
            "activate"=>{self.active=i;self.metadata(client).await?},
            _=>return Err("unsupported command in subset policy control".into()),
        }Ok(())
    }
    async fn interaction(&mut self,client:&SupervisedClient,i:usize,v:&Value)->Result<()> {
        let p=&mut self.panes[i];
        if v["session"]!=self.session || v["generation"].as_u64()!=Some(p.generation)
            || v["seq"].as_u64().unwrap_or(0)<=p.ack {return Ok(())}
        p.ack=v["seq"].as_u64().unwrap();
        match v["kind"].as_str().unwrap_or("") {
            "selection"=>{p.selection=v["selection"].clone();p.selected=serde_json::from_value(v["selected_ids"].clone())?;
                p.focus=v["focus"].clone();p.focus_id=v["focus_id"].clone();self.active=i;self.serial+=1;self.metadata(client).await?},
            "model_error"=>{p.loading=false;p.error=Some(v["error"].as_str().unwrap_or("native model error").into());
                p.source=Value::Null;p.total=0;self.status="Native model error".into();self.paint(client,i).await?},
            "key" if v["key"]=="Tab"=>{self.active=1-i;self.metadata(client).await?},
            _=>{}, // No file operations in this deliberately limited control.
        }Ok(())
    }
    fn snapshot(&self)->Value {json!({"policy":"rust-common-worker-subset","active":self.active,
        "serial":self.serial,"worker_bytes":self.worker_bytes,"retained_controller_rows":[0,0],
        "panes":self.panes.iter().map(|p|json!({"path":p.path,"rows":if p.loading {0}else{p.total},
            "loading":p.loading,"error":p.error,"generation":p.generation,"view_revision":p.revision,
            "selected":p.selected.first(),"selected_count":p.count(),"sort":p.key,"ascending":p.ascending})).collect::<Vec<_>>()})}
}

#[tokio::main(flavor="current_thread")]
async fn main()->Result<()> {
    let args:Vec<_>=std::env::args().collect();
    if args.len()!=6 {return Err("arguments: HOST FIXTURE_ROOT SCENE MIX_BINARY WORKER_SCRIPT".into())}
    let root=std::fs::canonicalize(&args[2])?;
    let (jobs,rx)=mpsc::sync_channel::<Value>(1);
    let (results,mut output)=tokio::sync::mpsc::channel::<Result<(Value,usize)>>(2);
    let mut child=Command::new(&args[4]).arg(&args[5]).env("DOPUS_BULK_LIST","1").env("DOPUS_NATIVE_SORT","1")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn()?;
    let mut stdin=child.stdin.take().ok_or("worker stdin")?;
    let mut stdout=BufReader::new(child.stdout.take().ok_or("worker stdout")?);
    let worker=tokio::task::spawn_blocking(move || {
        while let Ok(q)=rx.recv() {
            let result=(||->Result<(Value,usize)> {
                writeln!(stdin,"{q}")?;stdin.flush()?;let mut line=String::new();
                if stdout.read_line(&mut line)?==0 {return Err("worker exited".into())}
                let len=line.trim_end_matches('\n').len();Ok((serde_json::from_str(&line)?,len))
            })();
            let failed=result.is_err();if results.blocking_send(result).is_err() || failed {break}
        }
        let _=child.kill();let _=child.wait();
    });
    let client=SupervisedClient::connect_options(NAME,&bus::noded_url()).bounded_incoming_rpc(64)
        .fatal_on_registration_rejection(true).connect().await?;
    let mut incoming=client.incoming_bounded().ok_or("incoming unavailable")?;
    let session=format!("rust-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos());
    let mut policy=Policy {root:root.clone(),session:session.clone(),host:args[1].clone(),
        panes:[Pane::new(root.join("left-100").to_str().ok_or("root")?.into()),Pane::new(root.join("right-100").to_str().ok_or("root")?.into())],
        active:0,status:"Loading…".into(),pending:[None,None],busy:None,turn:0,worker_bytes:0,serial:0,last_ui:Value::Null,jobs};
    let mut source=std::fs::read_to_string(&args[3])?.replace("DOPUS_VIEW_SESSION",&session).replace("dopus-mix-lab",NAME);
    for (old,new) in [("$model.left.heading","$model.meta.left_heading"),("$model.right.heading","$model.meta.right_heading"),
        ("$model.left.active","$model.meta.left_active"),("$model.right.active","$model.meta.right_active"),
        ("$model.status","$model.meta.status"),("$model.has_drop","$model.meta.has_drop")] {source=source.replace(old,new)}
    source=source.replace("\"row_height\":24",&format!("\"row_height\":{},\"font_size\":{},\"secondary_scale\":{}",
        std::env::var("DOPUS_ROW_HEIGHT").unwrap_or("24".into()),std::env::var("DOPUS_FONT_SIZE").unwrap_or("13".into()),std::env::var("DOPUS_SECONDARY_SCALE").unwrap_or("1".into())));
    client.call(&policy.host,"shell.scene.load",json!({"source":source})).await?;
    client.call(&policy.host,"shell.scene.patch",json!({"scene":"dopus-lab","path":"model",
        "value":{"left":policy.view(0),"right":policy.view(1),"meta":policy.ui(),"status":"Loading…","has_drop":false}})).await?;
    policy.request(&client,0,"scan").await?;policy.request(&client,1,"scan").await?;
    client.call(&policy.host,"shell.dialog.show",json!({"scene":"dopus-lab"})).await?;
    loop {tokio::select! {
        Some(result)=output.recv()=>{let (r,len)=result?;policy.worker_bytes+=len as u64;policy.complete(&client,r).await?},
        event=incoming.recv()=>{
            let Some(BoundedIncomingEvent::Command(request))=event else {return Err("incoming closed or overflowed".into())};
            let command=request.command.strip_prefix(&format!("{NAME}.")).unwrap_or(&request.command);
            let response:Result<Value>=match command {
                "snapshot"|"state"=>Ok(policy.snapshot()),
                "apply"=>policy.apply(&client,request.args["command"].as_str().unwrap_or(""),&request.args["params"]).await.map(|()|policy.snapshot()),
                "table_left"|"table_right"=>policy.interaction(&client,usize::from(command=="table_right"),&request.args["value"]).await.map(|()|json!({})),
                "stop"=>break,
                name if name.starts_with("sort_")=>{
                    let parts:Vec<_>=name.split('_').collect();
                    if parts.len()==3 {policy.apply(&client,"sort",&json!({"pane":usize::from(parts[2]=="right"),"key":parts[1]})).await.map(|()|json!({}))}
                    else {Err("unsupported sort event".into())}
                },
                _=>Err("unsupported command in subset policy control".into()),
            };
            if request.id.is_some() {let (rc,body)=match response {Ok(v)=>(0,v),Err(e)=>(1,json!({"error":e.to_string()}))};client.respond(&request,rc,&body.to_string()).await?;}
        }
    }}
    client.close().await;drop(policy);worker.await?;Ok(())
}
