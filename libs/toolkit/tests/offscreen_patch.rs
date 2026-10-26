// SPDX-License-Identifier: MIT OR Apache-2.0
//! Re-vendoring must not silently remove the native hidden-window capture path.

#[test]
fn hidden_window_vendor_patch_guard() {
    let run = include_str!("../../../vendor/eframe/src/native/run.rs");
    let integration = include_str!("../../../vendor/eframe/src/native/wgpu_integration.rs");
    let modes = include_str!("../../../vendor/eframe/src/native/winit_integration.rs");
    let painter = include_str!("../../../vendor/egui-wgpu/src/winit.rs");
    assert!(run.contains("self.winit_app.overdue_pass_mode(window_id)"));
    assert!(run.contains("*repaint_time = (*repaint_time).max(*floor)"));
    assert!(run.contains(".insert(window_id, Instant::now() + MISSED_REDRAW_TIMEOUT)"));
    assert!(run.contains("self.windows_direct_repaint_floors.remove(&window_id)"));
    assert!(
        modes.contains("PassMode::LogicOnly"),
        "glow keeps logic-only fallback"
    );
    assert!(integration.contains("any(ActionRequested::wants_paint)"));
    assert!(integration.contains("paint_offscreen_and_update_textures("));
    assert!(
        painter.contains("&& !offscreen"),
        "offscreen bypasses surface acquire"
    );
    assert!(painter.contains("let capture = offscreen || !capture_data.is_empty()"));
    assert!(painter.contains("Failed to poll offscreen screenshot"));
    assert!(painter.contains("tests/desktop/offscreen_gpu.rs"));
    assert!(
        painter.contains("if !capture_data.is_empty()\n                && let Some(capture_state)")
    );
    assert!(painter.contains("sync_channel::<wgpu::Device>(8)"));
}
