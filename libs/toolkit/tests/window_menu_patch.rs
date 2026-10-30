// SPDX-License-Identifier: MIT OR Apache-2.0
//! Refresh guard for the viewport-scoped Wayland window-menu bridge.
#[test]
fn window_menu_bridge_survives_vendor_refresh() {
    let bridge = include_str!("../../../vendor/eframe/src/native/winit_integration.rs");
    assert!(bridge.contains("(\"eframe.window-menu\", viewport)"));
    assert!(bridge.contains("remove_temp::<egui::Pos2>(key)"));
    assert!(bridge.contains("RawDisplayHandle::Wayland"));
    assert!(bridge.contains("window.scale_factor() as f32 * ctx.zoom_factor()"));
    assert!(bridge.contains("window.show_window_menu(winit::dpi::PhysicalPosition::new("));
    for renderer in [
        include_str!("../../../vendor/eframe/src/native/wgpu_integration.rs"),
        include_str!("../../../vendor/eframe/src/native/glow_integration.rs"),
    ] {
        assert!(renderer.contains("viewport_id, &window)"));
        assert!(renderer.contains("show_window_menu(&egui_ctx, ids.this, window)"));
    }
}
