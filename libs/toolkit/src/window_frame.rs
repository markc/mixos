// SPDX-License-Identifier: MIT OR Apache-2.0
//! One shared frame request per native viewport, consumed after egui paints.
use egui::{Context, Id, Plugin, Ui, ViewportCommand};

pub(crate) fn install(ctx: &Context) {
    ctx.add_plugin(WindowFrame);
}

pub(crate) fn rounded(ctx: &Context) -> bool {
    crate::chrome::Chrome::of(ctx).decorations == design::Decorations::Client
        && ctx.input(|i| {
            !i.viewport().maximized.unwrap_or(false) && !i.viewport().fullscreen.unwrap_or(false)
        })
    // egui 0.37 / winit 0.30 expose no tiled-edge state. Maximised is the
    // supported square-corner fallback; do not guess tiling from window size.
}

#[cfg(any(test, all(target_os = "linux", feature = "shadow")))]
pub(crate) fn radius(ctx: &Context, frame: design::frame::Frame) -> f32 {
    let key = Id::unique(("egui-wgpu.transparent", ctx.viewport_id()));
    if rounded(ctx) && ctx.data(|d| d.get_temp::<bool>(key)) == Some(true) {
        frame.radius
    } else {
        0.0
    }
}

struct WindowFrame;
impl Plugin for WindowFrame {
    fn debug_name(&self) -> &'static str {
        "Native window frame"
    }
    fn on_end_pass(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx();
        let viewport = ctx.viewport_id();
        let init = Id::unique(("toolkit.transparent", viewport));
        if !ctx.data(|d| d.get_temp::<bool>(init)).unwrap_or(false) {
            ctx.send_viewport_cmd(ViewportCommand::Transparent(true));
            ctx.data_mut(|d| d.insert_temp(init, true));
        }
        let chrome = crate::chrome::Chrome::of(ctx);
        let frame = chrome.frame;
        let [r, g, b, _] = chrome.palette.chrome.to_srgba_unmultiplied();
        let fill = egui::Color32::from_rgb(r, g, b).to_normalized_gamma_f32();
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(i.focused));
        let scale = ctx.input(|i| i.viewport().native_pixels_per_point.unwrap_or(1.0));
        let active = rounded(ctx);
        let client = chrome.decorations == design::Decorations::Client
            && !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        let key = Id::unique(("egui-wgpu.window-frame", viewport));
        let request = if client {
            (
                if active { frame.radius * scale } else { 0.0 },
                if frame.border_width > 0.0 {
                    (frame.border_width * scale).round().max(1.0)
                } else {
                    0.0
                },
                if focused {
                    frame.border_focused
                } else {
                    frame.border_unfocused
                },
                fill,
            )
        } else {
            (0.0_f32, 0.0_f32, [0.0_f32; 4], fill)
        };
        ctx.data_mut(|d| d.insert_temp(key, request));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_requests_follow_state_and_alpha_support_without_a_compositor() {
        let ctx = Context::default();
        crate::install(
            &ctx,
            &crate::Theme::embedded()
                .framed(design::Decorations::Client, design::CaptionSide::Auto),
        );
        install(&ctx);
        let alpha = Id::unique(("egui-wgpu.transparent", egui::ViewportId::ROOT));
        let request = Id::unique(("egui-wgpu.window-frame", egui::ViewportId::ROOT));
        let frame = crate::chrome::Chrome::of(&ctx).frame;
        assert_eq!(radius(&ctx, frame), 0.0);
        ctx.data_mut(|d| d.insert_temp(alpha, true));
        for (maximized, fullscreen, expected) in [
            (false, false, true),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let mut input = egui::RawInput::default();
            let root = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
            root.maximized = Some(maximized);
            root.fullscreen = Some(fullscreen);
            let mut output = ctx.run_ui(input, |_| {});
            output.textures_delta.clear();
            let value = ctx
                .data_mut(|d| d.remove_temp::<(f32, f32, [f32; 4], [f32; 4])>(request))
                .unwrap();
            assert_eq!(value.0 > 0.0, expected);
            assert_eq!(radius(&ctx, frame) > 0.0, expected);
            let [r, g, b, _] = crate::chrome::Chrome::of(&ctx).palette.chrome.to_srgba_unmultiplied();
            assert_eq!(value.3, egui::Color32::from_rgb(r, g, b).to_normalized_gamma_f32());
            assert_eq!(value.3[3], 1.0);
        }
    }
}
