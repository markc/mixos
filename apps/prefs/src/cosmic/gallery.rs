// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Wallpaper's controls on the page (spec 1 §4.1): the framed preview, the
//! display selector, and the chooser (its category menu, the add link, and
//! a wrapping grid of thumbnails or 70 px swatches, 12 apart across and 16
//! down). Thumbnails are the shared cache's, made by the system's
//! thumbnailers (`thumbs`) and decoded off the window thread
//! (`toolkit::thumbnail`); leaving the page frees them ([`release`]).
//! "Add image" and "Open new folder" ask the files chooser.
use super::{Look, named, parse_hex};
use crate::view::UiEvent;
use crate::{label, label_with};
use egui::{
    Align, Align2, Color32, Context, Id, Layout, Pos2, Rect, Response, RichText, Role, Sense,
    Stroke, StrokeKind, TextureHandle, Ui, Vec2, WidgetInfo, pos2, vec2,
};
use preferences::Engine;
use preferences::gallery::Category;
use preferences::pages::wallpaper::{CUSTOM_COLOURS, CUSTOM_IMAGES, SAME, SLIDESHOW, output_image};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, mpsc};
use toolkit::Icon;

/// The preview's screen (spec 1 §4.1).
const SCREEN: Vec2 = vec2(300.0, 169.0);
/// The frame around it.
const FRAME: f32 = 4.0;
/// An image thumbnail in the grid.
const THUMB: Vec2 = vec2(158.0, 89.0);
const SWATCH: f32 = 70.0;
const GAP: Vec2 = vec2(12.0, 16.0);
const MAX_TEXTURES: usize = 128;

/// A colour fill: flat, or a two-stop linear gradient at an angle (CSS
/// degrees: 180 runs top to bottom).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fill {
    Flat(Color32),
    Gradient(Color32, Color32, f32),
}

fn fill_of(text: &str) -> Option<Fill> {
    let Some(rest) = text.strip_prefix("gradient:") else {
        return parse_hex(text).map(Fill::Flat);
    };
    let (stops, angle) = rest.split_once('@')?;
    let (a, b) = stops.split_once(',')?;
    Some(Fill::Gradient(
        parse_hex(a)?,
        parse_hex(b)?,
        angle.parse().ok()?,
    ))
}

impl Fill {
    fn at(self, rect: Rect, p: Pos2) -> Color32 {
        match self {
            Fill::Flat(c) => c,
            Fill::Gradient(a, b, degrees) => {
                let (sin, cos) = degrees.to_radians().sin_cos();
                let direction = vec2(sin, -cos);
                let length = (rect.width() * sin).abs() + (rect.height() * cos).abs();
                let t = (p - rect.center()).dot(direction) / length.max(1.0) + 0.5;
                a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
            }
        }
    }
}

/// A rounded rectangle coloured per point by `colour`, or textured with
/// `texture` over `uv`: the toolkit's rounded rectangle, re-coloured, so a
/// gradient or an image keeps the corners.
fn paint_rounded(
    ui: &Ui,
    rect: Rect,
    radius: f32,
    colour: impl Fn(Pos2) -> Color32,
    texture: Option<(egui::TextureId, Rect)>,
) {
    let shape = egui::epaint::RectShape::filled(rect, Look::radius(radius), Color32::WHITE);
    let mut tessellator = egui::epaint::Tessellator::new(
        ui.ctx().pixels_per_point(),
        egui::epaint::TessellationOptions::default(),
        [1, 1],
        Vec::new(),
    );
    let mut mesh = egui::epaint::Mesh::default();
    tessellator.tessellate_rect(&shape, &mut mesh);
    for vertex in &mut mesh.vertices {
        let coverage = f32::from(vertex.color.a()) / 255.0;
        vertex.color = colour(vertex.pos).gamma_multiply(coverage);
        if let Some((_, uv)) = texture {
            let t = ((vertex.pos - rect.min) / rect.size()).clamp(Vec2::ZERO, Vec2::splat(1.0));
            vertex.uv = uv.lerp_inside(t);
        }
    }
    if let Some((id, _)) = texture {
        mesh.texture_id = id;
    }
    ui.painter().add(egui::Shape::mesh(mesh));
}

/// The part of an image of `size` that fills `target` (cropped, centred).
fn crop(size: Vec2, target: Vec2) -> Rect {
    let (image, wanted) = (size.x / size.y.max(1.0), target.x / target.y.max(1.0));
    if image > wanted {
        let w = wanted / image;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = image / wanted;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    }
}

/// The part of `target` an image of `size` takes when fitted (letterboxed).
fn fitted(size: Vec2, target: Rect) -> Rect {
    let scale = (target.width() / size.x.max(1.0)).min(target.height() / size.y.max(1.0));
    Rect::from_center_size(target.center(), size * scale)
}

/// The thumbnails asked for and made, and the files chooser open.
struct State {
    pool: thumbs::Pool<String>,
    replies: mpsc::Receiver<thumbs::Reply<String>>,
    decoder: toolkit::thumbnail::Decoder<String>,
    live: Arc<AtomicU64>,
    /// By size and path; `None` when the file has no thumbnail.
    textures: HashMap<String, Option<TextureHandle>>,
    asked: HashMap<String, u64>,
    used: u64,
    chooser: Option<(Choosing, toolkit::choose::Handle)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choosing {
    Images,
    Folder,
}

type Shared = Arc<Mutex<State>>;

fn state_id() -> Id {
    Id::unique("prefs-gallery")
}

impl State {
    fn new(ctx: &Context) -> Self {
        let (tx, replies) = mpsc::channel();
        let wake = ctx.clone();
        let pool = thumbs::Pool::new("prefs", move |reply| {
            let sent = tx.send(reply).is_ok();
            wake.request_repaint();
            sent
        });
        Self {
            pool,
            replies,
            decoder: toolkit::thumbnail::Decoder::new("prefs-thumbs"),
            live: Arc::new(AtomicU64::new(0)),
            textures: HashMap::new(),
            asked: HashMap::new(),
            used: 0,
            chooser: None,
        }
    }

    fn pump(&mut self, ctx: &Context) {
        while let Ok(reply) = self.replies.try_recv() {
            if !self.asked.contains_key(&reply.lane) {
                continue;
            }
            match reply.outcome {
                thumbs::Outcome::Ready(cache) => {
                    if !self.decoder.ask(&reply.lane, &cache, ctx) {
                        self.textures.insert(reply.lane, None);
                    }
                }
                _ => {
                    self.textures.insert(reply.lane, None);
                }
            }
        }
        for (key, image) in self.decoder.drain() {
            if !self.asked.contains_key(&key) {
                continue;
            }
            let texture =
                image.map(|image| ctx.load_texture(&key, image, egui::TextureOptions::LINEAR));
            self.textures.insert(key, texture);
        }
    }

    /// The thumbnail of `path` at `size`, once made; asked for the first
    /// time it is wanted.
    fn texture(&mut self, path: &str, size: thumbs::Size) -> Option<TextureHandle> {
        let key = format!("{}:{path}", size.px());
        self.used += 1;
        let first = self.asked.insert(key.clone(), self.used).is_none();
        while self.asked.len() > MAX_TEXTURES {
            let oldest = self.asked.iter().min_by_key(|(_, used)| *used).map(|(k, _)| k.clone()).expect("nonempty cache");
            self.asked.remove(&oldest);
            self.textures.remove(&oldest);
        }
        if let Some(texture) = self.textures.get(&key) {
            return texture.clone();
        }
        if first {
            self.pool.submit(thumbs::Request {
                lane: key,
                ticket: thumbs::Ticket::new(self.live.clone(), 0),
                path: PathBuf::from(path),
                size,
            });
        }
        None
    }
}

fn state(ctx: &Context) -> Shared {
    if let Some(shared) = ctx.data(|d| d.get_temp::<Shared>(state_id())) {
        return shared;
    }
    let shared: Shared = Arc::new(Mutex::new(State::new(ctx)));
    ctx.data_mut(|d| d.insert_temp(state_id(), shared.clone()));
    shared
}

fn with<R>(ctx: &Context, f: impl FnOnce(&mut State) -> R) -> R {
    let shared = state(ctx);
    let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
    state.pump(ctx);
    f(&mut state)
}

/// Free the thumbnails and close the chooser (the page was left).
pub fn release(ctx: &Context) {
    if ctx.data(|d| d.get_temp::<Shared>(state_id())).is_some() {
        ctx.data_mut(|d| d.remove::<Shared>(state_id()));
    }
}

/// The image the selected output shows: its own while the outputs differ,
/// else the shared one; a slideshow shows its stored image too.
fn shown_image(engine: &Engine, image: &str) -> Option<String> {
    if !engine.store.flag(SAME)
        && let Some(output) = engine.selected_output()
        && let Some(own) = engine.store.text(&output_image(&output))
    {
        return Some(own);
    }
    engine.store.text(image)
}

/// The framed screen: the image filled or fitted, else the colour, else
/// the theme's background.
pub fn preview(
    ui: &mut Ui,
    engine: &Engine,
    keys: (&str, &str, &str),
    name: &str,
    look: &Look,
) -> Response {
    let (image, colour, fit) = keys;
    let outer = SCREEN + Vec2::splat(2.0 * FRAME);
    let (slot, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), outer.y), Sense::hover());
    let frame = Rect::from_center_size(slot.center(), outer);
    ui.painter()
        .rect_filled(frame, Look::radius(look.r_xs + FRAME), look.text);
    let screen = frame.shrink(FRAME);
    let shown = shown_image(engine, image).or_else(|| {
        engine
            .store
            .flag(SLIDESHOW)
            .then(|| engine.gallery_images().first().cloned())
            .flatten()
    });
    let texture = shown.as_deref().and_then(|path| {
        with(ui.ctx(), |s| {
            s.texture(path, thumbs::Size::at_least(SCREEN.x as u32))
        })
    });
    match (texture, engine.store.text(colour).and_then(|c| fill_of(&c))) {
        (Some(texture), _) => {
            let size = texture.size_vec2();
            if engine.store.text(fit).as_deref() == Some("fit") {
                ui.painter()
                    .rect_filled(screen, Look::radius(look.r_xs), Color32::BLACK);
                let r = fitted(size, screen);
                paint_rounded(
                    ui,
                    r,
                    0.0,
                    |_| Color32::WHITE,
                    Some((texture.id(), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)))),
                );
            } else {
                let uv = crop(size, screen.size());
                paint_rounded(
                    ui,
                    screen,
                    look.r_xs,
                    |_| Color32::WHITE,
                    Some((texture.id(), uv)),
                );
            }
        }
        (None, Some(fill)) if shown.is_none() => {
            paint_rounded(ui, screen, look.r_xs, |p| fill.at(screen, p), None);
        }
        _ => {
            let base = if shown.is_some() {
                look.card
            } else {
                look.window
            };
            ui.painter()
                .rect_filled(screen, Look::radius(look.r_xs), base);
        }
    }
    response.widget_info(|| WidgetInfo::labeled(Role::Image, true, name));
    response
}

fn text_width(ui: &Ui, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(14.0),
            Color32::WHITE,
        )
        .size()
        .x
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// What the category menu shows for `category`.
fn category_label(category: &Category) -> String {
    match category {
        Category::Wallpapers => label("gallery-wallpapers"),
        Category::Colours => label("gallery-colours"),
        Category::Folder(folder) => file_name(folder),
    }
}

/// Start the files chooser for images or a folder.
fn choose(ui: &Ui, engine: &Engine, choosing: Choosing) {
    let start = engine
        .gallery_folder()
        .or_else(|| std::env::var("HOME").ok());
    let mut body = match choosing {
        Choosing::Images => json!({"mode": "open", "multiple": true,
            "title": label("gallery-add-image"), "accept_label": label("gallery-choose-add"),
            "filters": [{"name": label("gallery-images"),
                "patterns": ["*.png", "*.jpg", "*.jpeg", "*.webp"],
                "mime_types": ["image/png", "image/jpeg", "image/webp"]}]}),
        Choosing::Folder => json!({"mode": "open", "directory": true,
            "title": label("gallery-open-folder"), "accept_label": label("gallery-choose-open")}),
    };
    if let Some(start) = start {
        body["current_folder"] = json!(start);
    }
    let Ok(request) = toolkit::choose::request(&body.to_string()) else {
        return;
    };
    with(ui.ctx(), |s| {
        s.chooser = Some((choosing, toolkit::choose::choose(request)));
    });
}

/// The chooser's answer, once it gives one.
fn chosen(ui: &Ui, events: &mut Vec<UiEvent>) {
    let answer = with(ui.ctx(), |s| {
        let (choosing, handle) = s.chooser.as_mut()?;
        let outcome = handle.poll(ui.ctx())?;
        let choosing = *choosing;
        s.chooser = None;
        Some((choosing, outcome))
    });
    let Some((choosing, toolkit::choose::Outcome::Chosen { paths, .. })) = answer else {
        return;
    };
    let paths: Vec<String> = paths
        .iter()
        .filter_map(|p| p.to_str().map(str::to_owned))
        .collect();
    match choosing {
        Choosing::Images if !paths.is_empty() => events.push(UiEvent::AddImages(paths)),
        Choosing::Folder => {
            if let Some(folder) = paths.into_iter().next() {
                events.push(UiEvent::OpenFolder(folder));
            }
        }
        Choosing::Images => {}
    }
}

/// The category menu and the add link, then the grid.
pub fn chooser(
    ui: &mut Ui,
    engine: &Engine,
    row: &'static str,
    keys: (&'static str, &'static str),
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    chosen(ui, events);
    let (image, colour) = keys;
    let category = engine.category();
    ui.horizontal(|ui| {
        ui.set_width(ui.available_width());
        menu(ui, engine, &category, look, events);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let add = match category {
                Category::Wallpapers => Some("gallery-add-image"),
                Category::Colours => Some("gallery-add-colour"),
                Category::Folder(_) => None,
            };
            if let Some(add) = add {
                let text = label(add);
                if super::extra_button(ui, &text, true, ui.is_enabled(), look).clicked() {
                    match category {
                        Category::Colours => events.push(UiEvent::Drawer(Some(row))),
                        _ => choose(ui, engine, Choosing::Images),
                    }
                }
            }
        });
    });
    ui.add_space(look.m);
    let selected_image = shown_image(engine, image);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = GAP;
        if category == Category::Colours {
            let selected = selected_image
                .is_none()
                .then(|| engine.store.text(colour))
                .flatten();
            let custom = engine.custom_colours();
            for value in engine.gallery_colours() {
                let removable = custom.contains(&value);
                let on = selected.as_deref() == Some(value.as_str());
                let (response, remove) = swatch(ui, &value, on, removable, look);
                if response.clicked() && !on {
                    events.push(UiEvent::Set(colour, json!(value)));
                }
                if remove {
                    events.push(UiEvent::RemoveCustom(CUSTOM_COLOURS, value.clone()));
                }
            }
            return;
        }
        let images = engine.gallery_images();
        if images.is_empty() {
            let text = if engine.gallery.error.is_some() {
                label("gallery-unreadable")
            } else if engine.gallery_reading() {
                label("gallery-reading")
            } else {
                label("gallery-empty")
            };
            ui.label(RichText::new(text).size(14.0).color(look.text));
            return;
        }
        let custom = match category {
            Category::Wallpapers => engine.custom_images(),
            _ => Vec::new(),
        };
        for path in images {
            let on = selected_image.as_deref() == Some(path.as_str());
            let removable = custom.contains(&path);
            let (response, remove) = thumbnail(ui, &path, on, removable, look);
            if response.clicked() && !on {
                events.push(UiEvent::Set(image, json!(path)));
            }
            if remove {
                events.push(UiEvent::RemoveCustom(CUSTOM_IMAGES, path.clone()));
            }
        }
    });
}

/// The category dropdown: Wallpapers, Colors, the recent folders, and
/// "Open new folder"; a folder icon before it while a folder shows.
fn menu(ui: &mut Ui, engine: &Engine, category: &Category, look: &Look, events: &mut Vec<UiEvent>) {
    let text = category_label(category);
    let folder = matches!(category, Category::Folder(_));
    let width = text_width(ui, &text) + 8.0 + 16.0 + 8.0 + if folder { 24.0 } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    let ink = if ui.is_enabled() {
        look.text
    } else {
        look.text.gamma_multiply(0.65)
    };
    let mut x = rect.min.x + 4.0;
    if folder {
        toolkit::button::paint_icon(ui, Icon::Folder, pos2(x + 8.0, rect.center().y), 16.0, ink);
        x += 24.0;
    }
    ui.painter().text(
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        &text,
        egui::FontId::proportional(14.0),
        ink,
    );
    toolkit::button::paint_icon(
        ui,
        Icon::ChevronDown,
        pos2(rect.max.x - 12.0, rect.center().y),
        16.0,
        ink,
    );
    response.widget_info(|| {
        WidgetInfo::selected(
            Role::ComboBox,
            ui.is_enabled(),
            false,
            label("gallery-category"),
        )
    });
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width(width.max(200.0));
        for choice in [Category::Wallpapers, Category::Colours] {
            let on = *category == choice;
            if toolkit::selectable(ui, on, category_label(&choice)).clicked() && !on {
                events.push(UiEvent::Category(choice));
            }
        }
        let recent = engine.recent_folders();
        if !recent.is_empty() {
            ui.separator();
            ui.label(
                RichText::new(label("gallery-recent"))
                    .size(12.0)
                    .color(look.text),
            );
            for folder in recent {
                let choice = Category::Folder(folder.clone());
                let on = *category == choice;
                if toolkit::selectable(ui, on, file_name(&folder))
                    .on_hover_text(&folder)
                    .clicked()
                    && !on
                {
                    events.push(UiEvent::Category(choice));
                }
            }
        }
        ui.separator();
        if toolkit::selectable(ui, false, label("gallery-open-folder")).clicked() {
            choose(ui, engine, Choosing::Folder);
        }
    });
}

/// The small remove button in a tile's corner; whether it was clicked.
fn remove_button(ui: &mut Ui, tile: Rect, name: &str, look: &Look) -> bool {
    let rect = Rect::from_center_size(tile.right_top() + vec2(-12.0, 12.0), Vec2::splat(20.0));
    let response = ui.interact(rect, ui.scope_id().with(("remove", name)), Sense::click());
    ui.painter().circle_filled(
        rect.center(),
        10.0,
        if response.hovered() {
            look.button_hover
        } else {
            look.button
        },
    );
    toolkit::button::paint_icon(ui, Icon::X, rect.center(), 12.0, look.text);
    let text = label_with("gallery-remove", &[("name", name)]);
    named(response, Role::Button, ui.is_enabled(), &text).clicked()
}

fn ring(ui: &Ui, rect: Rect, on: bool, response: &Response, look: &Look) {
    if on {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            Look::radius(look.r_s + 2.0),
            Stroke::new(2.0, look.accent),
            StrokeKind::Outside,
        );
    } else if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(1.0),
            Look::radius(look.r_s),
            Stroke::new(1.0, look.accent),
            StrokeKind::Outside,
        );
    }
}

/// One image tile: its thumbnail (a placeholder until it is made).
fn thumbnail(ui: &mut Ui, path: &str, on: bool, removable: bool, look: &Look) -> (Response, bool) {
    let (rect, response) = ui.allocate_exact_size(THUMB, Sense::click());
    let name = file_name(path);
    if ui.is_rect_visible(rect) {
        match with(ui.ctx(), |s| s.texture(path, thumbs::Size::Normal)) {
            Some(texture) => {
                let uv = crop(texture.size_vec2(), THUMB);
                paint_rounded(
                    ui,
                    rect,
                    look.r_s,
                    |_| Color32::WHITE,
                    Some((texture.id(), uv)),
                );
            }
            None => {
                ui.painter()
                    .rect_filled(rect, Look::radius(look.r_s), look.card);
                toolkit::button::paint_icon(ui, Icon::Image, rect.center(), 24.0, look.icon);
            }
        }
    }
    ring(ui, rect, on, &response, look);
    response.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), on, &name));
    let remove = removable && remove_button(ui, rect, &name, look);
    (response, remove)
}

/// One colour or gradient swatch.
fn swatch(ui: &mut Ui, value: &str, on: bool, removable: bool, look: &Look) -> (Response, bool) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SWATCH), Sense::click());
    if let Some(fill) = fill_of(value) {
        paint_rounded(ui, rect, look.r_s, |p| fill.at(rect, p), None);
    }
    if on {
        toolkit::button::paint_icon(
            ui,
            Icon::Check,
            pos2(rect.min.x + 14.0, rect.max.y - 14.0),
            16.0,
            Color32::WHITE,
        );
    }
    ring(ui, rect, on, &response, look);
    response.widget_info(|| WidgetInfo::selected(Role::RadioButton, ui.is_enabled(), on, value));
    let remove = removable && remove_button(ui, rect, value, look);
    (response, remove)
}

#[cfg(test)]
mod tests {
    #[test]
    fn both_sizes_share_a_bounded_cache_and_late_replies_are_ignored() {
        let ctx = egui::Context::default();
        let mut s = super::State::new(&ctx);
        for i in 0..super::MAX_TEXTURES {
            let key = format!("128:/image{i}.png");
            s.asked.insert(key.clone(), i as u64);
            s.textures.insert(key, None);
        }
        s.used = super::MAX_TEXTURES as u64;
        s.texture("/image0.png", thumbs::Size::Normal);
        s.texture("/preview.png", thumbs::Size::at_least(300));
        assert_eq!(s.asked.len(), super::MAX_TEXTURES);
        assert!(!s.asked.contains_key("128:/image1.png"));
        assert!(!s.textures.contains_key("128:/image1.png"));
        assert!(s.asked.contains_key("128:/image0.png"));
        assert!(s.asked.contains_key("512:/preview.png"));
    }
    use super::*;

    #[test]
    fn a_gradient_runs_top_to_bottom_at_180() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(70.0, 70.0));
        let fill = fill_of("gradient:#000000,#ffffff@180").unwrap();
        assert_eq!(fill.at(rect, pos2(35.0, 0.0)), Color32::BLACK);
        assert_eq!(fill.at(rect, pos2(35.0, 70.0)), Color32::WHITE);
        assert_eq!(
            fill.at(rect, pos2(0.0, 0.0)),
            fill.at(rect, pos2(70.0, 0.0))
        );
        assert_eq!(
            fill_of("#123456"),
            Some(Fill::Flat(Color32::from_rgb(0x12, 0x34, 0x56)))
        );
        assert_eq!(fill_of("gradient:#123456"), None);
    }

    #[test]
    fn a_wide_image_is_cropped_to_fill_and_boxed_to_fit() {
        let uv = crop(vec2(400.0, 100.0), vec2(200.0, 100.0));
        assert_eq!(
            (uv.min.x, uv.max.x, uv.min.y, uv.max.y),
            (0.25, 0.75, 0.0, 1.0)
        );
        let r = fitted(
            vec2(400.0, 100.0),
            Rect::from_min_size(Pos2::ZERO, vec2(200.0, 100.0)),
        );
        assert_eq!(r.size(), vec2(200.0, 50.0));
    }
}
