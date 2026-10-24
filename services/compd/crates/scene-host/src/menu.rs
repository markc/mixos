// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Compositor-owned corner menu. Mode choices use the panel core's SetMode;
//! extras use the host's existing Bus worker, never a separate transport.

use std::path::Path;
use std::sync::Arc;

use config::{Value as MixValue, parse as parse_mix_data};
use edges::{Corner, PanelMode};
use serde_json::{Value, json};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface};
use smithay::utils::{Physical, Rectangle};
use ui::UiProgram;
use ui::{HandleId, UiHandle};
use world::scene::layer::base::Layer;
use world::state::Loop;
use world::surface::draw::handle::{UiSpace, load};

/// Underscores are outside the authored scene-name grammar: no id collision.
pub const SCENE: &str = "__corner_menu";
const ROW_HEIGHT: f32 = 32.0;
const PADDING: f32 = 4.0;

pub fn corner_name(corner: Corner) -> &'static str {
    match corner {
        Corner::TopLeft => "top-left",
        Corner::BottomLeft => "bottom-left",
        Corner::BottomRight => "bottom-right",
        Corner::TopRight => "top-right",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Extra {
    pub label: String,
    pub target: String,
    pub kind: String,
    pub args: Vec<String>,
    pub confirm: Option<String>,
}

/// Quoin config.rs's menu_items schema. Only this section is interpreted.
pub fn read_extras(path: &Path, edge: &str) -> Result<Vec<Extra>, String> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let value = parse_mix_data(&source).map_err(|error| error.to_string())?;
    let invalid = || {
        "invalid conf.mix menu_items (expected edge lists of label/target/command/args/confirm)"
            .to_owned()
    };
    let MixValue::Map(root) = &value else {
        return Err(invalid());
    };
    let Some(menus) = root.get("menu_items") else {
        return Ok(Vec::new());
    };
    let MixValue::Map(menus) = menus else {
        return Err(invalid());
    };
    if menus
        .keys()
        .any(|name| crate::panels::parse_edge(name).is_none())
    {
        return Err(invalid());
    }
    let mut chosen = Vec::new();
    for (name, items) in menus.iter() {
        let MixValue::List(items) = items else {
            return Err(invalid());
        };
        for item in items.iter() {
            let MixValue::Map(item) = item else {
                return Err(invalid());
            };
            if item.keys().any(|key| {
                !["label", "target", "command", "args", "confirm"].contains(&key.as_str())
            }) {
                return Err(invalid());
            }
            let string = |key: &str| match item.get(key) {
                Some(MixValue::String(s))
                    if !s.trim().is_empty() && !s.chars().any(char::is_control) =>
                {
                    Ok(s.clone())
                }
                _ => Err(invalid()),
            };
            let (label, target, kind) = (string("label")?, string("target")?, string("command")?);
            let identifier = |s: &str| {
                !s.is_empty()
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
            };
            if !identifier(&target) || !identifier(&kind) {
                return Err(invalid());
            }
            let args = match item.get("args") {
                None => Vec::new(),
                Some(MixValue::List(args)) => args
                    .iter()
                    .map(|arg| match arg {
                        MixValue::String(arg)
                            if !arg.trim().is_empty() && !arg.chars().any(char::is_control) =>
                        {
                            Ok(arg.clone())
                        }
                        _ => Err(invalid()),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(invalid()),
            };
            let confirm = item.get("confirm").map(|_| string("confirm")).transpose()?;
            // Quoin's QUESTION_MAX_CHARS: 24 * 3 - 4.
            if confirm.as_ref().is_some_and(|s| s.chars().count() > 68) {
                return Err(invalid());
            }
            if name == edge {
                chosen.push(Extra {
                    label,
                    target,
                    kind,
                    args,
                    confirm,
                });
            }
        }
    }
    // TODO (§8.4): scene.mix-authored menu content. Quoin currently reads
    // menu_items from conf.mix only; no scene.mix menu schema to mirror.
    Ok(chosen)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    Mode(PanelMode),
    Extra(Extra),
    Cancel,
    Inert,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub label: String,
    pub checked: bool,
    pub disabled: bool,
    pub choice: Choice,
}

impl Item {
    pub fn enabled(&self) -> bool {
        !self.checked && !self.disabled && self.choice != Choice::Inert
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub serial: u64,
    pub output: String,
    pub corner: Corner,
    pub items: Vec<Item>,
    pub selected: usize,
    pub rect: (f32, f32, f32, f32),
    pub inset: f32,
}

impl Menu {
    pub fn new(
        serial: u64,
        output: &str,
        corner: Corner,
        mode: PanelMode,
        extras: Vec<Extra>,
        size: (f32, f32),
        inset: f32,
    ) -> Self {
        let mut items: Vec<_> = [
            ("Pin", PanelMode::Pinned),
            ("Dock", PanelMode::Docked),
            ("Hide", PanelMode::Hidden),
        ]
        .into_iter()
        .map(|(label, item)| Item {
            label: label.into(),
            checked: item == mode,
            disabled: false,
            choice: Choice::Mode(item),
        })
        .collect();
        items.extend(extras.into_iter().map(|extra| Item {
            label: extra.label.clone(),
            checked: false,
            disabled: false,
            choice: Choice::Extra(extra),
        }));
        let mut menu = Self {
            serial,
            output: output.into(),
            corner,
            items,
            selected: 0,
            rect: (0.0, 0.0, 0.0, 0.0),
            inset,
        };
        menu.fit(size, inset);
        menu.selected = menu.items.iter().position(Item::enabled).unwrap_or(0);
        menu
    }

    pub fn fit(&mut self, (width, height): (f32, f32), inset: f32) {
        self.inset = inset;
        let w = self
            .items
            .iter()
            .map(|item| item.label.chars().count())
            .max()
            .unwrap_or(0) as f32
            * 8.0
            + 48.0;
        let w = w.max(160.0).min((width - inset * 2.0).max(1.0));
        let h = (self.items.len() as f32 * ROW_HEIGHT + PADDING * 2.0)
            .min((height - inset * 2.0).max(1.0));
        let right = matches!(self.corner, Corner::TopRight | Corner::BottomRight);
        let bottom = matches!(self.corner, Corner::BottomLeft | Corner::BottomRight);
        self.rect = (
            if right {
                (width - inset - w).max(0.0)
            } else {
                inset.min(width - w).max(0.0)
            },
            if bottom {
                (height - inset - h).max(0.0)
            } else {
                inset.min(height - h).max(0.0)
            },
            w,
            h,
        );
    }

    pub fn contains(&self, output: &str, x: f32, y: f32) -> bool {
        let (mx, my, w, h) = self.rect;
        output == self.output && x >= mx && y >= my && x < mx + w && y < my + h
    }

    pub fn navigate(&mut self, direction: i32) {
        for _ in 0..self.items.len() {
            self.selected =
                (self.selected as i32 + direction).rem_euclid(self.items.len() as i32) as usize;
            if self.items[self.selected].enabled() {
                break;
            }
        }
    }

    pub fn snapshot(&self) -> Value {
        json!({"serial":self.serial, "output":self.output, "corner":corner_name(self.corner),
            "edge":self.corner.summoned_edge().as_str(), "selected":self.selected,
            "x":self.rect.0, "y":self.rect.1, "width":self.rect.2, "height":self.rect.3,
            "items":self.items.iter().enumerate().map(|(index, item)| json!({
                "index":index, "label":item.label, "checked":item.checked, "enabled":item.enabled(),
                "mode":match &item.choice { Choice::Mode(mode) => Some(mode.as_str()), _ => None },
            })).collect::<Vec<_>>()})
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Move(i32),
    /// Resolve selection on the host after any queued arrow events.
    Activate,
    Choose(usize),
    Close,
}

#[derive(Clone, Debug)]
pub enum Message {
    Replace(Menu),
    Appearance(Arc<crate::appearance::Look>),
    Input(u64, Input),
}

struct MenuUi {
    menu: Menu,
    look: Option<Arc<crate::appearance::Look>>,
    /// The look installed on the context (its identity).
    installed: Option<usize>,
}

impl MenuUi {
    fn new(menu: Menu) -> Self {
        Self {
            menu,
            look: None,
            installed: None,
        }
    }

    fn theme(&self) -> Arc<toolkit::theme::Theme> {
        match &self.look {
            Some(look) => look.theme.clone(),
            None => Arc::new(toolkit::theme::Theme::embedded()),
        }
    }

    fn apply_appearance(&mut self, look: Arc<crate::appearance::Look>) {
        self.look = Some(look);
        self.installed = None;
    }
}

impl UiProgram for MenuUi {
    type Message = Message;

    fn setup(&mut self, _ctx: &egui::Context) {}

    fn show(&mut self, ui: &mut egui::Ui, emit: &mut Vec<Message>) {
        let ctx = ui.ctx().clone();
        let identity = self
            .look
            .as_ref()
            .map_or(0, |look| Arc::as_ptr(look) as usize);
        let scale = self.look.as_ref().map_or(1.0, |look| look.text_scale);
        let theme = self.theme();
        if self.installed != Some(identity) {
            crate::appearance::install(&ctx, &theme, scale, true);
            self.installed = Some(identity);
        }
        let serial = self.menu.serial;
        let keys = ctx.input(|input| {
            [
                (
                    input.key_pressed(egui::Key::ArrowUp)
                        || input.key_pressed(egui::Key::ArrowLeft),
                    Input::Move(-1),
                ),
                (
                    input.key_pressed(egui::Key::ArrowDown)
                        || input.key_pressed(egui::Key::ArrowRight),
                    Input::Move(1),
                ),
                (input.key_pressed(egui::Key::Enter), Input::Activate),
                (input.key_pressed(egui::Key::Escape), Input::Close),
            ]
        });
        for (pressed, input) in keys {
            if pressed {
                emit.push(Message::Input(serial, input));
            }
        }
        let palette = crate::appearance::page(&theme, true);
        let base = crate::view::colour(palette.base.surface);
        let ink = crate::view::colour(palette.base.foreground);
        let border = crate::view::colour(palette.border);
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, base);
        ui.painter().rect_stroke(
            full,
            0.0,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        let size = design::active_typography(Some(theme.typography()), design::TypographyRole::Ui)
            .font_size as f32
            * scale;
        let size = if size > 0.0 {
            size
        } else {
            crate::view::TEXT_SIZE * scale
        };
        let inner = full.shrink(PADDING);
        egui::ScrollArea::vertical().show_viewport(ui, |ui, _viewport| {
            let (rows, _) = ui.allocate_exact_size(
                egui::vec2(inner.width(), ROW_HEIGHT * self.menu.items.len() as f32),
                egui::Sense::hover(),
            );
            for (index, item) in self.menu.items.iter().enumerate() {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(rows.left(), rows.top() + index as f32 * ROW_HEIGHT),
                    egui::vec2(rows.width(), ROW_HEIGHT),
                );
                let enabled = item.enabled();
                let response = ui.interact(
                    rect,
                    egui::Id::new(("corner-menu", index)),
                    if enabled {
                        egui::Sense::click()
                    } else {
                        egui::Sense::hover()
                    },
                );
                let lit = enabled && (self.menu.selected == index || response.hovered());
                let pair = if lit { palette.primary } else { palette.base };
                ui.painter()
                    .rect_filled(rect, 0.0, crate::view::colour(pair.surface));
                let label = if item.checked {
                    format!("✓  {}", item.label)
                } else {
                    format!("    {}", item.label)
                };
                let colour = if lit {
                    crate::view::colour(pair.foreground)
                } else {
                    ink
                };
                ui.painter().text(
                    egui::pos2(rect.left() + 8.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    label,
                    egui::FontId::proportional(size),
                    if enabled {
                        colour
                    } else {
                        colour.gamma_multiply(0.5)
                    },
                );
                if response.clicked() {
                    emit.push(Message::Input(serial, Input::Choose(index)));
                }
            }
        });
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::Replace(menu) => self.menu = menu,
            Message::Appearance(look) => self.apply_appearance(look),
            Message::Input(serial, Input::Move(direction)) if serial == self.menu.serial => {
                self.menu.navigate(direction.signum())
            }
            _ => {}
        }
    }
}

pub(crate) struct Surface {
    pub handle: HandleId,
    world: u128,
    menu: Menu,
    rect: Rectangle<i32, Physical>,
    factor: f32,
    prior_ui: Option<HandleId>,
    prior_client: Option<WlSurface>,
    appearance_generation: u64,
}

impl Surface {
    pub(crate) fn input_geometry(&self) -> (HandleId, Rectangle<i32, Physical>, f32) {
        (self.handle, self.rect, self.factor)
    }
}

/// Shares the scene host's registry, scale, output affinity and action channel.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reconcile(
    panels: &mut crate::panels::Panels,
    live: &mut Option<Surface>,
    _palette: decor::Palette,
    prepared: Option<Arc<crate::appearance::Look>>,
    appearance_generation: u64,
    wiring: &crate::render::Wiring,
    state: &mut Loop,
    renderer: &mut GlesRenderer,
    restack: bool,
) {
    let world = state.inner.worlds.spawn_target().as_u128();
    let output = state.inner.current_output().name();
    let Some(registry) = state.inner.surface().registry.as_ref() else {
        return;
    };
    if let Some(surface) = live.as_ref() {
        if surface.world == world
            && registry.contains(surface.handle)
            && registry.keyboard_focus() != Some(surface.handle)
        {
            panels.close_menu();
            state
                .state
                .schedule_redraw(dispatcher::state::RedrawReason::Publish);
        }
        let gone = surface.world != world
            || !registry.contains(surface.handle)
            || panels.menu().is_none_or(|menu| {
                menu.serial != surface.menu.serial || menu.output != surface.menu.output
            });
        if gone {
            let surface = live.take().expect("live menu");
            if surface.world == world {
                let registry = state
                    .inner
                    .surface_mut()
                    .registry
                    .as_mut()
                    .expect("registry");
                // Replacement/confirmation keeps the grab, including when
                // a new dialog was mapped in the same reconcile pass.
                let held = registry.keyboard_focus() == Some(surface.handle);
                registry.destroy_by_id(surface.handle);
                if held {
                    let prior = surface.prior_ui.filter(|id| registry.contains(*id));
                    registry.set_keyboard_focus(prior);
                    if prior.is_none()
                        && let Some(keyboard) = state.state.seat.seat.get_keyboard()
                        && keyboard.current_focus().is_none()
                        && let Some(prior) =
                            surface.prior_client.filter(|surface| surface.is_alive())
                    {
                        keyboard.set_focus(
                            &mut state.state,
                            Some(prior),
                            smithay::utils::SERIAL_COUNTER.next_serial(),
                        );
                    }
                }
            }
        }
    }
    let Some(menu) = panels.menu().filter(|menu| menu.output == output).cloned() else {
        return;
    };
    let factor = state
        .inner
        .current_output()
        .current_scale()
        .fractional_scale()
        .max(0.1) as f32;
    let rect = crate::render::physical(menu.rect, f64::from(factor));
    if let Some(surface) = live.as_mut() {
        let registry = state
            .inner
            .surface_mut()
            .registry
            .as_mut()
            .expect("registry");
        if surface.rect.size != rect.size || surface.factor != factor {
            registry.request_resize_scaled_by_id(surface.handle, rect.size, factor);
        }
        if surface.rect.loc != rect.loc {
            registry.set_location_by_id(surface.handle, rect.loc);
        }
        if surface.menu != menu {
            let _ = registry.dispatch_message(
                UiHandle::<MenuUi>::from_id(surface.handle),
                Message::Replace(menu.clone()),
            );
        }
        if surface.appearance_generation != appearance_generation
            && let Some(prepared) = &prepared
        {
            let _ = registry.dispatch_message(
                UiHandle::<MenuUi>::from_id(surface.handle),
                Message::Appearance(Arc::clone(prepared)),
            );
            surface.appearance_generation = appearance_generation;
        }
        // Newly mapped furniture must stay below the menu. Static frames
        // leave newer compositor overlays (such as capture) above it.
        if restack {
            registry.raise(surface.handle);
        }
        surface.menu = menu;
        surface.rect = rect;
        surface.factor = factor;
        return;
    }
    let prior_ui = state
        .inner
        .surface()
        .registry
        .as_ref()
        .and_then(|registry| registry.keyboard_focus());
    let keyboard = state.state.seat.seat.get_keyboard();
    let prior_client = keyboard
        .as_ref()
        .and_then(|keyboard| keyboard.current_focus());
    if let Some(keyboard) = keyboard {
        keyboard.set_focus(
            &mut state.state,
            None,
            smithay::utils::SERIAL_COUNTER.next_serial(),
        );
    }
    let output_key = state.inner.current_output_key();
    let handle = load(
        state,
        renderer,
        {
            let mut ui = MenuUi::new(menu.clone());
            if let Some(prepared) = &prepared {
                ui.apply_appearance(Arc::clone(prepared));
            }
            ui
        },
        rect,
        UiSpace::Screen,
        Layer::SCENE.bits(),
    );
    if let Some(registry) = state.inner.surface_mut().registry.as_mut() {
        registry.request_resize_scaled_by_id(handle.id, rect.size, factor);
        registry.set_output_affinity_by_id(handle.id, Some(output_key));
        registry.set_keyboard_focus(Some(handle.id));
        let (actions, waker) = (wiring.actions.clone(), wiring.waker.clone());
        registry.set_message_handler(handle, move |message: &Message| {
            if let Message::Input(serial, input) = message {
                let _ = actions.send(crate::render::Action::Menu(*serial, input.clone()));
                waker();
            }
        });
    }
    *live = Some(Surface {
        handle: handle.id,
        world,
        menu,
        rect,
        factor,
        prior_ui,
        prior_client,
        appearance_generation,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_preserves_open_menu_selection_and_identity() {
        let menu = Menu::new(
            42,
            "output",
            Corner::TopLeft,
            PanelMode::Pinned,
            Vec::new(),
            (800.0, 600.0),
            4.0,
        );
        let mut ui = MenuUi::new(menu);
        ui.update(Message::Input(42, Input::Move(1)));
        let old = ui.menu.clone();
        ui.update(Message::Appearance(crate::appearance::fixture(
            "light", 1.0,
        )));
        let light = crate::appearance::page(&ui.theme(), true).base;
        ui.update(Message::Appearance(crate::appearance::fixture("dark", 1.5)));
        assert_eq!(ui.menu, old, "a new look keeps the menu and its selection");
        assert_ne!(crate::appearance::page(&ui.theme(), true).base, light);
        assert_eq!(ui.look.as_ref().unwrap().text_scale, 1.5);
    }

    #[test]
    fn extras_follow_quoins_conf_mix_schema_and_refuse_malformed_items() {
        let path = std::env::temp_dir().join(format!("compd-menu-conf-{}.mix", std::process::id()));
        std::fs::write(&path, r#"{menu_items: {left: [{label: "Tools", target: "tools", command: "tools.open", args: ["main"], confirm: "Open tools?"}]}}"#).unwrap();
        let extras = read_extras(&path, "left").unwrap();
        assert_eq!(extras.len(), 1);
        assert_eq!(
            (
                &extras[0].target,
                &extras[0].kind,
                extras[0].args.as_slice()
            ),
            (
                &"tools".to_owned(),
                &"tools.open".to_owned(),
                ["main".to_owned()].as_slice()
            )
        );
        assert_eq!(extras[0].confirm.as_deref(), Some("Open tools?"));
        assert!(read_extras(&path, "right").unwrap().is_empty());
        std::fs::write(&path, r#"{menu_items: {left: [{label: "Tools"}]}}"#).unwrap();
        assert!(read_extras(&path, "left").is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mode_rows_are_checked_navigation_skips_them_and_all_corners_fit() {
        for corner in Corner::ALL {
            let mut menu = Menu::new(
                1,
                "DP-1",
                corner,
                PanelMode::Hidden,
                Vec::new(),
                (1280.0, 800.0),
                12.0,
            );
            assert_eq!(
                menu.items
                    .iter()
                    .map(|item| item.label.as_str())
                    .collect::<Vec<_>>(),
                ["Pin", "Dock", "Hide"]
            );
            assert!(menu.items[2].checked && !menu.items[2].enabled());
            menu.navigate(-1);
            assert_eq!(menu.selected, 1);
            menu.navigate(1);
            assert_eq!(menu.selected, 0);
            let (x, y, w, h) = menu.rect;
            assert!(x >= 12.0 && y >= 12.0 && x + w <= 1268.0 && y + h <= 788.0);
            assert!(menu.contains("DP-1", x, y));
            assert!(!menu.contains("DP-2", x, y));
        }
    }
}
