// SPDX-License-Identifier: MIT OR Apache-2.0
use backdrop::{
    program::Budget,
    views::{Snapshot, Train, host::Host},
};
use egui_kittest::Harness;
use serde_json::json;
use std::{sync::Arc, time::Instant};

#[test]
fn train_legible() {
    let now = Instant::now();
    let mut snapshot = Snapshot {
        now: 6000,
        queue: json!({"main":"0123456789ab", "gold":{"commit":"abcdef012345", "age_minutes":93,"unlanded":7},
            "batched":[{"branch":"work/exporters"},{"branch":"work/mix-global"}],
            "recent":[{"branch":"work/ci-n1-builtins","state":"landed"},{"branch":"work/backdrop-views","state":"landed"},
                {"branch":"work/a-very-long-branch-name-that-needs-truncation","state":"rejected","reason":"conflict","updated":4},
                {"branch":"work/test","state":"rejected","reason":"red: mix-tests","updated":3},
                {"branch":"work/infra","state":"rejected","reason":"infra rc 75","updated":2},
                {"branch":"work/old","state":"rejected","reason":"old","updated":1}]}),
        workers: json!({"slots":[{"worker":"heavy-a"},{"worker":"heavy-b"},{"worker":"light","light":1}], "running":[{"worker":"heavy-a", "started":5580, "spec":"{\"label\":\"train-current-batch\"}"}]}),
        history: json!({"landed":[{"updated":2400},{"updated":5400}],
            "batches":[{"started":4800,"ended":5580,"gate_reused":false}]}),
        ..Default::default()
    };
    snapshot.alert(
        "pipeline.alert",
        &json!({"check":"gate","text":"Gate stalled: inspect worker","at":1}),
    );
    let mut host = Host::new(Train::default(), None);
    host.visibility(true, now);
    host.event(Arc::new(snapshot), true, now);
    let budget = Budget::new(1);
    let mut harness = Harness::builder()
        .with_context_setup(|ctx| toolkit::install(ctx, &toolkit::Theme::embedded()))
        // 3840 × 2160 at 2.5, less the compositor's panel work area.
        .with_size(egui::vec2(1536., 780.))
        .build_ui(move |ui| host.draw(ui, &budget, true, now));
    harness.run();
    harness.snapshot("train_legible");
}
