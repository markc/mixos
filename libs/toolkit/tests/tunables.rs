// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Installed values update idle widgets and cached icon resources.

use egui::{Color32, Id, Rect, Ui, vec2};
use egui_kittest::Harness;
use toolkit::{Icon, Theme, canvas, field};

fn gallery(ui: &mut Ui, text: &mut String) {
    ui.add(toolkit::button::IconButton::new(Icon::Settings).tooltip("Settings"));
    field::text(ui, text, "Name", 200.0);
    let mut value = 1.0;
    ui.add(field::ValueField::new(&mut value, 0.0..=100.0));
    toolkit::selectable(ui, true, "Selected row");
    let rect = Rect::from_min_size(ui.next_widget_position(), vec2(260.0, 130.0));
    canvas::surround(ui, rect);
    let mut offset = egui::Vec2::ZERO;
    canvas::scrollbars(ui, Id::new("canvas"), rect, vec2(900.0, 700.0), &mut offset);
    ui.allocate_rect(rect, egui::Sense::hover());
}

fn theme() -> Theme {
    Theme::for_context(design::DesignContext {
        scheme: design::Scheme::Ocean,
        style: Some(design::Style::Plain),
        mode: design::Mode::Dark,
        ..Default::default()
    })
}

#[test]
fn distinct_roles_under_the_pair_renderer() {
    let mut overlay = toolkit::theme::Overlay::default();
    for (role, value) in [
        ("field", [83, 42, 51, 255]),
        ("canvas", [20, 38, 59, 255]),
        ("canvas_dot", [50, 185, 132, 255]),
        ("accent", [210, 129, 65, 255]),
        ("row_selected", [78, 65, 139, 255]),
    ] {
        overlay
            .chrome
            .insert(role.into(), design::family::style::StyleColour(value));
    }
    let mut h = Harness::builder()
        .with_size(vec2(360.0, 300.0))
        .wgpu()
        .build_ui_state(
            |ui, text| {
                gallery(ui, text);
                ui.add(toolkit::text::role("Canvas dots", "canvas_dot"));
                ui.add(toolkit::text::role(
                    "Independent selected row",
                    "row_selected",
                ));
            },
            "Context colours".into(),
        );
    toolkit::install(&h.ctx, &theme().with_overlay(overlay).unwrap());
    let chrome = toolkit::chrome::Chrome::of(&h.ctx);
    assert!(chrome.style.canvas_dots);
    assert_eq!(chrome.style.canvas_dot_size, 1.0);
    assert_eq!(chrome.palette.canvas_dot, Color32::from_rgb(50, 185, 132));
    h.run();
    h.snapshot("distinct_roles_plain");
}

#[test]
fn cached_icon_and_idle_number_repaint_after_a_tune() {
    let mut h = Harness::builder()
        .with_size(vec2(360.0, 300.0))
        .wgpu()
        .build_ui_state(gallery, "Idle".into());
    let theme = theme();
    toolkit::install(&h.ctx, &theme);
    h.run();
    let original_uri = toolkit::icons::image_of(&h.ctx, Icon::Settings, 16.0, Color32::WHITE)
        .uri()
        .unwrap()
        .to_owned();
    h.snapshot("idle_tune_before");
    let mut overlay = toolkit::theme::Overlay::default();
    overlay
        .style
        .insert("fine_span".into(), design::StyleValueSource::Number(100.0));
    overlay.style.insert(
        "icon_mask".into(),
        design::StyleValueSource::Name("#99ccff".into()),
    );
    toolkit::install(&h.ctx, &theme.with_overlay(overlay).unwrap());
    let tuned_uri = toolkit::icons::image_of(&h.ctx, Icon::Settings, 16.0, Color32::WHITE)
        .uri()
        .unwrap()
        .to_owned();
    assert_ne!(original_uri, tuned_uri);
    h.run();
    h.snapshot("idle_tune_after");
}

#[test]
fn motion_and_input_options_are_installed_without_a_settings_wire_change() {
    let mut overlay = toolkit::theme::Overlay {
        reduced_motion: true,
        ..Default::default()
    };
    overlay.style.insert(
        "double_click_time".into(),
        design::StyleValueSource::Number(0.7),
    );
    overlay.style.insert(
        "menu_scroll_speed".into(),
        design::StyleValueSource::Number(1500.0),
    );
    let ctx = egui::Context::default();
    toolkit::install(&ctx, &theme().with_overlay(overlay).unwrap());
    assert_eq!(ctx.global_style().animation_time, 0.0);
    assert_eq!(ctx.global_style().scroll_animation.duration.min, 0.0);
    assert_eq!(ctx.global_style().scroll_animation.duration.max, 0.0);
    assert_eq!(
        ctx.options(|options| options.input_options.max_double_click_delay),
        0.7
    );
    toolkit::install(&ctx, &theme());
    let defaults = egui::style::ScrollAnimation::default();
    assert_eq!(
        ctx.global_style().scroll_animation.duration,
        defaults.duration
    );

    let mut overlay = toolkit::theme::Overlay::default();
    overlay.style.insert(
        "menu_scroll_speed".into(),
        design::StyleValueSource::Number(1500.0),
    );
    toolkit::install(&ctx, &theme().with_overlay(overlay).unwrap());
    assert_eq!(
        ctx.global_style().scroll_animation.points_per_second,
        defaults.points_per_second
    );
    assert_eq!(
        ctx.global_style().scroll_animation.duration,
        defaults.duration
    );
}

#[test]
fn an_invalid_overlay_is_refused_before_the_installed_look_changes() {
    let base = theme();
    let ctx = egui::Context::default();
    toolkit::install(&ctx, &base);
    let before = ctx.global_style();
    let before_chrome = toolkit::chrome::Chrome::of(&ctx);
    for (name, value) in [
        ("canvas_dot_pitch", design::StyleValueSource::Number(0.001)),
        ("scrub_rate", design::StyleValueSource::Number(0.0)),
        ("resize_edge", design::StyleValueSource::Number(13.0)),
        (
            "field_focus_role",
            design::StyleValueSource::Name("missing".into()),
        ),
        ("unknown", design::StyleValueSource::Number(1.0)),
    ] {
        let mut overlay = toolkit::theme::Overlay::default();
        overlay.style.insert(name.into(), value);
        assert!(base.clone().with_overlay(overlay).is_err(), "{name}");
        assert_eq!(ctx.global_style(), before);
        assert_eq!(toolkit::chrome::Chrome::of(&ctx), before_chrome);
    }
    let mut overlay = toolkit::theme::Overlay::default();
    overlay.chrome.insert(
        "unknown".into(),
        design::family::style::StyleColour([255; 4]),
    );
    assert!(base.with_overlay(overlay).is_err());
}

#[test]
fn a_dialog_button_grows_when_its_installed_label_padding_grows() {
    use egui_kittest::kittest::Queryable;
    let button_width = |padding: f64| {
        let mut overlay = toolkit::theme::Overlay::default();
        overlay.style.insert(
            "button_push_padding".into(),
            design::StyleValueSource::Number(padding),
        );
        let mut h = Harness::builder()
            .with_size(vec2(500.0, 240.0))
            .build_ui(|ui| {
                toolkit::dialog::Dialog::new("padding", "Document")
                    .buttons(vec![toolkit::dialog::Choice::new(
                        "Apply document settings",
                        toolkit::dialog::Role::Default,
                    )])
                    .show(ui.ctx(), |ui| {
                        ui.label("Settings");
                    });
            });
        toolkit::install(&h.ctx, &theme().with_overlay(overlay).unwrap());
        h.run();
        h.get_by_label("Apply document settings").rect().width()
    };
    assert!((button_width(60.0) - button_width(28.0) - 32.0).abs() < 0.1);
}

struct CanvasState {
    offset: egui::Vec2,
    thumb: Rect,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            offset: egui::Vec2::ZERO,
            thumb: Rect::NOTHING,
        }
    }
}

#[test]
fn field_focus_width_is_drawn_from_the_installed_style() {
    let mut results = egui_kittest::SnapshotResults::new();
    for style in design::Style::ALL {
        for width in [0.0, 2.0] {
            let mut overlay = toolkit::theme::Overlay::default();
            overlay.style.insert(
                "field_focus_width".into(),
                design::StyleValueSource::Number(width),
            );
            let theme = Theme::for_context(design::DesignContext {
                scheme: design::Scheme::Ocean,
                style: Some(style),
                mode: design::Mode::Light,
                ..Default::default()
            })
            .with_overlay(overlay)
            .unwrap();
            let mut h = Harness::builder()
                .with_size(vec2(300.0, 90.0))
                .wgpu()
                .build_ui_state(
                    |ui, text| {
                        ui.add(field::text_edit(text, "Name", 240.0).id(Id::new("focus-field")));
                    },
                    "Focused field".into(),
                );
            toolkit::install(&h.ctx, &theme);
            h.ctx
                .memory_mut(|memory| memory.request_focus(Id::new("focus-field")));
            h.run();
            results.add(h.try_snapshot(format!("field_focus_{}_{width}", style.name())));
        }
    }
}

#[test]
fn the_canvas_hot_distance_repaints_under_an_idle_pointer() {
    let mut h = Harness::builder()
        .with_size(vec2(300.0, 190.0))
        .wgpu()
        .build_ui_state(
            |ui, state: &mut CanvasState| {
                let viewport = ui.max_rect().shrink(12.0);
                let content = vec2(1000.0, 700.0);
                let inner = canvas::scrollbars(
                    ui,
                    Id::new("idle-bar"),
                    viewport,
                    content,
                    &mut state.offset,
                );
                let inset = toolkit::chrome::Chrome::of(ui.ctx())
                    .style
                    .canvas_thumb_inset as f32;
                let lane = Rect::from_min_max(
                    egui::pos2(inner.left(), inner.bottom()),
                    egui::pos2(inner.right(), viewport.bottom()),
                )
                .shrink(inset);
                let thumb = canvas::thumb(lane.width(), inner.width(), content.x, state.offset.x);
                state.thumb = Rect::from_min_size(
                    egui::pos2(lane.left() + thumb.start, lane.top()),
                    vec2(thumb.length, lane.height()),
                );
            },
            CanvasState::default(),
        );
    let theme = theme();
    toolkit::install(&h.ctx, &theme);
    h.run();
    let thumb = h.state().thumb;
    h.hover_at(egui::pos2(thumb.right() + 4.0, thumb.center().y));
    h.run();
    h.snapshot("canvas_hot_distance_before");
    let mut overlay = toolkit::theme::Overlay::default();
    overlay
        .style
        .insert("hot_distance".into(), design::StyleValueSource::Number(8.0));
    toolkit::install(&h.ctx, &theme.with_overlay(overlay).unwrap());
    h.run();
    h.snapshot("canvas_hot_distance_after");
}
