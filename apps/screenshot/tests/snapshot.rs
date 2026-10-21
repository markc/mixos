// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Offscreen snapshots of screenshot driven through its real app state and
//! view: an annotated shot with every kind of object and a selection; the
//! empty window; the Text tool with window capture chosen; the Discard and
//! Save As dialogs. The picture is the painter's own render, as in the
//! window. Regenerate with `UPDATE_SNAPSHOTS=1` and look at the images
//! before committing them.
use annotation::{Kind, Point, Shape};
use design::{DesignContext, Mode, Scheme};
use egui_kittest::Harness;
use image::{Rgba, RgbaImage};
use screenshot::app::{App, Dialog, Tool};
use screenshot::capture::{self, Target, Window};
use screenshot::view::{Drag, Preview, view};
use screenshot::{commands, strings};
use toolkit::Theme;

/// A made-up desktop: a backdrop, a window with a title bar and a sidebar,
/// and lines of "text" (fixture pixels, not UI colours).
fn desktop() -> RgbaImage {
    let mut image = RgbaImage::from_pixel(1280, 800, Rgba([46, 62, 84, 255]));
    let mut fill = |x0: u32, y0: u32, w: u32, h: u32, c: [u8; 4]| {
        for y in y0..(y0 + h).min(800) {
            for x in x0..(x0 + w).min(1280) {
                image.put_pixel(x, y, Rgba(c));
            }
        }
    };
    fill(120, 90, 1000, 620, [244, 244, 240, 255]);
    fill(120, 90, 1000, 36, [214, 216, 222, 255]);
    fill(120, 126, 220, 584, [228, 230, 234, 255]);
    for row in 0..12 {
        let y = 170 + row * 38;
        fill(380, y, 560 - (row % 4) * 90, 12, [120, 126, 134, 255]);
        fill(150, y, 150, 10, [168, 172, 180, 255]);
    }
    fill(980, 600, 110, 34, [52, 120, 220, 255]);
    image
}

fn shape(kind: Kind, a: (f32, f32), b: (f32, f32), colour: [u8; 4], width: f32) -> Shape {
    Shape::new(
        kind,
        vec![Point { x: a.0, y: a.1 }, Point { x: b.0, y: b.1 }],
        colour,
        width,
    )
}

/// An app holding the fixture with one object of every kind.
fn annotated() -> App {
    let mut app = App::new(screenshot::label, screenshot::ink::inks(), false);
    app.document = Some(annotation::Document::new(desktop()).unwrap());
    let inks = app.inks;
    let ink = |i: usize| inks[i].rgba;
    let (red, amber, green, blue) = (ink(0), ink(1), ink(2), ink(3));
    app.add_shape(shape(
        Kind::Rectangle,
        (370.0, 160.0),
        (960.0, 260.0),
        red,
        4.0,
    ));
    app.add_shape(shape(
        Kind::Arrow,
        (1130.0, 420.0),
        (960.0, 300.0),
        red,
        6.0,
    ));
    app.add_shape(shape(
        Kind::Ellipse,
        (960.0, 585.0),
        (1110.0, 650.0),
        green,
        4.0,
    ));
    let mut pen = shape(Kind::Pen, (400.0, 560.0), (420.0, 575.0), blue, 4.0);
    pen.points = (0..40)
        .map(|i| Point {
            x: 400.0 + i as f32 * 8.0,
            y: 560.0 + (i as f32 * 0.6).sin() * 14.0,
        })
        .collect();
    app.add_shape(pen);
    let mut marker = shape(
        Kind::Highlighter,
        (380.0, 360.0),
        (900.0, 360.0),
        amber,
        22.0,
    );
    marker.colour[3] = 255;
    app.add_shape(marker);
    app.add_shape(shape(
        Kind::Redact,
        (150.0, 390.0),
        (300.0, 440.0),
        ink(4),
        2.0,
    ));
    app.set_text("Check this value".into());
    app.set_text_size(32.0);
    app.add_shape(shape(Kind::Text, (380.0, 640.0), (900.0, 690.0), red, 2.0));
    for at in [(390.0, 110.0), (980.0, 540.0)] {
        app.add_shape(shape(
            Kind::Number,
            (at.0 - 22.0, at.1 - 22.0),
            (at.0 + 22.0, at.1 + 22.0),
            blue,
            2.0,
        ));
    }
    // The arrow stays selected.
    let arrow = app.document.as_ref().unwrap().objects()[1].id;
    app.set_tool(Tool::Select);
    app.select(Some(arrow));
    app.status = screenshot::label("capture-complete");
    app
}

fn window(theme: Theme, name: &str, app: App) {
    window_sized(theme, name, app, egui::vec2(1100.0, 720.0));
}

fn window_sized(theme: Theme, name: &str, app: App, size: egui::Vec2) {
    let registry = commands::registry();
    let strings = strings();
    let rendered = app
        .document
        .as_ref()
        .map(|d| d.render(&screenshot::face()).unwrap());
    let mut preview: Option<Preview> = None;
    let mut drag = Drag::default();
    let mut harness = Harness::builder()
        .with_size(size)
        .wgpu()
        .build_ui(move |ui| {
            if preview.is_none()
                && let Some(image) = &rendered
            {
                let size = [image.width() as usize, image.height() as usize];
                let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image);
                preview = Some(Preview {
                    texture: ui.ctx().load_texture(
                        "screenshot-preview",
                        pixels,
                        egui::TextureOptions::LINEAR,
                    ),
                    size,
                    document: app.document_id,
                });
            }
            let _ = view(ui, &app, &registry, &strings, preview.as_ref(), &mut drag);
        });
    toolkit::install(&harness.ctx, &theme);
    harness.run();
    harness.snapshot(name);
}

fn chrome(scheme: Scheme, mode: Mode) -> Theme {
    Theme::for_context(DesignContext {
        scheme,
        mode,
        ..DesignContext::default()
    })
}

/// The default look with every kind of annotation and the arrow selected.
#[test]
fn annotated_studio_dark() {
    window(
        chrome(Scheme::Studio, Mode::Dark),
        "annotated_studio_dark",
        annotated(),
    );
}

#[test]
fn annotated_pro_light() {
    window(
        chrome(Scheme::Pro, Mode::Light),
        "annotated_pro_light",
        annotated(),
    );
}

/// Nothing taken yet.
#[test]
fn empty_studio_light() {
    window(
        chrome(Scheme::Studio, Mode::Light),
        "empty_studio_light",
        App::new(screenshot::label, screenshot::ink::inks(), false),
    );
}

/// The Text tool's controls, with window capture and a window chosen.
#[test]
fn text_tool_window_mode_ocean_dark() {
    let mut app = annotated();
    app.set_tool(Tool::Draw(Kind::Text));
    app.set_mode(capture::Mode::Window);
    let listed = |id, title: &str| Window {
        target: Target { id, generation: 1 },
        title: title.into(),
        app_id: String::new(),
        focused: id == 2,
        minimized: false,
    };
    app.refreshed(Ok((
        vec![listed(1, "Notes — ced"), listed(2, "Files — DOpus")],
        vec!["DP-1".into()],
    )));
    window(
        chrome(Scheme::Ocean, Mode::Dark),
        "text_tool_window_mode_ocean_dark",
        app,
    );
}

/// The Text tool at the window's smallest size (760 points) with long
/// text: Copy and Save keep their place and the controls never run under
/// them.
#[test]
fn text_tool_narrow_studio_dark() {
    let mut app = annotated();
    app.set_tool(Tool::Draw(Kind::Text));
    app.set_text("A long note that is much wider than the field can show at once".into());
    window_sized(
        chrome(Scheme::Studio, Mode::Dark),
        "text_tool_narrow_studio_dark",
        app,
        egui::vec2(760.0, 450.0),
    );
}

/// The Number tool at about 810 points, where the text size list ("Medium
/// text", wider than the list's minimum width) only just fits or is left
/// out whole, never cut at Copy.
#[test]
fn number_tool_narrow_studio_dark() {
    let mut app = annotated();
    app.set_tool(Tool::Draw(Kind::Number));
    window_sized(
        chrome(Scheme::Studio, Mode::Dark),
        "number_tool_narrow_studio_dark",
        app,
        egui::vec2(810.0, 450.0),
    );
}

/// Taking another shot over unsaved annotations asks first.
#[test]
fn discard_dialog_studio_light() {
    let mut app = annotated();
    app.take(Some(capture::Mode::Screen));
    assert_eq!(app.ui.dialog, Some(Dialog::Discard));
    window(
        chrome(Scheme::Studio, Mode::Light),
        "discard_dialog_studio_light",
        app,
    );
}

/// Save As, at a fixed path so the picture does not depend on the machine.
#[test]
fn save_as_classic() {
    let mut app = annotated();
    app.save_as();
    assert_eq!(app.ui.dialog, Some(Dialog::SaveAs));
    app.set_path("/home/user/Pictures/Screenshots/20261012_140322.png".into());
    window(chrome(Scheme::Classic, Mode::Light), "save_as_classic", app);
}
