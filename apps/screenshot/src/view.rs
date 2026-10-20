// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The egui rendering of the app's state. [`view`] reads the app and
//! returns what the person did as [`UiEvent`]s; the shell applies them
//! after the frame, so drawing never changes state and a snapshot needs no
//! Bus.
//!
//! The window: the title bar with the menus; the options bar (Take, the
//! capture mode, the window to capture, then the ink, width and text
//! controls, and Save and Copy at its right end); the tool bar of annotation tools at the left; the picture on
//! the canvas surround; the status bar. The picture is the painter's own
//! render (what is saved is what is shown); only a gesture in progress and
//! the selection are drawn over it here.
use crate::app::{Answer, App, Dialog, SIZES, Tool, WIDTHS};
use crate::capture::{Mode, Target};
use crate::commands::{ink_id, mode_id, size_id, tool_id, width_id};
use crate::{label, label_with};
use annotation::{Crop, Kind, Point, Shape, Viewport};
use egui::{
    Align, Color32, Id, Layout, PointerButton, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2,
};
use toolkit::button::{IconButton, PushButton};
use toolkit::command::Origin;
use toolkit::dialog::{self, Choice, Role};
use toolkit::titlebar::{self, Control};
use toolkit::{Icon, Strings, bars, field, tooltip};
use crate::app::Commands;

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Command(&'static str, Origin),
    Add(Shape),
    Crop(Crop),
    Move(u64, f32, f32),
    Select(Option<u64>),
    Pan(f32, f32),
    Zoom(f32),
    /// The picture's view scale (view points per image pixel) changed.
    Scale(f32),
    Text(String),
    Path(String),
    Window(Option<Target>),
    Output(Option<String>),
    /// Read the windows and outputs again.
    Refresh,
    Answer(Answer),
    SubmitPath,
    CloseDialog,
}

/// The picture as last rendered: a texture of the document's export.
pub struct Preview {
    pub texture: egui::TextureHandle,
    /// The document it was rendered from ([`App::document_id`]): a render
    /// of another document is never shown.
    pub document: u64,
    /// The render's size (the document's output size when it was made).
    pub size: [usize; 2],
}

/// A gesture in progress on the picture, in original-image pixels.
#[derive(Default, Debug)]
pub struct Drag {
    points: Vec<Point>,
    /// The object being moved (Select tool).
    moving: Option<u64>,
}

/// How near (in view points) a press must be to an object to pick it.
const PICK: f32 = 6.0;
/// The swatch side in the options bar.
const SWATCH: f32 = 18.0;
/// The Text tool's field: its full width, and the least it shrinks to.
const TEXT_MAX: f32 = 220.0;
const TEXT_MIN: f32 = 80.0;
/// The width and text size lists' width.
const COMBO: f32 = 96.0;
/// The space after the swatches.
const INKS_GAP: f32 = 6.0;

/// The window title.
pub fn title(app: &App) -> String {
    match app.saved.as_ref().map(|s| &s.path).or(app.source.as_ref()) {
        Some(path) => label_with(
            "title-with",
            &[(
                "name",
                &path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )],
        ),
        None => label("title"),
    }
}

/// Draw the whole window into `ui`.
pub fn view(
    ui: &mut Ui,
    app: &App,
    commands: &Commands,
    strings: &Strings,
    stroke: f32,
    preview: Option<&Preview>,
    drag: &mut Drag,
) -> Vec<UiEvent> {
    let mut events = Vec::new();
    let mut controls = [
        Control::Icon {
            command: "capture.take",
            icon: Icon::Camera,
            selected: false,
        },
        Control::Link {
            command: "help.shortcuts",
            icon: Icon::Keyboard,
        },
    ];
    let fired = titlebar::show_with(
        ui,
        &title(app),
        Some(Icon::Camera),
        stroke,
        commands,
        app,
        strings,
        &mut controls,
    );
    events.extend(fired.into_iter().map(|(id, o)| UiEvent::Command(id, o)));
    bars::options(ui, |ui| options(ui, app, commands, strings, &mut events));
    bars::status(ui, |ui| status(ui, app));
    bars::tools(ui, |ui| tools(ui, app, commands, strings, &mut events));
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| picture(ui, app, preview, drag, &mut events));
    dialogs(ui.ctx(), app, commands, strings, &mut events);
    titlebar::edges(ui);
    events
}

/// A command's button: `push` for a push button, else an icon button.
fn command_button(
    ui: &mut Ui,
    app: &App,
    commands: &Commands,
    strings: &Strings,
    id: &'static str,
    selected: bool,
    events: &mut Vec<UiEvent>,
) {
    let Some(command) = commands.registry().get(id) else {
        return;
    };
    let tip = tooltip::for_command(ui.ctx(), command, strings);
    let button = IconButton::new(commands.icon(id).unwrap_or(Icon::Square))
        .selected(selected)
        .tooltip(tip);
    if ui.add_enabled(commands.registry().enabled(id, app).is_ok(), button).clicked() {
        events.push(UiEvent::Command(id, Origin::Ui));
    }
}

/// The options bar's controls, left to right.
fn controls(
    ui: &mut Ui,
    app: &App,
    commands: &Commands,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    if let Some(take) = commands.registry().get("capture.take") {
        let button = PushButton::primary(label("take"));
        let response = ui
            .add_enabled(commands.registry().enabled("capture.take", app).is_ok(), button)
            .on_hover_text(tooltip::for_command(ui.ctx(), take, strings));
        if response.clicked() {
            events.push(UiEvent::Command("capture.take", Origin::Ui));
        }
    }
    for (mode, icon) in [
        (Mode::Region, Icon::SquareDashed),
        (Mode::Screen, Icon::Monitor),
        (Mode::Window, Icon::AppWindow),
    ] {
        let id = mode_id(mode);
        let enabled = commands.registry().enabled(id, app).is_ok() && !app.busy();
        let tip = commands.registry()
            .get(id)
            .map(|c| toolkit::command::label_text(strings, c.label))
            .unwrap_or_default();
        let button = IconButton::new(icon)
            .selected(app.ui.mode == mode)
            .tooltip(tip);
        if ui.add_enabled(enabled, button).clicked() {
            events.push(UiEvent::Command(id, Origin::Ui));
        }
    }
    if app.ui.mode == Mode::Window {
        let names: Vec<String> = app.windows.iter().map(|w| w.name().to_owned()).collect();
        if names.is_empty() {
            ui.label(RichText::new(label("no-windows")).weak());
        } else {
            let mut at = app
                .ui
                .window
                .and_then(|t| app.windows.iter().position(|w| w.target == t))
                .unwrap_or(0);
            let response = ui
                .add_enabled_ui(!app.busy(), |ui| {
                    toolkit::combo::show(ui, "screenshot-window", &mut at, &names, Some(180.0))
                })
                .inner;
            if response.changed() {
                events.push(UiEvent::Window(app.windows.get(at).map(|w| w.target)));
            }
            // Opening the list reads the windows again.
            if response.clicked() {
                events.push(UiEvent::Refresh);
            }
        }
    } else if app.outputs.len() > 1 {
        let mut names = vec![label("output-pointer")];
        names.extend(app.outputs.iter().cloned());
        let mut at = app
            .ui
            .output
            .as_ref()
            .and_then(|o| app.outputs.iter().position(|n| n == o))
            .map_or(0, |i| i + 1);
        let response = ui
            .add_enabled_ui(!app.busy(), |ui| {
                toolkit::combo::show(ui, "screenshot-output", &mut at, &names, Some(180.0))
            })
            .inner
            .on_hover_text(label("output"));
        if response.changed() {
            let output = at.checked_sub(1).and_then(|i| app.outputs.get(i).cloned());
            events.push(UiEvent::Output(output));
        }
        if response.clicked() {
            events.push(UiEvent::Refresh);
        }
    }
    if app.document.is_none() {
        return;
    }
    bars::divider(ui, bars::DIVIDER_HEIGHT);
    let editable = !app.modal();
    // In a narrow window a group that does not fit whole is left out
    // (Annotate › Colour, Width and Text size still reach it), keeping
    // room for the Text field rather than cutting a control in half.
    let gap = ui.spacing().item_spacing.x;
    let text_tool = app.ui.tool == Tool::Draw(Kind::Text);
    let reserve = if text_tool { TEXT_MIN + gap } else { 0.0 };
    let fits = |ui: &Ui, width: f32| ui.available_width() >= width + reserve;
    let inks_width = app.inks.len() as f32 * (SWATCH + gap) + INKS_GAP;
    if fits(ui, inks_width) {
        inks(ui, app, editable, events);
    }
    let widths: Vec<String> = WIDTHS.iter().map(|(_, key, _)| label(key)).collect();
    let (inner, outer) = combo_size(ui, &widths);
    if fits(ui, outer + gap) {
        width_combo(ui, app, &widths, inner, editable, events);
    }
    if matches!(app.ui.tool, Tool::Draw(Kind::Text | Kind::Number)) {
        let sizes: Vec<String> = SIZES.iter().map(|(_, key, _)| label(key)).collect();
        let (inner, outer) = combo_size(ui, &sizes);
        if fits(ui, outer + gap) {
            size_combo(ui, app, &sizes, inner, editable, events);
        }
    }
    if text_tool {
        // The field takes what is left, up to its full width.
        let width = (ui.available_width() - 2.0).clamp(TEXT_MIN, TEXT_MAX);
        let mut text = app.ui.text.clone();
        let response = field::text(ui, &mut text, &label("text-placeholder"), width);
        if response.changed() {
            events.push(UiEvent::Text(text));
        }
    }
}

/// The ink swatches.
fn inks(ui: &mut Ui, app: &App, editable: bool, events: &mut Vec<UiEvent>) {
    for ink in app.inks {
        let selected = app.ui.colour == ink.rgba;
        let (rect, response) = ui.allocate_exact_size(vec2(SWATCH, SWATCH), Sense::click());
        let name = label(ink.label);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, editable, selected, &name)
        });
        let visuals = ui.visuals();
        let outline = if selected {
            Stroke::new(2.0, visuals.selection.stroke.color)
        } else {
            visuals.widgets.inactive.bg_stroke
        };
        let [r, g, b, a] = ink.rgba;
        ui.painter().rect_filled(
            rect.shrink(2.0),
            2.0,
            Color32::from_rgba_unmultiplied(r, g, b, a),
        );
        ui.painter()
            .rect_stroke(rect, 3.0, outline, egui::StrokeKind::Inside);
        if response.on_hover_text(&name).clicked() && editable {
            events.push(UiEvent::Command(ink_id(ink.name), Origin::Ui));
        }
    }
    ui.add_space(INKS_GAP);
}

/// The stroke width list.
fn width_combo(
    ui: &mut Ui,
    app: &App,
    widths: &[String],
    inner: f32,
    editable: bool,
    events: &mut Vec<UiEvent>,
) {
    let mut at = WIDTHS
        .iter()
        .position(|(_, _, w)| *w == app.ui.width)
        .unwrap_or(1);
    let response = ui
        .add_enabled_ui(editable, |ui| {
            toolkit::combo::show(ui, "screenshot-width", &mut at, widths, Some(inner))
        })
        .inner
        .on_hover_text(label("width"));
    if response.changed() {
        events.push(UiEvent::Command(width_id(WIDTHS[at].0), Origin::Ui));
    }
}

/// A list's size from its longest choice, as egui lays a combo out (the
/// width it is given is only a minimum): `inner`, the width it is given so
/// it keeps one width whatever is chosen, and `outer`, the room it takes
/// with its padding (what the fit check needs).
fn combo_size(ui: &Ui, options: &[String]) -> (f32, f32) {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text = options
        .iter()
        .map(|o| {
            ui.painter()
                .layout_no_wrap(o.clone(), font.clone(), Color32::PLACEHOLDER)
                .size()
                .x
        })
        .fold(0.0, f32::max);
    let spacing = ui.spacing();
    let inner = (text + spacing.icon_spacing + spacing.icon_width).max(COMBO);
    (inner, inner + 2.0 * spacing.button_padding.x)
}

/// The text size list (Text and Number tools).
fn size_combo(
    ui: &mut Ui,
    app: &App,
    sizes: &[String],
    inner: f32,
    editable: bool,
    events: &mut Vec<UiEvent>,
) {
    let mut at = SIZES
        .iter()
        .position(|(_, _, s)| *s == app.ui.text_size)
        .unwrap_or(1);
    let response = ui
        .add_enabled_ui(editable, |ui| {
            toolkit::combo::show(ui, "screenshot-size", &mut at, sizes, Some(inner))
        })
        .inner
        .on_hover_text(label("text-size"));
    if response.changed() {
        events.push(UiEvent::Command(size_id(SIZES[at].0), Origin::Ui));
    }
}

/// The options bar: Copy and Save keep their place at the right end; the
/// controls take the rest, and what does not fit is cut off at the
/// buttons, never drawn under them.
fn options(
    ui: &mut Ui,
    app: &App,
    commands: &Commands,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if app.document.is_some() {
            file_buttons(ui, app, commands, strings, events);
            ui.add_space(8.0);
        }
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            let room = ui.max_rect().intersect(ui.clip_rect());
            ui.set_clip_rect(room);
            controls(ui, app, commands, strings, events);
        });
    });
}

/// Save and Copy, labelled, through their commands (enablement and
/// shortcuts stay in the registry). Save's tip names the folder. Laid out
/// right to left: Save at the end.
fn file_buttons(
    ui: &mut Ui,
    app: &App,
    commands: &Commands,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    for (id, text) in [("file.save", "save"), ("file.copy", "copy-short")] {
        let Some(command) = commands.registry().get(id) else {
            continue;
        };
        let tip = if id == "file.save" {
            let shortcut = command.shortcut.map(|s| ui.ctx().format_shortcut(&s));
            tooltip::text(
                &label_with("save-tip", &[("folder", screenshots_folder())]),
                shortcut.as_deref(),
            )
        } else {
            tooltip::for_command(ui.ctx(), command, strings)
        };
        let response = ui
            .add_enabled(commands.registry().enabled(id, app).is_ok(), PushButton::secondary(label(text)))
            .on_hover_text(tip);
        if response.clicked() {
            events.push(UiEvent::Command(id, Origin::Ui));
        }
    }
}

/// The screenshots folder as Save's tip names it (read once: drawing never
/// touches the disk each frame).
fn screenshots_folder() -> &'static str {
    static FOLDER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FOLDER.get_or_init(|| {
        annotation::library::screenshots_dir()
            .map(|folder| crate::app::shown_path(&folder))
            .unwrap_or_default()
    })
}

fn tools(
    ui: &mut Ui,
    app: &App,
    commands: &Commands,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    for tool in Tool::ALL {
        command_button(
            ui,
            app,
            commands,
            strings,
            tool_id(tool),
            app.ui.tool == tool,
            events,
        );
        if tool == Tool::Crop {
            bars::hairline(ui);
        }
    }
    bars::hairline(ui);
    for id in ["edit.undo", "edit.redo", "edit.delete"] {
        command_button(ui, app, commands, strings, id, false, events);
    }
}

fn status(ui: &mut Ui, app: &App) {
    ui.small(&app.status);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if let Some(doc) = &app.document {
            let (w, h) = doc.output_dimensions();
            let state = if app.unsaved() {
                label("not-saved")
            } else {
                label("saved")
            };
            ui.small(format!("{w} × {h} · {:.0}% · {state}", app.ui.zoom * 100.0));
        }
    });
}

/// The view point over image point `p` (original pixels).
fn to_view(rect: Rect, vp: Viewport, crop: Option<Crop>, p: Point) -> Pos2 {
    let (ox, oy) = crop.map_or((0.0, 0.0), |c| (c.x as f32, c.y as f32));
    let v = vp.view(Point {
        x: p.x - ox,
        y: p.y - oy,
    });
    pos2(rect.min.x + v.x, rect.min.y + v.y)
}

/// The image point (original pixels) under view point `p`.
fn to_image(rect: Rect, vp: Viewport, crop: Option<Crop>, p: Pos2) -> Point {
    let (ox, oy) = crop.map_or((0.0, 0.0), |c| (c.x as f32, c.y as f32));
    let q = vp.image(Point {
        x: p.x - rect.min.x,
        y: p.y - rect.min.y,
    });
    Point {
        x: q.x + ox,
        y: q.y + oy,
    }
}

fn picture(
    ui: &mut Ui,
    app: &App,
    preview: Option<&Preview>,
    drag: &mut Drag,
    events: &mut Vec<UiEvent>,
) {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    toolkit::canvas::surround(ui, rect);
    let Some(doc) = &app.document else {
        drag.points.clear();
        let text = if app.busy() {
            label("capturing")
        } else {
            label("empty")
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().weak_text_color(),
        );
        return;
    };
    let (w, h) = doc.output_dimensions();
    let vp = Viewport::fit(
        (w, h),
        (rect.width(), rect.height()),
        app.ui.zoom,
        app.ui.pan,
    );
    // The app restyles the selection at this scale; this frame's own
    // gestures pass it directly (the event lands after them).
    if vp.scale.is_finite()
        && vp.scale > 0.0
        && (vp.scale - app.view_scale).abs() > f32::EPSILON * vp.scale.max(1.0)
    {
        events.push(UiEvent::Scale(vp.scale));
    }
    let image_rect = Rect::from_min_size(
        pos2(rect.min.x + vp.origin.x, rect.min.y + vp.origin.y),
        vec2(vp.width, vp.height),
    );
    let painter = ui.painter_at(rect);
    match preview.filter(|p| p.document == app.document_id && p.size == [w as usize, h as usize]) {
        Some(preview) => {
            egui::Image::from_texture(egui::load::SizedTexture::new(
                preview.texture.id(),
                image_rect.size(),
            ))
            .paint_at(ui, image_rect);
        }
        None => {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label("working"),
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().weak_text_color(),
            );
        }
    }
    let crop = doc.crop();
    let active = app.editable();
    // The selection's bounds.
    if let Some(object) = app.ui.selected.and_then(|id| doc.object(id)) {
        let (l, t, r, b) = object.shape.bounds();
        let a = to_view(rect, vp, crop, Point { x: l, y: t });
        let z = to_view(rect, vp, crop, Point { x: r, y: b });
        let margin = (object.shape.width * vp.scale / 2.0).max(2.0);
        outline(ui, &painter, Rect::from_two_pos(a, z).expand(margin));
    }
    if !active {
        drag.points.clear();
        drag.moving = None;
        return;
    }
    // Cursor: a crosshair to draw or crop, a hand over what can be moved.
    if let Some(at) = response.hover_pos() {
        let over = doc.hit(to_image(rect, vp, crop, at), PICK / vp.scale);
        ui.ctx().set_cursor_icon(match app.ui.tool {
            Tool::Select if over.is_some() => egui::CursorIcon::Grab,
            Tool::Select => egui::CursorIcon::Default,
            _ => egui::CursorIcon::Crosshair,
        });
    }
    // Wheel pans, Ctrl+wheel (or a pinch) zooms.
    if response.hovered() {
        let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
        if (zoom - 1.0).abs() > f32::EPSILON {
            events.push(UiEvent::Zoom(zoom));
        } else if scroll != egui::Vec2::ZERO {
            events.push(UiEvent::Pan(scroll.x, scroll.y));
        }
    }
    if response.dragged_by(PointerButton::Middle) {
        let delta = response.drag_delta();
        events.push(UiEvent::Pan(delta.x, delta.y));
    }
    if response.clicked()
        && let Some(at) = response.interact_pointer_pos()
        && image_rect.contains(at)
    {
        click(app, doc, to_image(rect, vp, crop, at), vp, events);
    }
    if response.drag_started_by(PointerButton::Primary)
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        && image_rect.contains(origin)
    {
        let p = to_image(rect, vp, crop, origin);
        drag.points = vec![p];
        drag.moving = None;
        if app.ui.tool == Tool::Select {
            drag.moving = doc.hit(p, PICK / vp.scale);
            events.push(UiEvent::Select(drag.moving));
        }
    }
    if response.dragged_by(PointerButton::Primary)
        && !drag.points.is_empty()
        && let Some(at) = response.interact_pointer_pos()
    {
        let p = to_image(rect, vp, crop, at);
        if matches!(app.ui.tool, Tool::Draw(kind) if kind.freehand()) {
            if drag.points.len() < annotation::document::MAX_POINTS {
                drag.points.push(p);
            } else if let Some(last) = drag.points.last_mut() {
                *last = p;
            }
        } else {
            drag.points.truncate(1);
            drag.points.push(p);
        }
    }
    if drag.points.len() > 1 {
        guide(ui, &painter, app, drag, rect, vp, crop);
    }
    if response.drag_stopped_by(PointerButton::Primary) {
        let points = std::mem::take(&mut drag.points);
        let moving = drag.moving.take();
        if points.len() > 1 {
            finish(app, doc, points, moving, vp.scale, events);
        }
    }
}

/// A new object of `kind` over `points`, in the chosen ink, with the
/// chosen width (and, for text and markers, size) converted at `scale`,
/// this frame's view scale: what is committed is what was shown.
fn styled(app: &App, kind: Kind, points: Vec<Point>, scale: f32) -> Shape {
    let mut shape = Shape::new(kind, points, app.ui.colour, app.stroke_width_at(scale));
    if kind.boxed_text() {
        shape.size = Some(app.text_px_at(scale));
    }
    shape
}

/// A click: pick with Select; drop a marker or a text box at the point.
fn click(app: &App, doc: &annotation::Document, p: Point, vp: Viewport, events: &mut Vec<UiEvent>) {
    let size = app.text_px_at(vp.scale);
    match app.ui.tool {
        Tool::Select => events.push(UiEvent::Select(doc.hit(p, PICK / vp.scale))),
        Tool::Draw(Kind::Number) => {
            let half = size * 0.8;
            events.push(UiEvent::Add(styled(
                app,
                Kind::Number,
                vec![
                    Point {
                        x: p.x - half,
                        y: p.y - half,
                    },
                    Point {
                        x: p.x + half,
                        y: p.y + half,
                    },
                ],
                vp.scale,
            )));
        }
        Tool::Draw(Kind::Text) => {
            let lines = app.ui.text.lines().count().max(1) as f32;
            let longest = app
                .ui
                .text
                .lines()
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(1) as f32;
            let width = (longest * size * 0.62 + size).max(size * 2.0);
            let height = lines * size * 1.25 + size * 0.25;
            events.push(UiEvent::Add(styled(
                app,
                Kind::Text,
                vec![
                    p,
                    Point {
                        x: p.x + width,
                        y: p.y + height,
                    },
                ],
                vp.scale,
            )));
        }
        _ => {}
    }
}

/// A drag ended: an object, a crop or a move.
fn finish(
    app: &App,
    doc: &annotation::Document,
    points: Vec<Point>,
    moving: Option<u64>,
    scale: f32,
    events: &mut Vec<UiEvent>,
) {
    let a = points[0];
    let b = *points.last().expect("two points");
    match app.ui.tool {
        Tool::Select => {
            if let Some(id) = moving {
                events.push(UiEvent::Move(id, b.x - a.x, b.y - a.y));
            }
        }
        Tool::Crop => {
            let (w, h) = doc.dimensions();
            let x = a.x.min(b.x).floor().clamp(0.0, w as f32) as u32;
            let y = a.y.min(b.y).floor().clamp(0.0, h as f32) as u32;
            let r = a.x.max(b.x).ceil().clamp(0.0, w as f32) as u32;
            let bottom = a.y.max(b.y).ceil().clamp(0.0, h as f32) as u32;
            if r > x && bottom > y {
                events.push(UiEvent::Crop(Crop {
                    x,
                    y,
                    width: r - x,
                    height: bottom - y,
                }));
            }
        }
        Tool::Draw(kind) => {
            let points = if kind.freehand() { points } else { vec![a, b] };
            events.push(UiEvent::Add(styled(app, kind, points, scale)));
        }
    }
}

/// A selection, crop or move outline that shows on any picture: the
/// window's text colour over its panel colour, one light and one dark in
/// every theme.
fn outline(ui: &Ui, painter: &egui::Painter, rect: Rect) {
    let visuals = ui.visuals();
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(3.0, visuals.panel_fill),
        egui::StrokeKind::Outside,
    );
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, visuals.text_color()),
        egui::StrokeKind::Outside,
    );
}

/// The gesture in progress, in the ink it will take (the crop and the
/// move outlined).
fn guide(
    ui: &Ui,
    painter: &egui::Painter,
    app: &App,
    drag: &Drag,
    rect: Rect,
    vp: Viewport,
    crop: Option<Crop>,
) {
    let view = |p: Point| to_view(rect, vp, crop, p);
    let a = view(drag.points[0]);
    let b = view(*drag.points.last().expect("two points"));
    let [r, g, bl, al] = app.ui.colour;
    let ink = Color32::from_rgba_unmultiplied(r, g, bl, al);
    let width = (app.stroke_width_at(vp.scale) * vp.scale).max(1.0);
    let stroke = Stroke::new(width, ink);
    let boxed = Rect::from_two_pos(a, b);
    match app.ui.tool {
        Tool::Select => {
            if let Some(object) = drag.moving.and_then(|id| app.document.as_ref()?.object(id)) {
                let (l, t, rr, bb) = object.shape.bounds();
                let delta = b - a;
                let moved =
                    Rect::from_two_pos(view(Point { x: l, y: t }), view(Point { x: rr, y: bb }))
                        .translate(delta);
                outline(ui, painter, moved);
            }
        }
        Tool::Crop => outline(ui, painter, boxed),
        Tool::Draw(Kind::Redact) => {
            painter.rect_filled(boxed, 0.0, ink.to_opaque());
        }
        Tool::Draw(Kind::Rectangle | Kind::Text) => {
            painter.rect_stroke(boxed, 0.0, stroke, egui::StrokeKind::Middle);
        }
        Tool::Draw(Kind::Ellipse | Kind::Number) => {
            painter.add(egui::Shape::ellipse_stroke(
                boxed.center(),
                boxed.size() / 2.0,
                stroke,
            ));
        }
        Tool::Draw(Kind::Pen | Kind::Highlighter) => {
            let points: Vec<Pos2> = drag.points.iter().map(|p| view(*p)).collect();
            let stroke = if app.ui.tool == Tool::Draw(Kind::Highlighter) {
                Stroke::new(width, ink.gamma_multiply(80.0 / 255.0))
            } else {
                stroke
            };
            painter.add(egui::Shape::line(points, stroke));
        }
        Tool::Draw(Kind::Line | Kind::Arrow) => {
            painter.line_segment([a, b], stroke);
            if app.ui.tool == Tool::Draw(Kind::Arrow) {
                let angle = (b.y - a.y).atan2(b.x - a.x);
                let length = (app.stroke_width_at(vp.scale) * 4.0).max(12.0) * vp.scale;
                for offset in [-0.5_f32, 0.5] {
                    let tip = pos2(
                        b.x - length * (angle + offset).cos(),
                        b.y - length * (angle + offset).sin(),
                    );
                    painter.line_segment([b, tip], stroke);
                }
            }
        }
    }
}

fn dialogs(
    ctx: &egui::Context,
    app: &App,
    commands: &Commands,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let Some(dialog) = app.ui.dialog else {
        return;
    };
    let id = Id::new("screenshot-dialog").with(dialog.name());
    let mut closed = false;
    match dialog {
        Dialog::Discard => {
            let shown = dialog::Dialog::new(id, label("discard-title"))
                .buttons(vec![
                    Choice::new(label("save"), Role::Default),
                    Choice::new(label("discard"), Role::Other),
                    Choice::new(label("keep"), Role::Cancel),
                ])
                .show(ctx, |ui| {
                    ui.add(egui::Label::new(label("discard-body")).wrap());
                });
            let answer = if shown.dismissed {
                Some(Answer::Keep)
            } else {
                shown.chosen.and_then(|i| {
                    [Answer::Save, Answer::Discard, Answer::Keep]
                        .get(i)
                        .copied()
                })
            };
            if let Some(answer) = answer {
                closed = true;
                events.push(UiEvent::Answer(answer));
            }
        }
        Dialog::Open | Dialog::SaveAs => {
            let (title, ok) = if dialog == Dialog::Open {
                ("open-title", "open-button")
            } else {
                ("save-title", "save-button")
            };
            let mut submitted = false;
            let shown = dialog::Dialog::new(id, label(title))
                .width(egui::Rangef::new(480.0, 600.0))
                .buttons(vec![
                    Choice::new(label(ok), Role::Default),
                    Choice::new(label("cancel"), Role::Cancel),
                ])
                .show(ctx, |ui| {
                    ui.label(label("path"));
                    let mut value = app.ui.path.clone();
                    let field = egui::TextEdit::singleline(&mut value)
                        .id(id.with("field"))
                        .desired_width(f32::INFINITY);
                    let response = ui.add(field);
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submitted = true;
                    }
                    if !response.has_focus() && ui.memory(|m| m.focused().is_none()) {
                        response.request_focus();
                    }
                    if response.changed() {
                        events.push(UiEvent::Path(value));
                    }
                    ui.label(RichText::new(&app.status).small().weak());
                });
            if shown.dismissed || shown.chosen == Some(1) {
                closed = true;
                events.push(UiEvent::CloseDialog);
            } else if submitted || shown.chosen == Some(0) {
                events.push(UiEvent::SubmitPath);
            }
        }
        Dialog::Shortcuts => {
            let shown = dialog::Dialog::new(id, label("shortcuts"))
                .buttons(vec![Choice::new(label("done"), Role::Default)])
                .show(ctx, |ui| commands.shortcut_grid(ui, strings));
            if shown.chosen.is_some() || shown.dismissed {
                closed = true;
                events.push(UiEvent::CloseDialog);
            }
        }
        Dialog::About => {
            let info = buildinfo::build_info!();
            let shown = dialog::Dialog::new(id, label("about"))
                .buttons(vec![Choice::new(label("done"), Role::Default)])
                .show(ctx, |ui| {
                    ui.add(egui::Label::new(label("about-body")).wrap());
                    ui.label(RichText::new(format!("{} ({})", info.version, info.git_sha)).weak());
                });
            if shown.chosen.is_some() || shown.dismissed {
                closed = true;
                events.push(UiEvent::CloseDialog);
            }
        }
    }
    if closed {
        // Each opening starts centred again.
        dialog::Dialog::reset(ctx, id);
    }
}

/// Apply one frame's interactions: edits first, then commands, so a
/// command sees every edit made in the same frame.
pub fn apply(app: &mut App, _commands: &Commands, events: Vec<UiEvent>) {
    let (fired, edits): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|e| matches!(e, UiEvent::Command(..)));
    for event in edits.into_iter().chain(fired) {
        match event {
            UiEvent::Command(id, origin) => {
                let _ = app.execute(id, serde_json::Value::Null, origin);
            }
            UiEvent::Add(shape) => app.add_shape(shape),
            UiEvent::Crop(crop) => app.crop(crop),
            UiEvent::Move(id, dx, dy) => app.move_object(id, dx, dy),
            UiEvent::Select(id) => app.select(id),
            UiEvent::Pan(dx, dy) => app.pan(dx, dy),
            UiEvent::Zoom(factor) => app.zoom_by(factor),
            UiEvent::Scale(scale) => app.set_view_scale(scale),
            UiEvent::Text(text) => app.set_text(text),
            UiEvent::Path(path) => app.set_path(path),
            UiEvent::Window(window) => app.choose_window(window),
            UiEvent::Output(output) => app.choose_output(output),
            UiEvent::Refresh => app.refresh(),
            UiEvent::Answer(answer) => app.answer(answer),
            UiEvent::SubmitPath => app.submit_path(),
            UiEvent::CloseDialog => app.close_dialog(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use annotation::Document;
    use image::RgbaImage;

    fn added(events: &[UiEvent]) -> Vec<Shape> {
        events
            .iter()
            .filter_map(|e| match e {
                UiEvent::Add(shape) => Some(shape.clone()),
                _ => None,
            })
            .collect()
    }

    /// The first frame after a zoom or resize: the app still holds the
    /// last reported scale, and the gesture commits at this frame's.
    #[test]
    fn a_commit_takes_this_frames_scale_not_the_last_reported_one() {
        let mut app = App::new(crate::label, crate::ink::inks(), false);
        app.document = Some(Document::new(RgbaImage::new(3840, 2160)).unwrap());
        app.set_view_scale(1.0);
        let vp = Viewport::fit((3840, 2160), (960.0, 540.0), 1.0, app.ui.pan);
        assert_eq!(vp.scale, 0.25);
        let a = Point { x: 100.0, y: 100.0 };
        let b = Point { x: 900.0, y: 500.0 };
        let mut events = Vec::new();
        app.set_tool(Tool::Draw(Kind::Arrow));
        let doc = app.document.as_ref().unwrap();
        finish(&app, doc, vec![a, b], None, vp.scale, &mut events);
        app.set_tool(Tool::Draw(Kind::Text));
        app.set_text("Note".into());
        let doc = app.document.as_ref().unwrap();
        click(&app, doc, a, vp, &mut events);
        app.set_tool(Tool::Draw(Kind::Number));
        let doc = app.document.as_ref().unwrap();
        click(&app, doc, b, vp, &mut events);
        let shapes = added(&events);
        assert_eq!(shapes.len(), 3);
        // Medium (4 points) and medium text (24 points) at a quarter scale.
        assert!(shapes.iter().all(|s| s.width == 16.0));
        assert_eq!(shapes[0].size, None);
        assert_eq!((shapes[1].size, shapes[2].size), (Some(96.0), Some(96.0)));
        // The marker's box is sized at the same scale as its text.
        let (l, _, r, _) = shapes[2].bounds();
        assert!((r - l - 96.0 * 1.6).abs() < 1e-3);
        // Applied with the stale scale still in the app, they keep it.
        let commands = crate::commands::registry();
        apply(&mut app, &commands, events);
        let doc = app.document.as_ref().unwrap();
        let objects = doc.objects();
        assert_eq!(objects.len(), 3);
        assert!(objects.iter().all(|o| o.shape.width == 16.0));
        assert_eq!(objects[1].shape.size, Some(96.0));
        assert_eq!(objects[2].shape.size, Some(96.0));
    }
}
