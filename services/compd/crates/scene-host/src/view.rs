// SPDX-License-Identifier: MIT OR Apache-2.0
//! One scene drawn with egui: the ten families laid out as flexbox with taffy
//! ([`flex`], Quoin's own layout engine) and painted and hit-tested at those
//! rectangles ([`paint`]).
//!
//! The UI is runtime-side state only: what it shows arrives as a
//! [`SceneMessage::Replace`] per accepted revision, and every interaction
//! leaves as a message the host's handler turns into a Bus event. Its look is
//! the session's (`crate::appearance::Look`), installed on the egui context
//! whenever it changes; before the first one arrives a scene uses the
//! embedded design (an edge page in dark mode, as Quoin's panel token is).

use std::collections::BTreeMap;
use std::sync::Arc;

use decor::Srgba;
use design::{ButtonCellKey, ButtonSize, ButtonVariant, InteractionState, LinearRgba};
use egui::Color32;
use scene::{Node, ResolvedScene};
use serde_json::{Value, json};
use toolkit::theme::Theme;

use crate::appearance::Look;
use crate::templates::{children, rows, text as port_text};

mod flex;
mod paint;
pub(crate) mod tip;

pub(crate) use flex::SceneLayout;
pub use tip::{Tip, TipAt};

/// The font a tip card line draws in: the scene's own text font.
pub(crate) fn tip_font(size: f32, bold: bool) -> egui::FontId {
    paint::font(size, false, bold)
}

/// Text size where a node names none (Quoin's 13 px).
pub const TEXT_SIZE: f32 = 13.0;
/// The dialog frame's title bar (the frozen layout puts `root` at y 32).
pub const TITLEBAR: f32 = 32.0;
/// The layout key of the frame's close button.
pub const CLOSE_ID: &str = "chrome:close";

/// What one surface draws: an accepted revision and its prepared rows.
#[derive(Debug)]
pub struct Content {
    pub tree: ResolvedScene,
    pub lists: crate::templates::PreparedLists,
    pub revision: u64,
    /// The dialog frame, when the scene is a dialog with `chrome:true`.
    pub frame: Option<Frame>,
    /// The scene sits in the dialog seat (framed or not): Escape hides it.
    pub dialog: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub title: String,
}

/// Where an interaction goes: the citizen, the handler per kind, and the
/// payload's identity. A node inside a list row carries the LIST's route and
/// the row item, as Quoin's rows do.
#[derive(Debug)]
pub struct Route {
    pub scene: String,
    pub citizen: String,
    pub node: String,
    pub item: Option<Value>,
    pub click: Option<String>,
    pub change: Option<String>,
    pub submit: Option<String>,
}

impl Route {
    pub(crate) fn new(tree: &ResolvedScene, id: &str, node: &Node) -> Route {
        let port = |name: &str| {
            node.ports
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        Route {
            scene: tree.name.clone(),
            citizen: tree.citizen.clone(),
            node: id.to_owned(),
            item: None,
            click: port("on_click"),
            change: port("on_change"),
            submit: port("on_submit"),
        }
    }

    /// The handler for `kind` and the event body `{scene,node,kind,value?,item?}`.
    pub fn event(&self, kind: &str, value: Option<Value>) -> Option<(&str, &str, Value)> {
        let handler = match kind {
            "click" => self.click.as_deref(),
            "change" => self.change.as_deref(),
            _ => self.submit.as_deref(),
        }?;
        let mut body = json!({"scene": self.scene, "node": self.node, "kind": kind});
        if let Some(value) = value {
            body["value"] = value;
        }
        if let Some(item) = &self.item {
            body["item"] = item.clone();
        }
        Some((&self.citizen, handler, body))
    }
}

#[derive(Clone, Debug)]
pub enum SceneMessage {
    /// A new revision to draw.
    Replace(Arc<Content>),
    /// Restyle with the session's look, keeping local edits and focus.
    Appearance(Arc<Look>),
    Click(Arc<Route>),
    Toggle(Arc<Route>, String, bool),
    Input(Arc<Route>, String, String),
    Submit(Arc<Route>, String),
    /// The dialog frame's close button (or Escape in a dialog).
    Close,
    EscapeEdge,
    EdgeFocus(bool),
    /// Show (`Some`) or hide (`None`) this scene's hover tip.
    Tooltip(Option<TipAt>),
    /// The host took this scene's tip down (a menu opened): forget it, so
    /// the next hover shows it again.
    ForgetTip,
}

pub(crate) fn colour(c: Srgba) -> Color32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(byte(c.r), byte(c.g), byte(c.b), byte(c.a))
}

pub(crate) fn linear(c: LinearRgba) -> Color32 {
    egui::Rgba::from_rgba_unmultiplied(c.red as f32, c.green as f32, c.blue as f32, c.alpha as f32)
        .into()
}

/// `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`, as Quoin parses scene colours.
pub fn hex(value: &str) -> Option<Color32> {
    let digits = value.strip_prefix('#')?;
    if !digits.is_ascii() {
        return None;
    }
    let nibble = |i: usize| {
        u8::from_str_radix(&digits[i..i + 1], 16)
            .ok()
            .map(|v| v * 17)
    };
    let byte = |i: usize| u8::from_str_radix(digits.get(i..i + 2)?, 16).ok();
    let (r, g, b, a) = match digits.len() {
        3 => (nibble(0)?, nibble(1)?, nibble(2)?, 255),
        4 => (nibble(0)?, nibble(1)?, nibble(2)?, nibble(3)?),
        6 => (byte(0)?, byte(2)?, byte(4)?, 255),
        8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    Some(Color32::from_rgba_unmultiplied(r, g, b, a))
}

/// `#rrggbbaa`, the form [`hex`] reads back (sRGB, unmultiplied).
pub fn hex_of(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
}

pub(crate) fn number(node: &Node, port: &str) -> Option<f32> {
    node.ports
        .get(port)
        .and_then(Value::as_f64)
        .map(|v| v as f32)
}

pub(crate) fn flag(node: &Node, port: &str) -> bool {
    node.ports
        .get(port)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The rows a vertical list shows before it scrolls: Quoin's
/// `min(rows, max_rows (8)).max(1) * (row_height + gap)`.
pub fn list_height(node: &Node) -> f32 {
    let shown = rows(node)
        .len()
        .min(number(node, "max_rows").unwrap_or(8.0) as usize)
        .max(1);
    shown as f32 * (number(node, "row_height").unwrap_or(24.0) + number(node, "gap").unwrap_or(0.0))
}

/// Where a node's ports are read from: the document, or one list row's
/// instantiated template.
#[derive(Clone, Copy)]
pub(crate) enum Nodes<'a> {
    Tree(&'a ResolvedScene),
    Row(&'a BTreeMap<String, Node>),
}

impl<'a> Nodes<'a> {
    pub(crate) fn get(self, id: &str) -> Option<&'a Node> {
        match self {
            Nodes::Tree(tree) => tree.nodes.get(id),
            Nodes::Row(nodes) => nodes.get(id),
        }
    }
}

/// Quoin's scene `tone` selects a canonical button; its design cell owns
/// both halves of the pair. Scene `background`, `hover` and `color` ports do
/// not override a button (they belong to rows/columns and text).
pub(crate) fn button_key(node: &Node, interaction: InteractionState) -> ButtonCellKey {
    let variant = match port_text(node, "tone") {
        "primary" => ButtonVariant::Primary,
        "danger" => ButtonVariant::Destructive,
        _ => ButtonVariant::default(),
    };
    ButtonCellKey {
        variant,
        size: ButtonSize::default(),
        interaction,
        focus_visible: false,
    }
}

/// Optional host metadata: `window: { ..., "autofocus": "search" }` opts an
/// edge into keyboard focus when its reveal finishes, including pointer reveals.
/// This explicit opt-in extends §4.3's usual pointer-does-not-focus rule.
pub fn autofocus(tree: &ResolvedScene) -> Option<&str> {
    let id = tree.window.as_ref()?.get("autofocus")?.as_str()?;
    fn visible(tree: &ResolvedScene, id: &str, target: &str, depth: usize) -> bool {
        if depth > tree.nodes.len() {
            return false;
        }
        let Some(node) = tree.nodes.get(id) else {
            return false;
        };
        !flag(node, "hidden")
            && ((id == target && node.family == "field")
                || children(node).any(|child| visible(tree, child, target, depth + 1)))
    }
    visible(tree, "root", id, 0).then_some(id)
}

/// The egui id of a document field, for focus.
pub fn field_id(id: &str) -> egui::Id {
    egui::Id::new(("scene-field", id))
}

/// The embedded design a scene draws with before the session look arrives:
/// an edge page in dark mode (Quoin's panel token), a dialog as the design
/// selects.
fn embedded(dialog: bool) -> Arc<Theme> {
    use std::sync::OnceLock;
    static EDGE: OnceLock<Arc<Theme>> = OnceLock::new();
    static DIALOG: OnceLock<Arc<Theme>> = OnceLock::new();
    if dialog {
        DIALOG.get_or_init(|| Arc::new(Theme::embedded())).clone()
    } else {
        EDGE.get_or_init(|| {
            Arc::new(Theme::for_context(design::DesignContext {
                mode: design::Mode::Dark,
                ..design::DesignContext::default()
            }))
        })
        .clone()
    }
}

/// One scene surface's program.
pub struct SceneUi {
    content: Arc<Content>,
    look: Option<Arc<Look>>,
    /// Local text of the fields being edited, by instance key, until the
    /// scene's own `value` port changes.
    edits: BTreeMap<String, String>,
    /// Local toggle states, likewise.
    toggles: BTreeMap<String, bool>,
    images: crate::images::Prepared,
    /// Uploaded icons by (asset, pixel width, pixel height).
    textures: BTreeMap<(u64, u32, u32), egui::TextureHandle>,
    /// The latest pass's layout, as drawn.
    layout: SceneLayout,
    /// Each scrolling list's offset (points from its top).
    scroll: BTreeMap<String, f32>,
    /// The look installed on the context: (look identity, dialog).
    installed: Option<(usize, bool)>,
    /// The surface's keyboard focus as last seen (edges report changes).
    focused: Option<bool>,
    /// The hover tip's state between passes.
    hover: tip::Hover,
}

impl SceneUi {
    pub fn new(content: Arc<Content>) -> SceneUi {
        SceneUi {
            hover: tip::Hover::default(),
            images: crate::images::prepare(&content.tree, &content.lists),
            content,
            look: None,
            edits: BTreeMap::new(),
            toggles: BTreeMap::new(),
            textures: BTreeMap::new(),
            layout: SceneLayout::default(),
            scroll: BTreeMap::new(),
            installed: None,
            focused: None,
        }
    }

    pub(crate) fn with_look(content: Arc<Content>, look: Arc<Look>) -> SceneUi {
        let mut ui = Self::new(content);
        ui.look = Some(look);
        ui
    }

    fn theme(&self) -> Arc<Theme> {
        match &self.look {
            Some(look) => look.theme.clone(),
            None => embedded(self.content.dialog),
        }
    }

    fn text_scale(&self) -> f32 {
        self.look.as_ref().map_or(1.0, |look| look.text_scale)
    }

    /// The scene palette: the look's, or the embedded design's.
    pub(crate) fn palette(&self) -> decor::Palette {
        crate::appearance::page(&self.theme(), self.content.dialog)
    }

    /// The opaque page: the secondary pair's surface for an edge, the base
    /// pair's for a dialog. `shell.scene.layout` reports it as `page`.
    pub fn page(&self) -> Color32 {
        colour(self.palette().base.surface)
    }

    /// The matching design foreground for text that declares none.
    pub(crate) fn ink(&self) -> Color32 {
        colour(self.palette().base.foreground)
    }

    pub(crate) fn apply_appearance(&mut self, look: Arc<Look>) {
        self.look = Some(look);
        self.installed = None;
    }

    /// The text metrics layout and paint use: the design's ui and mono roles
    /// at the look's text scale (Quoin's 13 px where the design names none).
    pub(crate) fn text_styles(&self) -> flex::TextStyles {
        let theme = self.theme();
        let scale = self.text_scale();
        let role = |role| {
            let record = design::active_typography(Some(theme.typography()), role);
            flex::TextStyle {
                size: if record.font_size > 0.0 {
                    record.font_size as f32 * scale
                } else {
                    TEXT_SIZE * scale
                },
                line_height: record.line_height.map(|h| h as f32 * scale),
            }
        };
        flex::TextStyles {
            ui: role(design::TypographyRole::Ui),
            mono: role(design::TypographyRole::Mono),
        }
    }

    /// The revision this UI last applied (`applied_revision`).
    pub fn revision(&self) -> u64 {
        self.content.revision
    }

    /// The latest pass's layout (scroll offsets not applied).
    pub(crate) fn layout(&self) -> &SceneLayout {
        &self.layout
    }

    /// How far list `list` is scrolled.
    pub(crate) fn scrolled(&self, list: &str) -> f32 {
        self.scroll.get(list).copied().unwrap_or(0.0)
    }

    /// Focus document field `id` if the current layout draws it; the next
    /// pass applies it. Whether the field was there.
    pub fn focus_field(&self, ctx: &egui::Context, id: &str) -> bool {
        let drawn = self.layout.nodes.contains_key(id)
            && self
                .content
                .tree
                .nodes
                .get(id)
                .is_some_and(|node| node.family == "field");
        if drawn {
            ctx.memory_mut(|memory| memory.request_focus(field_id(id)));
        }
        drawn
    }

    /// Which instance keys' local state a new revision overrides: a field
    /// or toggle whose authored `value` changed, and every list row instance.
    fn forget_overridden(&mut self, next: &Content) {
        let previous = &self.content.tree;
        let changed = |id: &str| {
            next.tree.nodes.get(id).and_then(|n| n.ports.get("value"))
                != previous.nodes.get(id).and_then(|n| n.ports.get("value"))
        };
        let node_of = |key: &str| key.rsplit('/').next().unwrap_or_default().to_owned();
        let row_changed = |key: &str| key.contains('#');
        self.edits
            .retain(|key, _| !row_changed(key) && !changed(&node_of(key)));
        self.toggles
            .retain(|key, _| !row_changed(key) && !changed(&node_of(key)));
    }
}

impl ui::UiProgram for SceneUi {
    type Message = SceneMessage;

    fn setup(&mut self, ctx: &egui::Context) {
        // Before the first pass: egui binds new fonts from the next pass on,
        // and the first pass already measures text in the named families.
        let identity = self
            .look
            .as_ref()
            .map_or(0, |look| Arc::as_ptr(look) as usize);
        crate::appearance::install(ctx, &self.theme(), self.text_scale(), self.content.dialog);
        self.installed = Some((identity, self.content.dialog));
    }

    fn show(&mut self, ui: &mut egui::Ui, emit: &mut Vec<SceneMessage>) {
        let ctx = ui.ctx().clone();
        let identity = self
            .look
            .as_ref()
            .map_or(0, |look| Arc::as_ptr(look) as usize);
        if self.installed != Some((identity, self.content.dialog)) {
            crate::appearance::install(&ctx, &self.theme(), self.text_scale(), self.content.dialog);
            self.installed = Some((identity, self.content.dialog));
        }
        // Escape: a dialog hides (Quoin's chrome::dialog), an edge reports it.
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            emit.push(if self.content.dialog {
                SceneMessage::Close
            } else {
                SceneMessage::EscapeEdge
            });
        }
        if !self.content.dialog {
            let focused = ctx.input(|input| input.focused);
            if self.focused.is_some_and(|was| was != focused) {
                emit.push(SceneMessage::EdgeFocus(focused));
            }
            self.focused = Some(focused);
        }
        let styles = self.text_styles();
        let measure_ctx = ctx.clone();
        let measure = move |text: &str, size: f32, mono: bool, bold: bool| -> egui::Vec2 {
            let font = paint::font(size, mono, bold);
            measure_ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(text.to_owned(), font, Color32::WHITE)
                    .size()
            })
        };
        self.layout = flex::layout(
            &self.content,
            ui.max_rect().size(),
            styles,
            &self.images,
            &measure,
        );
        paint::paint(self, ui, emit);
    }

    fn update(&mut self, message: SceneMessage) {
        match message {
            SceneMessage::Replace(content) => {
                self.forget_overridden(&content);
                self.images = crate::images::prepare(&content.tree, &content.lists);
                if self.content.dialog != content.dialog {
                    self.installed = None;
                }
                self.content = content;
            }
            SceneMessage::Appearance(look) => self.apply_appearance(look),
            SceneMessage::Input(_, key, value) => {
                self.edits.insert(key, value);
            }
            SceneMessage::Toggle(_, key, on) => {
                self.toggles.insert(key, on);
            }
            SceneMessage::Click(_)
            | SceneMessage::Submit(..)
            | SceneMessage::Close
            | SceneMessage::EscapeEdge
            | SceneMessage::EdgeFocus(_)
            | SceneMessage::Tooltip(_) => {}
            SceneMessage::ForgetTip => self.hover = tip::Hover::default(),
        }
    }
}

/// The Bus event a message asks for, if its node has a handler for it:
/// `(citizen, handler, body)`.
pub fn event_of(message: &SceneMessage) -> Option<(&str, &str, Value)> {
    match message {
        SceneMessage::Click(route) => route.event("click", None),
        SceneMessage::Toggle(route, _, on) => route.event("change", Some(json!(on))),
        SceneMessage::Input(route, _, value) => route.event("change", Some(json!(value))),
        SceneMessage::Submit(route, value) => route.event("submit", Some(json!(value))),
        SceneMessage::Replace(_)
        | SceneMessage::Appearance(_)
        | SceneMessage::Close
        | SceneMessage::EscapeEdge
        | SceneMessage::EdgeFocus(_)
        | SceneMessage::Tooltip(_)
        | SceneMessage::ForgetTip => None,
    }
}

#[cfg(test)]
mod tests;
