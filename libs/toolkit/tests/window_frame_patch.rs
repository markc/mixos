// SPDX-License-Identifier: MIT OR Apache-2.0
#[test]
fn rounded_frame_patch_and_all_native_transparent_entry_points_survive_refresh() {
    let painter = include_str!("../../../vendor/egui-wgpu/src/winit.rs");
    let pass = include_str!("../../../vendor/egui-wgpu/src/window_frame.rs");
    let shader = include_str!("../../../vendor/egui-wgpu/src/window_frame.wgsl");
    assert!(painter.contains("remove_temp::<(f32, f32, [f32; 4])>(frame_key)"));
    assert!(painter.contains("egui-wgpu.transparent"));
    assert!(painter.contains("let clear_color = if frame.is_some()"));
    assert!(painter.contains("CompositeAlphaMode::PreMultiplied"));
    assert!(pass.contains("src_factor: wgpu::BlendFactor::Zero"));
    assert!(pass.contains("dst_factor: wgpu::BlendFactor::SrcAlpha"));
    assert!(pass.contains("pass.set_scissor_rect"));
    assert!(shader.contains("clamp(0.5 - distance(p.xy), 0.0, 1.0)"));
    assert!(include_str!("../src/titlebar.rs").contains(".with_transparent(true)"));
    for main in [
        include_str!("../../../apps/ced/src/main.rs"),
        include_str!("../../../apps/dopus/src/main.rs"),
        include_str!("../../../apps/email/src/lib.rs"),
        include_str!("../../../apps/monitor/src/main.rs"),
        include_str!("../../../apps/prefs/src/main.rs"),
        include_str!("../../../apps/screenshot/src/main.rs"),
        include_str!("../../../apps/term/src/main.rs"),
        include_str!("../../../apps/workshop/src/main.rs"),
    ] {
        assert!(main.contains("toolkit::titlebar::viewport("));
        assert!(main.contains("toolkit::decorations::initialise_window("));
    }
    assert!(
        include_str!("../../../apps/dopus/src/choose/presenter.rs")
            .contains(".with_transparent(true)")
    );
}
