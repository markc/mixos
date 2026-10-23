// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real Bus requests exercise the shared application transport adapter.
use bus::native_client::SupervisedClient;
use std::collections::BTreeMap;
use std::time::Duration;

#[test]
fn macro_status_is_served_without_a_ui_dispatch_or_polling() {
    let broker = test_broker::Broker::start();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let caller = runtime
        .block_on(SupervisedClient::connect("macro-caller", &broker.url))
        .unwrap();
    for app in [
        "email",
        "busviewer",
        "monitor",
        "prefs",
        "screenshot",
        "ced",
        "dopus",
        "term",
    ] {
        let (handle, _deliveries) = citizen::start_with_macros(app, app, &broker.url, &[]).unwrap();
        let (rc, body, _) = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(5),
                caller.call_with_headers_raw(
                    app,
                    &format!("{app}.macros.status"),
                    &BTreeMap::new(),
                    "{}",
                ),
            )
            .await
            .unwrap()
            .unwrap()
        });
        assert_eq!(rc, 0, "{app}: {body}");
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["state"], "idle", "{app}");
        handle.quit();
        handle.wait_done().unwrap();
    }
    runtime.block_on(caller.close());
}
