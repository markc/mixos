// SPDX-License-Identifier: MIT OR Apache-2.0
use mix::{MixResult, value::Value, builtin_info::BuiltinInfo, extensions::BuiltinFamily};
use std::sync::OnceLock;

struct Probe;
static PROBE: Probe = Probe;
impl BuiltinFamily for Probe {
    fn name(&self) -> &'static str { "probe" }
    fn entries(&self) -> Vec<&'static BuiltinInfo> {
        static INFO: OnceLock<BuiltinInfo> = OnceLock::new();
        vec![INFO.get_or_init(|| BuiltinInfo {
            name: "extension_probe", capability: mix::CapabilityClass::Network,
            ..*mix::builtins::core_builtin_info("len").unwrap()
        })]
    }
    fn call(&self, _: &str, args: Vec<Value>) -> MixResult<Option<Value>> { Ok(Some(args[0].clone())) }
}

#[tokio::test]
async fn registered_metadata_dispatch_and_capability_are_visible() {
    mix::extensions::register(&PROBE).unwrap();
    mix::extensions::register(&PROBE).unwrap();
    assert!(mix::builtins::is_builtin("extension_probe"));
    assert_eq!(mix::builtins::capability_category("extension_probe"), mix::CapabilityClass::Network);
    assert_eq!(mix::builtins::builtin_entries().filter(|b| b.name == "extension_probe").count(), 1);
    let (_, out, _) = mix::run_capturing("print(extension_probe(37))\n").await.unwrap();
    assert_eq!(out, "37\n");
}
