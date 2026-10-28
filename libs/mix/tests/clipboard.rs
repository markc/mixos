// SPDX-License-Identifier: MIT OR Apache-2.0
use indexmap::IndexMap;
use mix::error::MixResult;
use mix::evaluator::{BusFuture, BusHandler, Evaluator, IncomingEvent, SharedBuf};
use mix::value::Value;
use std::rc::Rc;

struct ClipboardBus {
    fail: bool,
}
impl BusHandler for ClipboardBus {
    fn send<'a>(
        &'a self,
        target: &'a str,
        command: &'a str,
        args: &'a Value,
    ) -> BusFuture<'a, MixResult<(i32, Value)>> {
        Box::pin(async move {
            assert_eq!(target, "clipd");
            let Value::Map(args) = args else {
                panic!("JSON body required");
            };
            let body: serde_json::Value =
                serde_json::from_str(&args["body"].to_mix_string()).unwrap();
            if self.fail {
                return Ok((10, Value::String("EMPTY".into())));
            }
            match command {
                "clipboard.get" => {
                    let binary = body["mime"] == "application/octet-stream";
                    if !binary { assert_eq!(body, serde_json::json!({"mime":"text/plain"})); }
                    Ok((
                        0,
                        Value::map(IndexMap::from([
                            ("encoding".into(), Value::String(if binary { "base64" } else { "utf8" }.into())),
                            ("eof".into(), Value::Bool(true)),
                            ("data".into(), Value::String(if binary { "AP8=" } else { "clipboard text" }.into())),
                        ])),
                    ))
                }
                "clipboard.set" => {
                    assert_eq!(
                        body,
                        serde_json::json!({"data":"text to copy", "mime":"text/plain;charset=utf-8"})
                    );
                    Ok((0, Value::Bool(true)))
                }
                _ => panic!("unexpected clipboard command"),
            }
        })
    }
    fn emit<'a>(&'a self, _: &'a str, _: &'a str, _: &'a Value) -> BusFuture<'a, MixResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn port_exists<'a>(&'a self, _: &'a str) -> BusFuture<'a, MixResult<bool>> {
        Box::pin(async { Ok(true) })
    }
    fn next_incoming<'a>(&'a self) -> BusFuture<'a, Option<IncomingEvent>> {
        Box::pin(async { None })
    }
}

#[cfg(feature = "crypto")]
#[tokio::test]
async fn clipboard_get_preserves_binary_bytes() {
    let source = "print(base64_encode(clip_get(\"application/octet-stream\")))\n";
    let tokens = mix::lexer::Lexer::new(source).tokenize().unwrap();
    let program = mix::parser::Parser::new(tokens, source).parse_program().unwrap();
    let output = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(output.clone()), Box::new(SharedBuf::new()));
    eval.set_bus_handler(Rc::new(ClipboardBus { fail: false }));
    eval.execute(&program).await.unwrap();
    assert_eq!(output.to_string_lossy(), "AP8=\n");
}

#[tokio::test]
async fn clipboard_builtins_send_to_the_local_service_and_raise_refusals() {
    let source = "print(clip_get(\"text/plain\"))\nprint(clip_set(\"text to copy\"))\n";
    let tokens = mix::lexer::Lexer::new(source).tokenize().unwrap();
    let program = mix::parser::Parser::new(tokens, source)
        .parse_program()
        .unwrap();
    let output = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(output.clone()), Box::new(SharedBuf::new()));
    eval.set_bus_handler(Rc::new(ClipboardBus { fail: false }));
    eval.execute(&program).await.unwrap();
    assert_eq!(output.to_string_lossy(), "clipboard text\ntrue\n");
    eval.set_bus_handler(Rc::new(ClipboardBus { fail: true }));
    assert!(
        eval.execute(&program)
            .await
            .unwrap_err()
            .to_string()
            .contains("rc 10")
    );
}
