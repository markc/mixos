// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded discovery harness, CPU frame work only; no physical presentation claim.
use super::{model::State, *};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    time::Instant,
};

fn row(i: usize) -> Value {
    let id = format!("row-{i}");
    json!({"id":id,"cells":[id,"0","1700000000"],"edit":id})
}
fn initial(n: usize) -> State {
    let mut s = State::default();
    s.session = "limit".into();
    for offset in (0..n).step_by(256) {
        let v = json!({"generation":1,"revision":s.revision+1,"delta":{"kind":"splice","reset":offset==0,"base_revision":s.revision,"index":offset,"delete":[],"insert":(offset..(offset+256).min(n)).map(row).collect::<Vec<_>>()}});
        assert!(s.ingest(&v));
    }
    s
}

fn scene_store() -> crate::store::SceneStore {
    let mut store = crate::store::SceneStore::default();
    let source = "---\nscene: 1\nname: limit\ncitizen: limit\nmodel: {\"view\": {}}\n---\n```mix\nroot: {widget: column, children: [table]}\ntable: {widget: table, rows: [], columns: [Name, Size, Modified], session: limit, view: '= $model.view'}\n```\n";
    store
        .request(crate::command::SceneCommand::Load, source, &Value::Null)
        .unwrap();
    store
}

#[test]
fn older_paint_snapshot_does_not_replay_an_accepted_delta() {
    let mut s = initial(1000);
    let revision = s.revision;
    let ctx = egui::Context::default();
    let old = json!({"session":"limit","generation":1,"revision":1,"delta":{"base_revision":0,"kind":"splice","reset":true,"index":0,"delete":[],"insert":[row(0)]}});
    let refused = s.refused;
    paint(&ctx, &mut s, old);
    assert_eq!(s.refused, refused);
    assert_eq!(s.revision, revision);
    assert_eq!(s.total, 1000);
    s.model_error = Some("older source failed".into());
    let edit = json!({"generation":1,"revision":revision+1,"delta":{"base_revision":revision,"kind":"edit","edits":[{"index":0,"row":row(0)}]}});
    assert!(s.ingest(&edit));
    assert!(s.model_error.is_none());
}

#[test]
fn old_session_paint_cannot_erase_a_new_accepted_owner() {
    let mut s=initial(1000); s.session="new-owner".into();
    let before=s.revision; let refused=s.refused;
    let old=json!({"session":"limit","generation":1,"revision":1,"delta":{"kind":"edit","edits":[]}});
    paint(&egui::Context::default(),&mut s,old);
    assert_eq!(s.session,"new-owner"); assert_eq!(s.revision,before);
    assert_eq!(s.refused,refused); assert_eq!(s.incremental.as_ref().unwrap().row(999).unwrap().id,"row-999");
}

#[test]
fn scene_refusals_preserve_both_scene_and_native_data() {
    use crate::command::SceneCommand;
    let mut store=scene_store();
    let view=json!({"session":"limit","generation":1,"revision":1,"delta":{"kind":"splice","reset":true,"base_revision":0,"index":0,"delete":[],"insert":[row(0),row(1)]}});
    store.request(SceneCommand::Patch,"",&json!({"scene":"limit","path":"model.view","value":view})).unwrap();
    let before=serde_json::to_value(store.scene("limit").unwrap().tree()).unwrap();
    let revision=store.scene("limit").unwrap().revision();
    let mut bad=view.clone(); bad["revision"]=json!(2); bad["delta"]=json!({"kind":"edit","base_revision":1,"edits":[{"index":3,"row":row(3)}]});
    assert!(store.request(SceneCommand::Patch,"",&json!({"scene":"limit","path":"model.view","value":bad})).is_err());
    assert_eq!(store.scene("limit").unwrap().revision(),revision);
    assert_eq!(serde_json::to_value(store.scene("limit").unwrap().tree()).unwrap(),before);
    {let entry=store.scene("limit").unwrap();let tables=entry.native_tables.lock().unwrap();let s=&tables["table"];assert_eq!(s.revision,1);assert_eq!(s.total,2);assert_eq!(s.incremental.as_ref().unwrap().row(1).unwrap().id,"row-1");}
    let mut direct=view.clone(); direct["revision"]=json!(2);
    assert!(store.request(SceneCommand::Patch,"",&json!({"scene":"limit","path":"table.view","value":direct})).is_err());
    assert!(store.request(SceneCommand::Patch,"",&json!({"scene":"limit","path":"model.view","value":{}})).is_err());
    assert_eq!(store.scene("limit").unwrap().revision(),revision);
    let source=format!("---\nscene: 1\nname: literal\ncitizen: literal\n---\n```mix\nroot: {{widget: table, rows: [], columns: [Name], session: limit, view: {view}}}\n```\n");
    assert!(store.request(SceneCommand::Load,&source,&Value::Null).is_err());
    assert!(store.scene("literal").is_none());
}

#[test]
fn selection_and_local_edit_buffers_follow_stable_identity() {
    let mut s=initial(10); s.pick(1,false,false); s.pick(3,true,false);
    s.edit=Some(super::model::Edit{index:3,id:"row-3".into(),text:"unsaved".into(),pending:None,focus:true});
    let update=json!({"generation":1,"revision":s.revision+1,"delta":{"base_revision":s.revision,"kind":"edit","edits":[{"index":3,"row":row(3)}]}});
    assert!(s.ingest(&update)); assert_eq!(s.edit.as_ref().unwrap().text,"unsaved");
    let reverse=json!({"generation":1,"revision":s.revision+1,"delta":{"base_revision":s.revision,"kind":"reverse"}});
    assert!(s.ingest(&reverse)); assert_eq!(s.edit.as_ref().unwrap().index,6);
    assert!(s.selection.contains(6)); assert!(s.selection.contains(8)); assert_eq!(s.selection.count(),2);
    let delete=json!({"generation":1,"revision":s.revision+1,"delta":{"base_revision":s.revision,"kind":"splice","index":6,"delete":["row-3"],"insert":[]}});
    assert!(s.ingest(&delete)); assert!(s.edit.is_none()); assert_eq!(s.selection.count(),1);
    let reset=json!({"generation":1,"revision":s.revision+1,"delta":{"base_revision":s.revision,"reset":true,"kind":"reverse"}});
    assert!(!s.ingest(&reset));
    let source=json!({"generation":1,"revision":s.revision+1,"source":{},"delta":{"base_revision":s.revision,"kind":"reverse"}});
    assert!(!s.ingest(&source));
}

#[test]
fn incremental_projection_is_bounded_across_many_viewports() {
    let mut s=initial(10000);
    for first in (0..9900).step_by(128) {
        s.prepare_range(first,first+27);
        assert!(s.rows.len()<=super::model::ROW_BUDGET);
        assert_eq!(s.rows[&first].id,format!("row-{first}"));
    }
}

fn policy(r: &Value) -> Value {
    let started = Instant::now();
    let n = r["total"].as_u64().unwrap() as usize;
    let k = r["changed"].as_u64().unwrap() as usize;
    let seq = r["seq"].as_u64().unwrap();
    let revision = r["revision"].as_u64().unwrap();
    let delta = match r["kind"].as_str().unwrap() {
        "edit" => json!({"kind":"edit","base_revision":revision,"edits":(0..k).map(|j| {
            let i=if r["visible"]==true {r["start"].as_u64().unwrap_or(0) as usize+j} else {n-k+j};
            let mut value=row(i); value["cells"][1]=json!(seq.to_string());
            json!({"index":i,"row":value})
        }).collect::<Vec<_>>()}),
        "splice" => {
            if seq % 2 == 0 {
                let id = format!("added-{seq}");
                json!({"kind":"splice","base_revision":revision,"index":0,"delete":[],"insert":[{"id":id,"cells":[id,"0","1700000000"],"edit":id}]})
            } else {
                json!({"kind":"splice","base_revision":revision,"index":0,"delete":[format!("added-{}",seq-1)],"insert":[]})
            }
        }
        "reverse" => json!({"kind":"reverse","base_revision":revision}),
        "sort" => json!({"kind":"sort","base_revision":revision,"column":0,"ascending":seq%2==0}),
        "order" => {
            json!({"kind":"order","base_revision":revision,"ids":(0..n).map(|j|format!("row-{}",if seq%2==0 {n-1-j}else{j})).collect::<Vec<_>>()})
        }
        _ => panic!("bad request"),
    };
    json!({"generation":1,"revision":revision+1,"delta":delta,"policy_us":started.elapsed().as_secs_f64()*1e6})
}

#[test]
fn policy_intents_match_hand_written_expected_transactions() {
    let cases:Vec<Value>=serde_json::from_str(include_str!("../../../tests/fixtures/incremental_intents.json")).unwrap();
    for case in cases {
        let mut actual=policy(&case["request"]); actual.as_object_mut().unwrap().remove("policy_us");
        assert_eq!(actual,case["expected"]);
    }
}

#[test]
#[ignore]
fn rust_policy_process() {
    for line in std::io::stdin().lock().lines() {
        let r: Value = serde_json::from_str(&line.unwrap()).unwrap();
        println!("{}", policy(&r));
        std::io::stdout().flush().unwrap();
    }
}

struct Controller {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Controller {
    fn new(kind: &str) -> Self {
        let mut cmd = if kind == "mix" {
            let mut c = Command::new(std::env::var("LIMIT_MIX").expect("LIMIT_MIX"));
            c.arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/incremental_policy.mix"
            ));
            c
        } else {
            let mut c = Command::new(std::env::current_exe().unwrap());
            c.args([
                "--exact",
                "view::table::limit::rust_policy_process",
                "--ignored",
                "--nocapture",
                "--quiet",
            ]);
            c
        };
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
        }
    }
    fn call(&mut self, r: &Value) -> (Value, usize) {
        writeln!(self.input, "{r}").unwrap();
        self.input.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(self.output.read_line(&mut line).unwrap() > 0, "policy EOF");
            if line.starts_with('{') {
                return (serde_json::from_str(&line).unwrap(), line.len());
            }
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn route() -> Arc<Route> {
    Arc::new(Route {
        scene: "limit".into(),
        citizen: "limit".into(),
        node: "table".into(),
        item: None,
        click: None,
        context: None,
        change: Some("changed".into()),
        submit: None,
    })
}
fn paint(ctx: &egui::Context, s: &mut State, view: Value) -> (f64, f64) {
    let node:scene::Node=serde_json::from_value(json!({"family":"table","ports":{"view":view,"session":"limit","columns":["Name","Size","Modified"],"active":true,"clip_columns":true,"row_height":24},"line":1,"is_template":false})).unwrap();
    let rect = Rect::from_min_size(pos2(0., 0.), vec2(980., 640.));
    let started = Instant::now();
    let mut out = Vec::new();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        },
        |ui| super::paint(ui, rect, rect, &node, route(), &mut out, s, 0),
    );
    let frame = started.elapsed().as_secs_f64() * 1e6;
    let started = Instant::now();
    let _meshes = ctx.tessellate(output.shapes, output.pixels_per_point);
    output.textures_delta.clear(); // CPU harness has no texture uploader.
    (frame, started.elapsed().as_secs_f64() * 1e6)
}

#[test]
fn atomic_deltas_preserve_selection_identity_and_reject_stale_or_invalid_input() {
    let mut s = initial(1000);
    s.pick(4, false, false);
    s.prepare_range(0, 30);
    let rev = s.revision;
    let bad = json!({"generation":1,"revision":rev+1,"delta":{"base_revision":rev,"kind":"edit","edits":[{"index":4,"row":row(4)},{"index":5,"row":row(6)}]}});
    assert!(!s.ingest(&bad));
    assert_eq!(s.revision, rev);
    let mut inserted = row(1001);
    inserted["id"] = json!("inserted");
    let splice = json!({"generation":1,"revision":rev+1,"delta":{"base_revision":rev,"kind":"splice","index":0,"delete":[],"insert":[inserted]}});
    assert!(s.ingest(&splice));
    assert_eq!(s.focus, Some(5));
    assert_eq!(s.event("selection")["focus_id"], "row-4");
    assert!(!s.ingest(&bad));
    assert_eq!(s.total, 1001);
    let mut duplicate = splice.clone();
    duplicate["revision"] = json!(s.revision + 1);
    duplicate["delta"]["base_revision"] = json!(s.revision);
    assert!(!s.ingest(&duplicate));
    assert_eq!(s.total, 1001);
    let mut reorder: Vec<_> = (0..1000).rev().map(|i| json!(format!("row-{i}"))).collect();
    reorder.push(json!("inserted"));
    let order = json!({"generation":1,"revision":s.revision+1,"delta":{"base_revision":s.revision,"kind":"order","ids":reorder}});
    assert!(s.ingest(&order));
    assert_eq!(s.focus, Some(995));
    s.remount();
    assert_eq!(s.event("selection")["focus_id"], "row-4");
    let delete = json!({"generation":1,"revision":s.revision+1,"delta":{"base_revision":s.revision,"kind":"splice","index":995,"delete":["row-4"],"insert":[]}});
    assert!(s.ingest(&delete));
    assert_eq!(s.focus, None);
    assert_eq!(s.selection.count(), 0);
    let mut gap = delete;
    gap["revision"] = json!(s.revision + 2);
    gap["delta"]["base_revision"] = json!(s.revision);
    assert!(!s.ingest(&gap));
    assert_eq!(s.total, 1000);
}

#[test]
fn accepted_deltas_are_not_lost_when_paints_coalesce() {
    use crate::{command::SceneCommand, store::SceneStore};
    let mut store = SceneStore::default();
    let source = "---\nscene: 1\nname: limit\ncitizen: limit\nmodel: {\"view\": {}}\n---\n```mix\nroot: {widget: table, rows: [], columns: [Name], session: limit, view: '= $model.view'}\n```\n";
    store
        .request(SceneCommand::Load, source, &Value::Null)
        .unwrap();
    for i in 0..3 {
        let v = json!({"session":"limit","generation":1,"revision":i+1,"delta":{"reset":i==0,"base_revision":i,"kind":"splice","index":i,"delete":[],"insert":[row(i as usize)]}});
        store
            .request(
                SceneCommand::Patch,
                "",
                &json!({"scene":"limit","path":"model.view","value":v}),
            )
            .unwrap();
    }
    let entry = store.scene("limit").unwrap();
    let tables = entry.native_tables.lock().unwrap();
    assert_eq!(tables["root"].total, 3);
    assert_eq!(tables["root"].revision, 3);
    for i in 0..3 { assert_eq!(tables["root"].incremental.as_ref().unwrap().row(i).unwrap().raw,row(i)); }
}

fn pss(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
        .unwrap()
        .lines()
        .find(|line| line.starts_with("Pss:"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn preserved_dopus_oracle_survives_incremental_edits_and_reorders() {
    let capture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/incremental_oracle.json"
    ))
    .unwrap();
    assert_eq!(capture["cases"].as_array().unwrap().len(), 25);
    for case in capture["cases"].as_array().unwrap() {
        let rows = case["rows"].as_array().unwrap();
        let mut s = State::default();
        s.session = "limit".into();
        for (batch, chunk) in rows.chunks(256).enumerate() {
            let view = json!({"generation":1,"revision":s.revision+1,"delta":{"kind":"splice","reset":batch==0,"base_revision":s.revision,"index":batch*256,"delete":[],"insert":chunk}});
            assert!(s.ingest(&view));
        }
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(s.incremental.as_ref().unwrap().row(i).unwrap().raw, *row);
        }
        s.pick(0, false, false);
        let mut changed = rows[0].clone();
        changed["cells"][1] = json!("changed");
        let view = json!({"generation":1,"revision":s.revision+1,"delta":{"kind":"edit","base_revision":s.revision,"edits":[{"index":0,"row":changed}]}});
        assert!(s.ingest(&view));
        for (i, row) in rows.iter().enumerate().skip(1) {
            assert_eq!(s.incremental.as_ref().unwrap().row(i).unwrap().raw, *row);
        }
        let view = json!({"generation":1,"revision":s.revision+1,"delta":{"kind":"reverse","base_revision":s.revision}});
        assert!(s.ingest(&view));
        assert_eq!(s.focus, Some(rows.len() - 1));
        for (i, row) in rows.iter().rev().enumerate().take(rows.len() - 1) {
            assert_eq!(s.incremental.as_ref().unwrap().row(i).unwrap().raw, *row);
        }
    }
}

#[test]
#[ignore]
fn measure_incremental_limit() {
    let output_path = std::env::var("LIMIT_OUTPUT").expect("LIMIT_OUTPUT");
    let mut file = std::fs::File::create(output_path).unwrap();
    let repeats: usize = std::env::var("LIMIT_REPEATS")
        .unwrap_or("3".into())
        .parse()
        .unwrap();
    let samples: usize = std::env::var("LIMIT_SAMPLES")
        .unwrap_or("300".into())
        .parse()
        .unwrap();
    for repeat in 0..repeats {
        for n in [1000, 10000, 100000] {
            if let Ok(only)=std::env::var("LIMIT_ONLY_N") { if n!=only.parse::<usize>().unwrap() {continue;} }
            {
                let mut s = initial(n);
                let ctx = egui::Context::default();
                let view =
                    json!({"session":"limit","generation":1,"revision":s.revision,"delta":{}});
                for kind in ["unchanged", "local-selection"] {
                    for seq in 0..samples + 20 {
                        let started = Instant::now();
                        if kind == "local-selection" {
                            s.pick(seq % 24, false, false);
                            let _event = s.event("selection");
                        }
                        let apply_us = started.elapsed().as_secs_f64() * 1e6;
                        let (frame_us, tess_us) = paint(&ctx, &mut s, view.clone());
                        if seq >= 20 {
                            writeln!(file,"{}",json!({"repeat":repeat,"n":n,"kind":kind,"changed":0,"visible":true,"controller":"native-local","seq":seq-20,"policy_us":0,"ipc_us":0,"apply_us":apply_us,"frame_us":frame_us,"tess_us":tess_us,"serial_us":apply_us+frame_us+tess_us,"reply_bytes":0,"cache_rows":s.rows.len()})).unwrap();
                        }
                    }
                }
            }
            for (kind, k, visible) in [
                ("edit", 1, true),
                ("edit", 16, true),
                ("edit", 128, true),
                ("edit", 256, true),
                ("edit", 1, false),
                ("edit", 128, false),
                ("edit", 256, false),
                ("splice", 1, true),
                ("reverse", n, false),
                ("sort", n, false),
                ("order", n, false),
            ] {
                if std::env::var("LIMIT_REPRESENTATIVE").is_ok()
                    && !((kind=="edit" && visible && (k==1 || k==256)) || kind=="splice" || kind=="sort") {continue;}
                let count = if kind == "order" || kind == "sort" {
                    32
                } else {
                    samples
                };
                for controller_kind in if repeat % 2 == 0 {
                    ["mix", "rust"]
                } else {
                    ["rust", "mix"]
                } {
                    let mut controller = Controller::new(controller_kind);
                    let baseline_pss_kib = pss(std::process::id());
                    let started = Instant::now();
                    let mut store = scene_store();
                    {
                        let entry = store.scenes.get_mut("limit").unwrap();
                        let mut state = initial(n);
                        state.pick(4, false, false);
                        entry
                            .native_tables
                            .lock()
                            .unwrap()
                            .insert("table".into(), state);
                    }
                    let setup_us = started.elapsed().as_secs_f64() * 1e6;
                    let ctx = egui::Context::default();
                    let tables = Arc::clone(&store.scene("limit").unwrap().native_tables);
                    let mut revision = tables.lock().unwrap()["table"].revision;
                    let initial_view =
                        json!({"session":"limit","generation":1,"revision":revision,"delta":{}});
                    for _ in 0..10 {
                        paint(
                            &ctx,
                            tables.lock().unwrap().get_mut("table").unwrap(),
                            initial_view.clone(),
                        );
                    }
                    for seq in 0..count + 20 {
                        let request = json!({"kind":kind,"changed":k,"visible":visible,"seq":seq,"total":n,"revision":revision});
                        let started = Instant::now();
                        let (mut view, bytes) = controller.call(&request);
                        let ipc = started.elapsed().as_secs_f64() * 1e6;
                        let mut expected = policy(&request);
                        expected.as_object_mut().unwrap().remove("policy_us");
                        let policy_us = view
                            .as_object_mut()
                            .unwrap()
                            .remove("policy_us")
                            .unwrap()
                            .as_f64()
                            .unwrap();
                        assert_eq!(view, expected, "policy parity");
                        view["session"] = json!("limit");
                        let started = Instant::now();
                        if let Err(error) = store.request(
                            crate::command::SceneCommand::Patch,
                            "",
                            &json!({"scene":"limit","path":"model.view","value":view}),
                        ) {
                            assert!(kind == "order" && n == 100000, "unexpected refusal {error}");
                            writeln!(file,"{}",json!({"record":"refusal","repeat":repeat,"n":n,"kind":kind,"controller":controller_kind,"reply_bytes":bytes,"error":error,"ipc_us":ipc})).unwrap();
                            break;
                        }
                        let apply_us = started.elapsed().as_secs_f64() * 1e6;
                        let mut lock = tables.lock().unwrap();
                        let s = lock.get_mut("table").unwrap();
                        revision = s.revision;
                        let (frame_us, tess_us) = paint(&ctx, s, view);
                        assert!(s.rows.len() <= super::model::ROW_BUDGET);
                        assert!(s.end>s.first && s.shapes>0);
                        if kind=="edit" && visible {assert_eq!(s.rows[&0].cells[1],seq.to_string());}
                        if seq >= 20 {
                            writeln!(file,"{}",json!({"repeat":repeat,"n":n,"kind":kind,"changed":k,"visible":visible,"controller":controller_kind,"seq":seq-20,"policy_us":policy_us,"ipc_us":ipc,"apply_us":apply_us,"frame_us":frame_us,"tess_us":tess_us,"serial_us":ipc+apply_us+frame_us+tess_us,"reply_bytes":bytes,"cache_rows":s.rows.len(),"snapshot":if seq==count+19 {s.snapshot()}else{Value::Null}})).unwrap();
                        }
                    }
                    writeln!(file,"{}",json!({"record":"resources","repeat":repeat,"n":n,"kind":kind,"changed":k,"visible":visible,"controller":controller_kind,"setup_us":setup_us,"baseline_pss_kib":baseline_pss_kib,"parent_pss_kib":pss(std::process::id()),"controller_pss_kib":pss(controller.child.id())})).unwrap();
                    file.flush().unwrap();
                    println!(
                        "LIMIT repeat={repeat} n={n} kind={kind} k={k} visible={visible} controller={controller_kind}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore]
fn measure_incremental_burst_limit() {
    let mut file = std::io::BufWriter::new(
        std::fs::File::create(std::env::var("LIMIT_OUTPUT").unwrap()).unwrap(),
    );
    let n = 100000;
    let samples: usize = std::env::var("LIMIT_SAMPLES")
        .unwrap_or("120".into())
        .parse()
        .unwrap();
    for repeat in 0..3 {
        for changed in [256usize, 512, 768, 1024, 2048] {
            for kind in if repeat % 2 == 0 {
                ["mix", "rust"]
            } else {
                ["rust", "mix"]
            } {
                let mut controller = Controller::new(kind);
                let mut store = scene_store();
                let tables = Arc::clone(&store.scene("limit").unwrap().native_tables);
                tables.lock().unwrap().insert("table".into(), initial(n));
                let mut revision = tables.lock().unwrap()["table"].revision;
                let ctx = egui::Context::default();
                for seq in 0..samples + 20 {
                    let mut policy_us = 0.;
                    let mut ipc_us = 0.;
                    let mut apply_us = 0.;
                    let mut bytes = 0;
                    let mut last = Value::Null;
                    for start in (0..changed).step_by(256) {
                        let request = json!({"kind":"edit","changed":256,"start":start,"visible":true,"seq":seq,"total":n,"revision":revision});
                        let began = Instant::now();
                        let (mut view, length) = controller.call(&request);
                        ipc_us += began.elapsed().as_secs_f64() * 1e6;
                        bytes += length;
                        policy_us += view
                            .as_object_mut()
                            .unwrap()
                            .remove("policy_us")
                            .unwrap()
                            .as_f64()
                            .unwrap();
                        let mut expected = policy(&request);
                        expected.as_object_mut().unwrap().remove("policy_us");
                        assert_eq!(view, expected);
                        view["session"] = json!("limit");
                        let began = Instant::now();
                        store
                            .request(
                                crate::command::SceneCommand::Patch,
                                "",
                                &json!({"scene":"limit","path":"model.view","value":view}),
                            )
                            .unwrap();
                        apply_us += began.elapsed().as_secs_f64() * 1e6;
                        revision += 1;
                        {let lock=tables.lock().unwrap();let state=&lock["table"];
                         assert_eq!(state.revision,revision); assert_eq!((state.refused,state.stale),(0,0));
                         for index in [start,start+255] {assert_eq!(state.incremental.as_ref().unwrap().row(index).unwrap().cells[1],seq.to_string());}}
                        last = view;
                    }
                    let mut lock = tables.lock().unwrap();
                    let s = lock.get_mut("table").unwrap();
                    assert_eq!(s.revision, revision);
                    let (frame_us, tess_us) = paint(&ctx, s, last);
                    assert!(s.end>s.first && !s.rows.is_empty() && s.shapes>0);
                    assert_eq!(s.rows[&0].cells[1],seq.to_string());
                    if seq >= 20 {
                        writeln!(file,"{}",json!({"repeat":repeat,"n":n,"kind":"burst","changed":changed,"visible":true,"controller":kind,"seq":seq-20,"policy_us":policy_us,"ipc_us":ipc_us,"apply_us":apply_us,"frame_us":frame_us,"tess_us":tess_us,"serial_us":ipc_us+apply_us+frame_us+tess_us,"reply_bytes":bytes,"cache_rows":s.rows.len()})).unwrap();
                    }
                }
                file.flush().unwrap();
                println!("BURST repeat={repeat} changed={changed} controller={kind}");
            }
        }
    }
}
