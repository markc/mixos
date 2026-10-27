// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Painted graph fixtures also exercise the delayed hover formatter.
use egui::{Rect, Ui, vec2};
use egui_kittest::Harness;
use std::cell::Cell;
use toolkit::graph::{self, Hue, Readout};

struct State {
    rect: Rect,
    calls: Cell<usize>,
}

fn graph(ui: &mut Ui, state: &mut State) {
    let a: Vec<_> = (0..60)
        .map(|i| 50.0 + 35.0 * (i as f64 / 6.0).sin())
        .collect();
    let b: Vec<_> = (0..60)
        .map(|i| 30.0 + 20.0 * (i as f64 / 8.0).cos())
        .collect();
    let format = |series, value, age| {
        state.calls.set(state.calls.get() + 1);
        format!(
            "{}: {value:.1}% · {age} s ago",
            if series == 0 { "Out" } else { "In" }
        )
    };
    state.rect = graph::lines(
        ui,
        vec2(400.0, 120.0),
        &[(&a, Hue::Accent), (&b, Hue::Warning)],
        100.0,
        Some(Readout {
            samples: 60,
            interval_s: 1.0,
            format: &format,
        }),
    )
    .rect;
}

#[test]
fn painted_history_and_hover_readout() {
    let mut h = Harness::builder()
        .with_context_setup(|ctx| toolkit::install(ctx, &toolkit::Theme::embedded()))
        .with_size(vec2(480.0, 240.0))
        .wgpu()
        .build_ui_state(
            graph,
            State {
                rect: Rect::NOTHING,
                calls: Cell::new(0),
            },
        );
    h.run();
    assert_eq!(h.state().calls.get(), 0, "no readout work without hover");
    h.snapshot("graph_painted");
    let rect = h.state().rect.shrink(1.0);
    h.hover_at(egui::pos2(
        rect.right() - rect.width() * 12.0 / 59.0,
        rect.center().y,
    ));
    h.run_steps(40);
    assert!(h.state().calls.get() >= 2);
    h.snapshot("graph_hover");
}
