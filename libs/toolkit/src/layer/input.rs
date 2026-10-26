// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Wayland input as egui sees it: buttons, keys, modifiers, wheel deltas,
//! and egui's cursor as a `wp_cursor_shape_v1` shape.

use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use smithay_client_toolkit::seat::keyboard::Modifiers;

pub(crate) fn wheel_source(
    source: Option<wayland_client::protocol::wl_pointer::AxisSource>,
) -> egui::MouseWheelSource {
    use wayland_client::protocol::wl_pointer::AxisSource;
    match source {
        Some(AxisSource::Finger) => egui::MouseWheelSource::Trackpad,
        Some(AxisSource::Wheel | AxisSource::WheelTilt) => egui::MouseWheelSource::Wheel,
        _ => egui::MouseWheelSource::Unknown,
    }
}

/// Linux input button codes (`BTN_LEFT` and on) as egui buttons.
pub(crate) fn button(code: u32) -> Option<egui::PointerButton> {
    match code {
        0x110 => Some(egui::PointerButton::Primary),
        0x111 => Some(egui::PointerButton::Secondary),
        0x112 => Some(egui::PointerButton::Middle),
        0x113 => Some(egui::PointerButton::Extra1),
        0x114 => Some(egui::PointerButton::Extra2),
        _ => None,
    }
}

/// The keyboard's modifiers as egui's (Ctrl is the command key here).
pub(crate) fn modifiers(m: &Modifiers) -> egui::Modifiers {
    egui::Modifiers {
        alt: m.alt,
        ctrl: m.ctrl,
        shift: m.shift,
        mac_cmd: false,
        command: m.ctrl,
    }
}

/// A keysym as egui's logical key: named keys by keysym, Latin letters,
/// digits and punctuation by keysym too (so Ctrl+A is A whatever it types,
/// and a release, which carries no text, still names its key), anything
/// else by the text it typed.
pub(crate) fn key(keysym: u32, text: Option<&str>) -> Option<egui::Key> {
    use egui::Key;
    const XK_F1: u32 = 0xffbe;
    const XK_F12: u32 = 0xffc9;
    let named = match keysym {
        0xff0d | 0xff8d => Key::Enter,
        0xff1b => Key::Escape,
        0xff08 => Key::Backspace,
        0xff09 | 0xfe20 => Key::Tab,
        0xffff => Key::Delete,
        0xff63 => Key::Insert,
        0xff50 => Key::Home,
        0xff57 => Key::End,
        0xff55 => Key::PageUp,
        0xff56 => Key::PageDown,
        0xff52 => Key::ArrowUp,
        0xff54 => Key::ArrowDown,
        0xff51 => Key::ArrowLeft,
        0xff53 => Key::ArrowRight,
        0x0020 => Key::Space,
        n if (XK_F1..=XK_F12).contains(&n) => {
            const F: [Key; 12] = [
                Key::F1,
                Key::F2,
                Key::F3,
                Key::F4,
                Key::F5,
                Key::F6,
                Key::F7,
                Key::F8,
                Key::F9,
                Key::F10,
                Key::F11,
                Key::F12,
            ];
            F[(n - XK_F1) as usize]
        }
        n @ 0x0061..=0x007a => return Key::from_name(&char::from(n as u8 - 32).to_string()),
        n @ 0x0021..=0x007e => return Key::from_name(&char::from(n as u8).to_string()),
        _ => return text.and_then(Key::from_name),
    };
    Some(named)
}

/// Text a key press types, unless a command chord is held or it is a
/// control character (Enter, Tab and Backspace arrive as keys).
pub(crate) fn typed(text: Option<&str>, m: &Modifiers) -> Option<String> {
    let text = text?;
    if m.ctrl || m.alt || m.logo || text.is_empty() || text.chars().any(char::is_control) {
        return None;
    }
    Some(text.to_owned())
}

/// A Wayland axis value (positive down and right) as egui's wheel delta
/// (positive up and left).
pub(crate) fn wheel(horizontal: f64, vertical: f64) -> egui::Vec2 {
    egui::vec2(-horizontal as f32, -vertical as f32)
}

/// egui's cursor as a cursor-shape protocol shape.
pub(crate) fn shape(icon: egui::CursorIcon) -> Shape {
    use egui::CursorIcon as C;
    match icon {
        C::Default => Shape::Default,
        C::None => Shape::Default,
        C::ContextMenu => Shape::ContextMenu,
        C::Help => Shape::Help,
        C::PointingHand => Shape::Pointer,
        C::Progress => Shape::Progress,
        C::Wait => Shape::Wait,
        C::Cell => Shape::Cell,
        C::Crosshair => Shape::Crosshair,
        C::Text => Shape::Text,
        C::VerticalText => Shape::VerticalText,
        C::Alias => Shape::Alias,
        C::Copy => Shape::Copy,
        C::Move => Shape::Move,
        C::NoDrop => Shape::NoDrop,
        C::NotAllowed => Shape::NotAllowed,
        C::Grab => Shape::Grab,
        C::Grabbing => Shape::Grabbing,
        C::AllScroll => Shape::AllScroll,
        C::ResizeHorizontal => Shape::EwResize,
        C::ResizeNeSw => Shape::NeswResize,
        C::ResizeNwSe => Shape::NwseResize,
        C::ResizeVertical => Shape::NsResize,
        C::ResizeEast => Shape::EResize,
        C::ResizeSouthEast => Shape::SeResize,
        C::ResizeSouth => Shape::SResize,
        C::ResizeSouthWest => Shape::SwResize,
        C::ResizeWest => Shape::WResize,
        C::ResizeNorthWest => Shape::NwResize,
        C::ResizeNorth => Shape::NResize,
        C::ResizeNorthEast => Shape::NeResize,
        C::ResizeColumn => Shape::ColResize,
        C::ResizeRow => Shape::RowResize,
        C::ZoomIn => Shape::ZoomIn,
        C::ZoomOut => Shape::ZoomOut,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wayland_axis_sources_keep_fingers_distinct_from_wheels() {
        use wayland_client::protocol::wl_pointer::AxisSource;
        for (source, expected) in [
            (Some(AxisSource::Finger), egui::MouseWheelSource::Trackpad),
            (Some(AxisSource::Wheel), egui::MouseWheelSource::Wheel),
            (Some(AxisSource::WheelTilt), egui::MouseWheelSource::Wheel),
            (
                Some(AxisSource::Continuous),
                egui::MouseWheelSource::Unknown,
            ),
            (None, egui::MouseWheelSource::Unknown),
        ] {
            assert_eq!(super::wheel_source(source), expected);
        }
    }
    #[test]
    fn move_only_wheel_sources_have_identical_smoothing_and_zoom() {
        fn sample(
            source: egui::MouseWheelSource,
            unit: egui::MouseWheelUnit,
            modifiers: egui::Modifiers,
        ) -> Vec<(egui::Vec2, f32, bool)> {
            let ctx = egui::Context::default();
            let mut samples = Vec::new();
            for frame in 0..40 {
                let events = if frame == 0 {
                    vec![egui::Event::MouseWheel {
                        source,
                        unit,
                        modifiers,
                        phase: egui::TouchPhase::Move,
                        delta: egui::vec2(0.0, -12.0),
                    }]
                } else {
                    Vec::new()
                };
                ctx.run_ui(
                    egui::RawInput {
                        time: Some(f64::from(frame) / 60.0),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        samples.push(ui.input(|i| {
                            (i.smooth_scroll_delta(), i.zoom_delta(), i.is_scrolling())
                        }));
                    },
                )
                .drop_without_applying_deltas();
            }
            samples
        }
        for unit in [egui::MouseWheelUnit::Line, egui::MouseWheelUnit::Point] {
            for modifiers in [egui::Modifiers::NONE, egui::Modifiers::COMMAND] {
                let unknown = sample(egui::MouseWheelSource::Unknown, unit, modifiers);
                assert!(
                    unknown
                        .iter()
                        .any(|(delta, zoom, _)| *delta != egui::Vec2::ZERO || *zoom != 1.0)
                );
                for source in [
                    egui::MouseWheelSource::Wheel,
                    egui::MouseWheelSource::Trackpad,
                ] {
                    assert_eq!(unknown, sample(source, unit, modifiers));
                }
            }
        }
    }
    use super::*;

    #[test]
    fn keys_by_keysym_and_text() {
        assert_eq!(key(0xff0d, None), Some(egui::Key::Enter));
        assert_eq!(key(0x0061, Some("a")), Some(egui::Key::A));
        // Ctrl+A types nothing, and is still A.
        assert_eq!(key(0x0061, None), Some(egui::Key::A));
        assert_eq!(key(0x0031, Some("1")), Some(egui::Key::Num1));
        assert_eq!(key(0xffbe, None), Some(egui::Key::F1));
        assert_eq!(key(0xffc9, None), Some(egui::Key::F12));
        assert_eq!(key(0xff52, None), Some(egui::Key::ArrowUp));
        assert_eq!(key(0x1234, None), None);
    }

    #[test]
    fn text_is_typed_only_without_a_chord() {
        let plain = Modifiers::default();
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(typed(Some("é"), &plain).as_deref(), Some("é"));
        assert_eq!(typed(Some("a"), &ctrl), None);
        assert_eq!(typed(Some("\r"), &plain), None);
        assert_eq!(typed(None, &plain), None);
    }

    #[test]
    fn buttons_wheel_and_cursor() {
        assert_eq!(button(0x110), Some(egui::PointerButton::Primary));
        assert_eq!(button(0x111), Some(egui::PointerButton::Secondary));
        assert_eq!(button(0x999), None);
        assert_eq!(wheel(0.0, 10.0), egui::vec2(0.0, -10.0));
        assert_eq!(shape(egui::CursorIcon::PointingHand), Shape::Pointer);
        assert_eq!(shape(egui::CursorIcon::Text), Shape::Text);
        assert_eq!(
            modifiers(&Modifiers {
                ctrl: true,
                ..Modifiers::default()
            }),
            egui::Modifiers {
                ctrl: true,
                command: true,
                ..egui::Modifiers::default()
            }
        );
    }
}
