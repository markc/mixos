// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The menu group, title, Action Bar and captions share the design title band
//! layout with SSD. The mark stays with the menu. Registered SSD apps may
//! expose their Action Bar through the drive for compd to host.

use crate::action::{Origin, Registry};
use crate::button::{IconButton, Link};
use crate::chrome::Chrome;
use crate::icons::{self, Icon};
use crate::strings::Strings;
pub mod hosting;
use design::{CaptionSide, Decorations};
use egui::emath::GuiRounding;
use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align, CursorIcon, Frame, Layout, Pos2, Rect, ResizeDirection, Role, Sense, Stroke, StrokeKind,
    Ui, UiBuilder, ViewportBuilder, ViewportCommand, WidgetInfo, pos2, vec2,
};

fn label(key: &str) -> String {
    crate::strings::own(key)
}

/// An undecorated window for a MixOS app: the title bar is drawn by [`show`].
pub fn viewport(app_id: &str, title: &str) -> ViewportBuilder {
    // eframe does not copy its native root builder into egui's root state.
    // Keep the first (application) id for services called from that context.
    let _ = APP_ID.set(app_id.to_owned());
    ViewportBuilder::default()
        .with_min_inner_size(crate::window::MIN_SIZE)
        .with_app_id(app_id)
        .with_title(title)
        .with_decorations(false)
        .with_transparent(true)
}

static APP_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub(crate) fn app_id() -> Option<String> {
    APP_ID.get().cloned()
}

fn maximized(ui: &Ui) -> bool {
    ui.input(|i| i.viewport().maximized.unwrap_or(false))
}

fn free_gaps(bar: Rect, occupied: &[Rect]) -> Vec<Rect> {
    let mut occupied: Vec<_> = occupied
        .iter()
        .map(|rect| rect.intersect(bar))
        .filter(|rect| rect.width() > 0.0)
        .collect();
    occupied.sort_by(|a, b| a.left().total_cmp(&b.left()));
    let mut left = bar.left();
    let mut gaps = Vec::new();
    for rect in occupied {
        if left < rect.left() {
            gaps.push(Rect::from_min_max(
                pos2(left, bar.top()),
                pos2(rect.left(), bar.bottom()),
            ));
        }
        left = left.max(rect.right());
    }
    if left < bar.right() {
        gaps.push(Rect::from_min_max(
            pos2(left, bar.top()),
            bar.right_bottom(),
        ));
    }
    gaps
}

/// The title bar's height: 32 pt in Pro, 38 in Studio and Classic, one
/// control plus an item gap above and below in the hue schemes.
pub fn height(ui: &Ui) -> f32 {
    Chrome::of(ui.ctx()).metrics.title_bar_height
}

/// What a caption button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Caption {
    Minimize,
    Maximize,
    Close,
}

/// One control of the right-hand group (§3.1), which runs right to left
/// from the caption buttons in the order given. Icons and links run
/// registry commands, so their labels, tooltips and enablement come from
/// the registry like the menus'.
pub enum Control<'a> {
    /// A dropdown, [`COMBO_WIDTH`] wide, shrinking to its minimum before
    /// anything else gives way (a workspace switcher).
    Combo {
        id: &'a str,
        label: &'a str,
        selected: &'a mut usize,
        options: &'a [String],
    },
    /// A 28 pt icon button; `selected` shows it toggled on.
    Icon {
        command: &'static str,
        icon: Icon,
        selected: bool,
    },
    /// A frameless text link with a 14 pt icon.
    Link { command: &'static str, icon: Icon },
}

/// The theme toggle's icon: a sun while the theme is dark, else a moon.
pub fn theme_icon(dark: bool) -> Icon {
    if dark { Icon::Sun } else { Icon::Moon }
}

/// A control's room in the group, for [`fit`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slot {
    Combo,
    /// An icon or link this wide.
    Fixed(f32),
    Link(f32),
}

/// Which controls of the group fit in `free` points: the combo's width and
/// how many controls show, nearest the caption buttons first. Controls drop
/// from the far end (§3.1: the link, then the theme toggle, then search),
/// the combo shrinks to its minimum before any does, and a link shows only
/// while [`LINK_ROOM`] stays free besides it.
pub fn fit(free: f32, slots: &[Slot]) -> (f32, usize) {
    fit_with_tokens(free, slots, &crate::style::default_tokens())
}

fn fit_with_tokens(free: f32, slots: &[Slot], tokens: &design::ResolvedStyle) -> (f32, usize) {
    let need = |n: usize, combo: f32| {
        let width: f32 = slots[..n]
            .iter()
            .map(|s| {
                tokens.title_control_spacing as f32
                    + match s {
                        Slot::Combo => combo,
                        Slot::Fixed(w) | Slot::Link(w) => *w,
                    }
            })
            .sum();
        let link = slots[..n].iter().any(|s| matches!(s, Slot::Link(_)));
        width
            + if link {
                tokens.title_link_room as f32
            } else {
                0.0
            }
    };
    let mut shown = slots.len();
    while shown > 0
        && need(
            shown,
            egui::Rangef {
                min: tokens.title_combo_min_width as f32,
                max: tokens.title_combo_max_width as f32,
            }
            .min,
        ) > free
    {
        shown -= 1;
    }
    let combo = (free - need(shown, 0.0)).clamp(
        egui::Rangef {
            min: tokens.title_combo_min_width as f32,
            max: tokens.title_combo_max_width as f32,
        }
        .min,
        egui::Rangef {
            min: tokens.title_combo_min_width as f32,
            max: tokens.title_combo_max_width as f32,
        }
        .max,
    );
    (combo, shown)
}

/// Draw the title bar at the top of `ui` and return the ids of commands
/// chosen from its menus this frame. `icon` is the app's mark.
pub fn show<S>(
    ui: &mut Ui,
    title: &str,
    icon: Option<Icon>,
    registry: &Registry<S>,
    state: &S,
    strings: &Strings,
) -> Vec<(&'static str, Origin)> {
    show_with(ui, title, icon, registry, state, strings, &mut [])
}

/// [`show`] with controls in the selected actions alignment. Commands run
/// from the controls join the menus'.
pub fn show_with<S>(
    ui: &mut Ui,
    title: &str,
    icon: Option<Icon>,
    registry: &Registry<S>,
    state: &S,
    strings: &Strings,
    controls: &mut [Control<'_>],
) -> Vec<(&'static str, Origin)> {
    crate::menu::anchored_keyboard(ui.ctx());
    crate::macros::keyboard(ui.ctx());
    let chrome = Chrome::of(ui.ctx());
    let (p, m) = (chrome.palette, chrome.metrics);
    let server = chrome.decorations == Decorations::Server;
    request_decorations(ui.ctx(), server);
    let mut fired = Vec::new();
    let height = if server
        && controls.is_empty()
        && crate::menu::effective(ui.ctx()) == design::MenuStyle::Titlebar
    {
        0.0
    } else {
        m.title_bar_height
    };
    let panel = egui::Panel::top("toolkit-titlebar")
        .exact_size(height)
        .frame(Frame::new().fill(p.chrome));
    panel.show(ui, |ui| {
        use design::title_band::{Alignment, Group};

        let bar = ui.max_rect();
        let form = crate::menu::effective(ui.ctx());
        let mark = if !server { m.mark + m.mark_gap } else { 0.0 };
        let menu_width: f32 = match form {
            design::MenuStyle::Bar => registry
                .model(ui.ctx(), state, strings)
                .iter()
                .map(|menu| {
                    ui.painter()
                        .layout_no_wrap(menu.title.clone(), Chrome::menu_font(ui.style()), p.text)
                        .size()
                        .x
                        + 2.0 * m.menu_title_padding.x
                })
                .sum(),
            design::MenuStyle::Button => chrome.style.icon_button as f32,
            design::MenuStyle::Titlebar => 0.0,
        };
        let control_width = |control: &Control<'_>| {
            let width = match control {
                Control::Combo { .. } => chrome.style.title_combo_max_width as f32,
                Control::Icon { .. } => chrome.style.icon_button as f32,
                Control::Link { command, .. } => registry.get(command).map_or(0.0, |c| {
                    Link::width(ui, &crate::action::label_text(strings, c.label))
                }),
            };
            width + chrome.style.title_control_spacing as f32
        };
        let mut actions_width: f32 = controls.iter().map(control_width).sum();
        if controls
            .iter()
            .any(|control| matches!(control, Control::Link { .. }))
        {
            actions_width += chrome.style.title_link_room as f32;
        }
        let actions_min: f32 = controls
            .iter()
            .filter(|c| matches!(c, Control::Icon { command, .. } if ::icons::standard(command)))
            .map(control_width)
            .sum();
        let title_align = chrome
            .band
            .title
            .alignment(chrome.band.style.title_alignment());
        let legacy_center =
            chrome.band.title == design::TitleAlign::Auto && title_align == Alignment::Center;
        let font = crate::fonts::bound(ui.ctx(), crate::fonts::medium(m.title_size));
        let title_width = if server || title.is_empty() {
            0.0
        } else {
            ui.painter()
                .layout_no_wrap(title.into(), font, p.text_dim)
                .size()
                .x
                + 2.0 * m.title_gap
                + if legacy_center {
                    m.title_bar_margin
                } else {
                    0.0
                }
        };
        let captions = if server { 0.0 } else { 3.0 * m.caption_width };
        let caption_align = chrome.captions.alignment(Alignment::Right);
        let menu_align = chrome.menu_side.alignment();
        let actions_align = chrome.band.actions.alignment();
        let menu_pad = if menu_width + mark > 0.0 && menu_align != Alignment::Center {
            m.title_bar_margin
        } else {
            0.0
        };
        let actions_pad = if controls.is_empty() || actions_align == Alignment::Center {
            0.0
        } else if !server && actions_align == caption_align {
            m.caption_gap
        } else {
            m.title_bar_margin
        };
        let groups = design::title_band::layout(
            bar.width(),
            &[
                Group {
                    preferred: captions,
                    minimum: captions,
                    align: caption_align,
                    shrink: None,
                    order: 3,
                },
                Group {
                    preferred: menu_width + mark + menu_pad,
                    minimum: menu_width + mark + menu_pad,
                    align: menu_align,
                    shrink: None,
                    order: 0,
                },
                Group {
                    preferred: actions_width + actions_pad,
                    minimum: actions_min + actions_pad,
                    align: actions_align,
                    shrink: Some(1),
                    order: 2,
                },
                Group {
                    preferred: title_width,
                    minimum: 0.0,
                    align: title_align,
                    shrink: Some(0),
                    order: 1,
                },
            ],
        );
        let group = |i: usize| {
            Rect::from_min_size(
                pos2(bar.left() + groups[i].x, bar.top()),
                vec2(groups[i].width, bar.height()),
            )
        };
        let drag: Vec<_> = if server {
            Vec::new()
        } else {
            // The title only paints, so it stays part of the drag zone
            // (press to move, double-click to maximise).
            free_gaps(bar, &[group(0), group(1), group(2)])
                .into_iter()
                .enumerate()
                .map(|(index, gap)| {
                    ui.interact(
                        gap,
                        ui.scope_id().with(("titlebar-drag", index)),
                        Sense::click_and_drag(),
                    )
                })
                .collect()
        };
        let mut menu = group(1);
        if menu_align == Alignment::Left {
            menu.min.x += menu_pad;
        } else if menu_align == Alignment::Right {
            menu.max.x -= menu_pad;
        }
        // The mark leads, as a window icon does, then the menu (button or bar).
        if mark > 0.0
            && let Some(icon) = icon
        {
            let at = pos2(menu.left(), bar.center().y - m.mark / 2.0);
            let rect = Rect::from_min_size(at, vec2(m.mark, m.mark))
                .round_to_pixels(ui.pixels_per_point());
            icons::image_of(ui.ctx(), icon, m.mark, p.icon).paint_at(ui, rect);
            let response =
                ui.interact(rect, ui.scope_id().with("window-menu-mark"), Sense::click());
            response.widget_info(|| {
                WidgetInfo::labeled(Role::Button, true, label("window-system-menu"))
            });
            if response.clicked_by(egui::PointerButton::Primary) {
                request_window_menu(ui.ctx(), response.interact_pointer_pos());
            }
        }
        menu.min.x += mark;
        let anchor = if form == design::MenuStyle::Button {
            let rect = Rect::from_min_size(menu.min, vec2(menu_width, menu.height()));
            menu.min.x += menu_width;
            menu_button_at(ui, rect)
        } else {
            None
        };
        menus(ui, menu, registry, state, strings, anchor, &mut fired);
        let mut actions = group(2);
        if actions_align == Alignment::Left {
            actions.min.x += actions_pad;
        } else if actions_align == Alignment::Right {
            actions.max.x -= actions_pad;
        }
        right_group(
            ui,
            controls,
            registry,
            state,
            strings,
            actions.left(),
            actions.right(),
            &mut fired,
        );
        if !server {
            let order = if chrome.captions == CaptionSide::Left {
                [Caption::Close, Caption::Minimize, Caption::Maximize]
            } else {
                [Caption::Minimize, Caption::Maximize, Caption::Close]
            };
            for (i, caption) in order.into_iter().enumerate() {
                let rect = Rect::from_min_size(
                    pos2(group(0).left() + i as f32 * m.caption_width, bar.top()),
                    vec2(m.caption_width, bar.height()),
                );
                caption_button(ui, rect, caption, &chrome);
            }
            let slot = group(3);
            window_title(
                ui,
                &chrome,
                title,
                slot,
                slot.left() + m.title_gap,
                slot.right() - m.title_gap,
            );
        }
        for drag in drag {
            if drag.secondary_clicked() {
                request_window_menu(ui.ctx(), drag.interact_pointer_pos());
            } else if drag.double_clicked() {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Maximized(!maximized(ui)));
            } else if drag.drag_started_by(egui::PointerButton::Primary) {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
        }
    });
    fired
}

fn request_window_menu(ctx: &egui::Context, position: Option<Pos2>) {
    if Chrome::of(ctx).decorations == Decorations::Client
        && design::decorations::is_wayland()
        && let Some(position) = position
    {
        // eframe drains this viewport-scoped one-shot after the UI pass.
        let key = egui::Id::unique(("eframe.window-menu", ctx.viewport_id()));
        ctx.data_mut(|data| data.insert_temp(key, position));
    }
}

/// The title bar for a core command table, with the same controls and layout.
#[allow(clippy::too_many_arguments)] // Same rendering contract as show_with.
pub fn show_engine_with<S>(
    ui: &mut Ui,
    title: &str,
    icon: Option<Icon>,
    registry: &crate::action::EngineAdapter<S>,
    state: &S,
    strings: &Strings,
    controls: &mut [Control<'_>],
) -> Vec<(&'static str, Origin)> {
    show_with(ui, title, icon, &registry.ui, state, strings, controls)
}

/// One app-wide action an app names for its icon bar: a registry command
/// and the icon it shows. Its tooltip, enablement and toggled state come
/// from the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    pub command: &'static str,
    pub icon: Icon,
}

impl Action {
    pub const fn new(command: &'static str, icon: Icon) -> Self {
        Self { command, icon }
    }
}

/// One resolved icon of the bar, as drawn and as published to a compositor
/// that hosts the bar in its own title band.
#[derive(Clone, Debug, PartialEq)]
pub struct IconBarItem {
    pub command: &'static str,
    pub icon: Icon,
    pub tooltip: String,
    pub enabled: bool,
    pub toggled: bool,
}

/// The standard tail, left to right: macros, search, the shortcuts sheet, light
/// and dark. Each slot takes the first id the app registers.
const TAIL: &[&[&str]] = ::icons::TAIL;

/// The icon bar, left to right: the app's `actions` that it registers, then
/// the standard tail for the commands it registers. `dark` picks the
/// sun/moon. Every item is an icon; nothing here draws.
pub fn icon_bar<S>(
    ctx: &egui::Context,
    registry: &Registry<S>,
    state: &S,
    strings: &Strings,
    actions: &[Action],
    dark: bool,
) -> Vec<IconBarItem> {
    let tail = TAIL.iter().filter_map(|ids| {
        let id = ids.iter().find(|id| registry.get(id).is_some())?;
        let icon = match *id {
            "help.macros" => Icon::ScrollText,
            "app.runner" | "view.search" => Icon::Search,
            "view.mode" => theme_icon(dark),
            _ => Icon::Keyboard,
        };
        Some(Action::new(registry.get(id)?.id, icon))
    });
    let standard = |id: &str| TAIL.iter().any(|ids| ids.contains(&id));
    actions
        .iter()
        .copied()
        .filter(|a| !standard(a.command))
        .chain(tail)
        .filter_map(|a| {
            let command = registry.get(a.command)?;
            Some(IconBarItem {
                command: command.id,
                icon: a.icon,
                tooltip: crate::tooltip::for_command(ctx, command, strings),
                enabled: registry.enabled(command.id, state),
                toggled: command.id != "view.mode"
                    && registry.checked(command.id, state) == Some(true),
            })
        })
        .collect()
}

/// [`show`] with the app's icon bar ([`icon_bar`]) at the right end of the
/// row. As the row narrows the app's actions give way, leftmost first; the
/// standard tail stays.
pub fn show_bar<S>(
    ui: &mut Ui,
    title: &str,
    icon: Option<Icon>,
    registry: &Registry<S>,
    state: &S,
    strings: &Strings,
    actions: &[Action],
) -> Vec<(&'static str, Origin)> {
    let dark = ui.visuals().dark_mode;
    let items = icon_bar(ui.ctx(), registry, state, strings, actions, dark);
    hosting::publish(ui.ctx(), &items);
    if hosting::hosted(ui.ctx()) {
        return show_with(ui, title, icon, registry, state, strings, &mut []);
    }
    // Fit keeps the tail and drops app actions from the left.
    let mut controls: Vec<Control<'_>> = items
        .iter()
        .rev()
        .map(|item| Control::Icon {
            command: item.command,
            icon: item.icon,
            selected: item.toggled,
        })
        .collect();
    show_with(ui, title, icon, registry, state, strings, &mut controls)
}

/// [`show_bar`] for a core command table.
pub fn show_engine_bar<S>(
    ui: &mut Ui,
    title: &str,
    icon: Option<Icon>,
    registry: &crate::action::EngineAdapter<S>,
    state: &S,
    strings: &Strings,
    actions: &[Action],
) -> Vec<(&'static str, Origin)> {
    show_bar(ui, title, icon, &registry.ui, state, strings, actions)
}

/// The registry's menus laid out left to right in `room`; their commands
/// join `fired`. Returns the menus' right edge.
fn menus<S>(
    ui: &mut Ui,
    room: Rect,
    registry: &Registry<S>,
    state: &S,
    strings: &Strings,
    anchor: Option<Rect>,
    fired: &mut Vec<(&'static str, Origin)>,
) -> f32 {
    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(room)
            .layout(Layout::left_to_right(Align::Center)),
    );
    row.spacing_mut().item_spacing.x = 0.0;
    let chrome = Chrome::of(ui.ctx());
    if crate::menu::effective(ui.ctx()) == design::MenuStyle::Bar {
        fired.extend(registry.menus(&mut row, state, strings));
    } else {
        let anchor = anchor.unwrap_or_else(|| {
            let content = ui.ctx().content_rect();
            let x = match chrome.menu_side {
                design::MenuSide::Left => content.left(),
                design::MenuSide::Center => content.center().x,
                design::MenuSide::Right => content.right(),
            };
            Rect::from_min_size(pos2(x, content.top()), egui::Vec2::ZERO)
        });
        fired.extend(registry.compact_menus(&mut row, state, strings, anchor));
    }
    row.min_rect().right().max(room.left())
}

fn menu_button_at(ui: &mut Ui, rect: Rect) -> Option<Rect> {
    let mut cell = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let response = cell.add(IconButton::new(Icon::Menu).tooltip(label("window-menu")));
    if (response.hovered() && ui.input(|i| i.pointer.primary_pressed()))
        || (response.clicked() && !response.clicked_by(egui::PointerButton::Primary))
    {
        crate::menu::toggle(ui.ctx());
    }
    Some(response.rect)
}

/// Ask the platform for its own decorations, or for none, when that is not
/// what this window last asked for. The window opens undecorated
/// ([`viewport`]), so a client-side window asks nothing.
fn request_decorations(ctx: &egui::Context, server: bool) {
    let viewport = ctx.viewport_id();
    if decorations_changed(ctx, viewport, server) {
        ctx.send_viewport_cmd(ViewportCommand::Decorations(server));
    }
}

/// Record what `viewport` asks for and say whether that differs from its
/// last ask. Each viewport keeps its own answer: a client-side composer
/// beside a server-decorated mailbox must not flip a shared flag, or each
/// pass would re-ask and repaint the other forever.
fn decorations_changed(ctx: &egui::Context, viewport: egui::ViewportId, server: bool) -> bool {
    let id = crate::window_id(viewport, "toolkit.decorations");
    let asked = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    if asked != server {
        ctx.data_mut(|d| d.insert_temp(id, server));
    }
    asked != server
}

/// Lay out and draw the right-hand group, right to left from `right`, in
/// the room down to `left`; push the commands its controls ran. Returns
/// the group's left edge (`right` when nothing shows).
#[expect(
    clippy::too_many_arguments,
    reason = "the title bar's inputs, called from one place"
)]
fn right_group<S>(
    ui: &mut Ui,
    controls: &mut [Control<'_>],
    registry: &Registry<S>,
    state: &S,
    strings: &Strings,
    left: f32,
    right: f32,
    fired: &mut Vec<(&'static str, Origin)>,
) -> f32 {
    if controls.is_empty() {
        return right;
    }
    let viewport = ui.ctx().viewport_id();
    let slots: Vec<Slot> = controls
        .iter()
        .map(|c| match c {
            Control::Combo { .. } => Slot::Combo,
            Control::Icon { .. } => Slot::Fixed(Chrome::of(ui.ctx()).style.icon_button as f32),
            Control::Link { command, .. } => {
                let label = registry
                    .get(command)
                    .map(|c| crate::action::label_text(strings, c.label))
                    .unwrap_or_default();
                Slot::Link(Link::width(ui, &label))
            }
        })
        .collect();
    let (combo_width, shown) = fit_with_tokens(right - left, &slots, &Chrome::of(ui.ctx()).style);
    let bar = ui.max_rect();
    let mut x = right;
    for control in controls.iter_mut().take(shown) {
        let width = match control {
            Control::Combo { .. } => combo_width,
            Control::Icon { .. } => Chrome::of(ui.ctx()).style.icon_button as f32,
            Control::Link { command, .. } => {
                let label = registry
                    .get(command)
                    .map(|c| crate::action::label_text(strings, c.label))
                    .unwrap_or_default();
                Link::width(ui, &label)
            }
        };
        let rect = Rect::from_min_max(pos2(x - width, bar.top()), pos2(x, bar.bottom()));
        let mut cell = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let command = match control {
            Control::Combo {
                id,
                label,
                selected,
                options,
            } => {
                crate::combo::show(&mut cell, *id, selected, options, Some(width))
                    .accessible_name(*label);
                None
            }
            Control::Icon {
                command,
                icon,
                selected,
            } => registry.get(command).map(|c| {
                let tooltip = crate::tooltip::for_command(ui.ctx(), c, strings);
                let button = IconButton::new(*icon).selected(*selected).tooltip(tooltip);
                let response = cell.add_enabled(registry.enabled(c.id, state), button);
                if c.id == "help.macros" {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(crate::window_id(viewport, "macro-anchor"), response.rect)
                    });
                    if ui
                        .ctx()
                        .data(|d| d.get_temp::<bool>(crate::window_id(viewport, "macro-busy")))
                        .unwrap_or(false)
                    {
                        let chrome = Chrome::of(ui.ctx());
                        ui.painter().circle_filled(
                            response.rect.right_top()
                                + egui::vec2(
                                    -chrome.metrics.menu_row_padding.x / 2.0,
                                    chrome.metrics.menu_row_padding.y,
                                ),
                            chrome.style.hairline_width as f32 * 2.0,
                            chrome.palette.accent,
                        );
                    }
                }
                (c, response.clicked())
            }),
            Control::Link { command, icon } => registry.get(command).map(|c| {
                let link = Link::new(*icon, crate::action::label_text(strings, c.label));
                (
                    c,
                    cell.add_enabled(registry.enabled(c.id, state), link)
                        .clicked(),
                )
            }),
        };
        if let Some((command, true)) = command {
            fired.push((command.id, Origin::Ui));
        }
        x -= width
            + crate::chrome::Chrome::of(ui.ctx())
                .style
                .title_control_spacing as f32;
    }
    x + crate::chrome::Chrome::of(ui.ctx())
        .style
        .title_control_spacing as f32
}

/// The window title in Inter Medium, `text_dim`: centred on the bar, slid
/// sideways to stay within `left..right`, elided with "…" from `left` when
/// it cannot fit but the gap is wider than [`MIN_TITLE_ROOM`], else omitted.
fn window_title(ui: &Ui, chrome: &Chrome, title: &str, bar: Rect, left: f32, right: f32) {
    let room = right - left;
    if title.is_empty() || room <= 0.0 {
        return;
    }
    let font = crate::fonts::bound(ui.ctx(), crate::fonts::medium(chrome.metrics.title_size));
    let painter = ui.painter();
    let mut galley =
        painter.layout_no_wrap(title.to_owned(), font.clone(), chrome.palette.text_dim);
    let width = galley.size().x;
    let x = if width <= room {
        let margin = if chrome.band.title == design::TitleAlign::Auto {
            chrome.metrics.title_bar_margin
        } else {
            0.0
        };
        let centre = (bar.left() + margin + bar.right()) / 2.0;
        (centre - width / 2.0).clamp(left, right - width)
    } else if room > crate::chrome::Chrome::of(ui.ctx()).style.title_min_room as f32 {
        let mut job = LayoutJob::simple_singleline(title.to_owned(), font, chrome.palette.text_dim);
        job.wrap = TextWrapping::truncate_at_width(room);
        galley = painter.layout_job(job);
        left
    } else {
        return;
    };
    let at =
        Pos2::new(x, bar.center().y - galley.size().y / 2.0).round_to_pixels(ui.pixels_per_point());
    painter.galley(at, galley, chrome.palette.text_dim);
}

fn caption_button(ui: &mut Ui, rect: Rect, caption: Caption, chrome: &Chrome) {
    let max = maximized(ui);
    let key = match caption {
        Caption::Minimize => "window-minimize",
        Caption::Maximize if max => "window-restore",
        Caption::Maximize => "window-maximize",
        Caption::Close => "window-close",
    };
    let name = label(key);
    let response = ui.interact(
        rect,
        ui.scope_id().with(("toolkit-caption", key)),
        Sense::click(),
    );
    response.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
    let p = &chrome.palette;
    let pressed = response.is_pointer_button_down_on();
    let hovered = response.hovered() || pressed;
    // Flat fills, no animation (§3.2, §4).
    let (fill, glyph) = match caption {
        Caption::Close if pressed => (
            Some(
                p.caption_close
                    .gamma_multiply(chrome.style.close_pressed_alpha as f32),
            ),
            p.caption_close_text,
        ),
        Caption::Close if hovered => (Some(p.caption_close), p.caption_close_text),
        _ if pressed => (Some(p.pressed), p.icon),
        _ if hovered => (Some(p.hover), p.icon),
        _ => (None, p.icon),
    };
    let painter = ui.painter();
    if let Some(fill) = fill {
        painter.rect_filled(rect, Chrome::of(ui.ctx()).style.square_radius, fill);
    }
    let size = chrome.metrics.caption_glyph;
    let b = Rect::from_center_size(rect.center(), vec2(size, size));
    let line = Stroke::new(Chrome::of(ui.ctx()).style.hairline_width as f32, glyph);
    match caption {
        Caption::Minimize => {
            painter.line_segment([b.left_center(), b.right_center()], line);
        }
        Caption::Maximize if max => {
            // The front square (the box less 2 pt at the top and right) and
            // the visible corner of the square behind it.
            let inset = 2.0;
            let front = Rect::from_min_max(
                pos2(b.left(), b.top() + inset),
                pos2(b.right() - inset, b.bottom()),
            );
            painter.rect_stroke(
                front,
                Chrome::of(ui.ctx()).style.square_radius,
                line,
                StrokeKind::Middle,
            );
            let back = [
                pos2(front.left() + inset, front.top()),
                pos2(front.left() + inset, b.top()),
                b.right_top(),
                pos2(b.right(), b.bottom() - inset),
                pos2(front.right(), b.bottom() - inset),
            ];
            painter.add(egui::Shape::line(back.to_vec(), line));
        }
        Caption::Maximize => {
            painter.rect_stroke(
                b,
                Chrome::of(ui.ctx()).style.square_radius,
                line,
                StrokeKind::Middle,
            );
        }
        Caption::Close => {
            painter.line_segment([b.left_top(), b.right_bottom()], line);
            painter.line_segment([b.right_top(), b.left_bottom()], line);
        }
    }
    if response.on_hover_text(&name).clicked() {
        let command = match caption {
            Caption::Minimize => ViewportCommand::Minimized(true),
            Caption::Maximize => ViewportCommand::Maximized(!max),
            Caption::Close => ViewportCommand::Close,
        };
        ui.ctx().send_viewport_cmd(command);
    }
}

/// Invisible resize zones along the window's edges and corners. Call last in
/// the frame, after everything else, so the zones take the border clicks.
/// A maximized or full-screen window has none, so its top-right pixel hits
/// Close; nor has a window the compositor decorates, which resizes it.
pub fn edges(ui: &mut Ui) {
    let fullscreen = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
    let server = Chrome::of(ui.ctx()).decorations == Decorations::Server;
    if maximized(ui) || fullscreen || server {
        return;
    }
    let m = Chrome::of(ui.ctx()).metrics;
    let (edge, corner) = (m.resize_edge, m.resize_corner);
    let r = ui.ctx().content_rect();
    use ResizeDirection as D;
    let zones = [
        (
            D::NorthWest,
            Rect::from_min_size(r.left_top(), vec2(corner, corner)),
            CursorIcon::ResizeNorthWest,
        ),
        (
            D::NorthEast,
            Rect::from_min_size(r.right_top() - vec2(corner, 0.0), vec2(corner, corner)),
            CursorIcon::ResizeNorthEast,
        ),
        (
            D::SouthWest,
            Rect::from_min_size(r.left_bottom() - vec2(0.0, corner), vec2(corner, corner)),
            CursorIcon::ResizeSouthWest,
        ),
        (
            D::SouthEast,
            Rect::from_min_size(
                r.right_bottom() - vec2(corner, corner),
                vec2(corner, corner),
            ),
            CursorIcon::ResizeSouthEast,
        ),
        (
            D::North,
            Rect::from_min_max(
                r.left_top() + vec2(corner, 0.0),
                r.right_top() + vec2(-corner, edge),
            ),
            CursorIcon::ResizeNorth,
        ),
        (
            D::South,
            Rect::from_min_max(
                r.left_bottom() + vec2(corner, -edge),
                r.right_bottom() - vec2(corner, 0.0),
            ),
            CursorIcon::ResizeSouth,
        ),
        (
            D::West,
            Rect::from_min_max(
                r.left_top() + vec2(0.0, corner),
                r.left_bottom() + vec2(edge, -corner),
            ),
            CursorIcon::ResizeWest,
        ),
        (
            D::East,
            Rect::from_min_max(
                r.right_top() + vec2(-edge, corner),
                r.right_bottom() - vec2(0.0, corner),
            ),
            CursorIcon::ResizeEast,
        ),
    ];
    for (direction, zone, cursor) in zones {
        let response = ui.interact(
            zone,
            ui.scope_id()
                .with(("toolkit-resize", format!("{direction:?}"))),
            Sense::drag(),
        );
        if response
            .on_hover_cursor(cursor)
            .drag_started_by(egui::PointerButton::Primary)
        {
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}

/// The installed value of the former `COMBO_WIDTH` constant.
pub fn combo_width_of(ctx: &egui::Context) -> egui::Rangef {
    egui::Rangef {
        min: crate::chrome::Chrome::of(ctx).style.title_combo_min_width as f32,
        max: crate::chrome::Chrome::of(ctx).style.title_combo_max_width as f32,
    }
}

#[cfg(test)]
mod decoration_tests {
    use super::decorations_changed;

    #[test]
    fn each_viewport_keeps_its_own_decoration_ask() {
        let ctx = egui::Context::default();
        let root = egui::ViewportId::ROOT;
        let child = egui::ViewportId::from_hash_of("compose-2");
        assert!(decorations_changed(&ctx, root, true));
        assert!(!decorations_changed(&ctx, child, false));
        // A shared flag would flip here and re-ask on every pass.
        assert!(!decorations_changed(&ctx, root, true));
        assert!(!decorations_changed(&ctx, child, false));
        assert!(decorations_changed(&ctx, child, true));
        assert!(!decorations_changed(&ctx, root, true));
    }
}
