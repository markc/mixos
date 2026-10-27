// SPDX-License-Identifier: MIT OR Apache-2.0
//! Activation must distinguish a mailbox from its same-process composers.
use futures::StreamExt;
use serde_json::{Value, json};

#[test]
fn show_matches_exact_app_id_and_pid_even_when_a_child_is_listed_first() {
    let broker = test_broker::Broker::start();
    let (comp, mut requests) = citizen::start("comp", "comp-test", &broker.url).unwrap();
    let (app, _deliveries) = citizen::start("email", "email-show-test", &broker.url).unwrap();
    let responder = comp.clone();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let mut seen = Vec::new();
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                while seen.len() < 4 {
                    if let Some(citizen::Delivery::Command { id, command, body }) = requests.next().await {
                        let args: Value = serde_json::from_str(&body).unwrap();
                        let answer = match command.as_str() {
                            "comp.window.wait" => {
                                assert_eq!(args["match"]["app_id"], "dev.mixos.email");
                                json!({"mapped":true})
                            }
                            "comp.windows.list" => json!({"windows":[
                                {"app_id":"dev.mixos.email.compose-2","pid":std::process::id(),"id":2,"generation":3},
                                {"app_id":"dev.mixos.email","pid":u64::from(std::process::id())+1,"id":9,"generation":9},
                                {"app_id":"dev.mixos.email","pid":std::process::id(),"id":1,"generation":4}
                            ]}),
                            "comp.window.restore" | "comp.window.focus" => {
                                assert_eq!(args["id"], 1);
                                assert_eq!(args["generation"], 4);
                                json!({"id":1,"generation":4,"minimized":false,"focused":true})
                            }
                            other => panic!("unexpected command: {other}"),
                        };
                        seen.push(command);
                        responder.reply(id, 0, answer);
                    }
                }
            }).await.unwrap();
            seen
        })
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let answer = runtime
        .block_on(citizen::show(
            app.clone(),
            "comp-test".into(),
            "dev.mixos.email",
        ))
        .unwrap();
    assert_eq!(answer["shown"], true);
    assert_eq!(worker.join().unwrap().last().unwrap(), "comp.window.focus");
    app.quit();
    comp.quit();
}
