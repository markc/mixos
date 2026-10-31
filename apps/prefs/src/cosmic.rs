// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The window in COSMIC Settings' anatomy (spec 4): the MixOS title bar as
//! the header (sidebar toggle first), a 280 px sidebar card of the eleven
//! top-level pages, and a content column of titled list cards whose rows
//! are the page data's controls ([`preferences::pages`]). Below 648 px the
//! sidebar folds away and the header's toggle shows it.
//!
//! Colours come from the derived theme of the installed look (the
//! `mixos` scheme draws COSMIC's numbers), spacing from the density key's
//! table and radii from the roundness key's tokens, so the window follows
//! the settings it edits the moment they apply.
use crate::view::UiEvent;
use crate::{label, label_with};
use design::cosmic_tables::{Density, Roundness};
use egui::{
    Align, Color32, CornerRadius, Layout, Rect, Response, RichText, Role, Sense, Stroke,
    StrokeKind, Ui, WidgetInfo, pos2, vec2,
};
use preferences::pages::{self, Body, ChoiceStyle, Control, Lit, Page, PickList, Row};
use preferences::{Engine, Section};
use serde_json::{Value, json};
use toolkit::action::Origin;
use toolkit::titlebar::{self, Action};
use toolkit::{Icon, Registry, Strings, icons};

mod controls;
mod drawer;

/// The sidebar card's width (spec 4).
pub const SIDEBAR: f32 = 280.0;
/// The narrowest content column before the sidebar folds.
const CONTENT_MIN: f32 = 360.0;
/// The content column's widest.
const CONTENT_MAX: f32 = 800.0;
/// A drawer's least width.
pub const DRAWER: f32 = 344.0;

fn rgba(c: design::SrgbColour) -> Color32 {
    let [r, g, b, a] = c.to_srgba8();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn over(top: Color32, alpha: f32, base: Color32) -> Color32 {
    let mix = |t: u8, b: u8| (f32::from(t) * alpha + f32::from(b) * (1.0 - alpha)).round() as u8;
    Color32::from_rgb(
        mix(top.r(), base.r()),
        mix(top.g(), base.g()),
        mix(top.b(), base.b()),
    )
}

/// What the window draws with: the derived theme's roles, the density's
/// spacing and the roundness tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub dark: bool,
    pub window: Color32,
    pub sidebar: Color32,
    pub sidebar_selected: Color32,
    pub sidebar_hover: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub divider: Color32,
    pub text: Color32,
    pub icon: Color32,
    pub value: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub button: Color32,
    pub button_hover: Color32,
    pub track_off: Color32,
    pub knob: Color32,
    pub field: Color32,
    pub field_border: Color32,
    pub swatch_border: Color32,
    pub warning: Color32,
    /// Spacing tokens xxs, xs, s, m, l, xl.
    pub xxs: f32,
    pub xs: f32,
    pub s: f32,
    pub m: f32,
    pub l: f32,
    pub xl: f32,
    /// Radius tokens xs, s, m, xl.
    pub r_xs: f32,
    pub r_s: f32,
    pub r_m: f32,
    pub r_xl: f32,
    pub density: Density,
}

impl Look {
    /// The look of `theme` at the density and roundness `engine` holds.
    pub fn of(theme: &toolkit::Theme, engine: &Engine) -> Self {
        let derived = design::seeds::derived_theme(theme.context());
        let p = &derived.palette;
        let density = match engine.store.text("appearance.density").as_deref() {
            Some("compact") => Density::Compact,
            Some("spacious") => Density::Spacious,
            _ => Density::Standard,
        };
        let space = density.spacing();
        let radii = match engine.store.text("appearance.roundness").as_deref() {
            Some("slightly_round") => Roundness::SlightlyRound.corner_radii(),
            // MixOS's Square is 0 on every token (key space F2).
            Some("square") => {
                let mut r = Roundness::Square.corner_radii();
                for token in [
                    &mut r.radius_xs,
                    &mut r.radius_s,
                    &mut r.radius_m,
                    &mut r.radius_l,
                    &mut r.radius_xl,
                ] {
                    *token = [0.0; 4];
                }
                r
            }
            _ => Roundness::Round.corner_radii(),
        };
        let sidebar = rgba(derived.background.component.base);
        let neutral5 = rgba(p.neutral[5]);
        let text = rgba(derived.primary.on);
        Self {
            dark: derived.is_dark,
            window: rgba(derived.background.base),
            sidebar,
            sidebar_selected: over(neutral5, 0.2, sidebar),
            sidebar_hover: over(neutral5, 0.1, sidebar),
            card: rgba(derived.primary.base),
            card_hover: rgba(derived.primary.component.hover),
            divider: rgba(derived.primary.divider),
            text,
            icon: text.gamma_multiply(0.95),
            value: rgba(derived.primary.component.on),
            accent: rgba(derived.accent.base),
            on_accent: rgba(derived.accent.on),
            button: rgba(derived.button.base),
            button_hover: rgba(derived.button.hover),
            track_off: rgba(if derived.is_dark {
                p.neutral[6]
            } else {
                p.neutral[5]
            }),
            knob: rgba(p.neutral[2]),
            field: rgba(derived.primary.small_widget),
            field_border: rgba(p.neutral[4]),
            swatch_border: rgba(p.neutral[8]),
            warning: rgba(derived.warning.base),
            xxs: f32::from(space.space_xxs),
            xs: f32::from(space.space_xs),
            s: f32::from(space.space_s),
            m: f32::from(space.space_m),
            l: f32::from(space.space_l),
            xl: f32::from(space.space_xl),
            r_xs: radii.radius_xs[0] as f32,
            r_s: radii.radius_s[0] as f32,
            r_m: radii.radius_m[0] as f32,
            r_xl: radii.radius_xl[0] as f32,
            density,
        }
    }

    /// A one-line row's height (40 compact, 48, 56).
    pub fn row_height(&self) -> f32 {
        match self.density {
            Density::Compact => 40.0,
            Density::Standard => 48.0,
            Density::Spacious => 56.0,
        }
    }

    /// A row's side padding (16, 24, 32).
    pub fn row_pad(&self) -> f32 {
        self.m
    }

    pub fn radius(r: f32) -> CornerRadius {
        CornerRadius::same(r.clamp(0.0, 255.0) as u8)
    }
}

/// Whether the window is narrower than the sidebar, its gap and the least
/// content column.
pub fn condensed(width: f32) -> bool {
    width < SIDEBAR + 8.0 + CONTENT_MIN
}

/// Draw the whole window.
pub fn view(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    look: &Look,
) -> Vec<UiEvent> {
    let mut events = Vec::new();
    let fired = titlebar::show_bar(
        ui,
        &label("title"),
        Some(Icon::Settings),
        commands,
        engine,
        strings,
        &[Action::new("sidebar.toggle", Icon::PanelLeft)],
    );
    events.extend(
        fired
            .into_iter()
            .map(|(id, origin)| UiEvent::Command(id, origin)),
    );
    typed_search(ui, engine, &mut events);
    let condensed = condensed(ui.available_width());
    let frame = egui::Frame::NONE.fill(look.window);
    if !condensed || engine.nav.sidebar {
        egui::Panel::left("pages")
            .exact_size(SIDEBAR + 16.0)
            .resizable(false)
            .frame(frame)
            .show(ui, |ui| sidebar(ui, engine, look, &mut events));
    }
    if let Some(open) = engine.nav.drawer
        && let Some(page) = pages::get(engine.nav.page)
    {
        egui::Panel::right("drawer")
            .exact_size(DRAWER + 2.0 * look.l)
            .resizable(false)
            .frame(frame)
            .show(ui, |ui| {
                drawer::show(ui, engine, page, open, look, &mut events)
            });
    }
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        content(ui, engine, commands, strings, look, condensed, &mut events)
    });
    crate::view::overlays(ui, engine, commands, strings, &mut events);
    events
}

/// Typing a printable key with nothing focused opens search with it
/// (spec 4, "Search").
fn typed_search(ui: &Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    if engine.nav.searching || ui.ctx().memory(|m| m.focused().is_some()) || !engine.can_navigate()
    {
        return;
    }
    let typed: String = ui.input(|i| {
        if i.modifiers.ctrl || i.modifiers.alt || i.modifiers.command || i.modifiers.mac_cmd {
            return String::new();
        }
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    });
    let typed = typed.trim_start().to_owned();
    if !typed.is_empty() && !typed.starts_with('/') {
        events.push(UiEvent::Query(typed));
    }
}

fn sidebar(ui: &mut Ui, engine: &Engine, look: &Look, events: &mut Vec<UiEvent>) {
    let outer = ui.max_rect();
    let card = Rect::from_min_max(
        pos2(outer.min.x + 8.0, outer.min.y),
        pos2(outer.min.x + 8.0 + SIDEBAR, outer.max.y - 8.0),
    );
    ui.painter()
        .rect_filled(card, Look::radius(look.r_s), look.sidebar);
    let pad = look.xxs.max(4.0);
    let inner = card.shrink(pad);
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
    );
    ui.spacing_mut().item_spacing.y = pad;
    if engine.nav.searching {
        search_field(&mut ui, engine, look, events);
    }
    let selected = pages::root(engine.nav.page);
    egui::ScrollArea::vertical()
        .id_salt("pages-scroll")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(&mut ui, |ui| {
            for page in pages::top_level() {
                let title = label(&page.title());
                let (rect, response) =
                    ui.allocate_exact_size(vec2(inner.width(), 32.0), Sense::click());
                let fill = if page.id == selected {
                    look.sidebar_selected
                } else if response.hovered() {
                    look.sidebar_hover
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, Look::radius(look.r_s), fill);
                let x = rect.min.x + look.s;
                toolkit::button::paint_icon(
                    ui,
                    icon(page.icon),
                    pos2(x + 8.0, rect.center().y),
                    16.0,
                    look.icon,
                );
                ui.painter().text(
                    pos2(x + 16.0 + 4.0 + 8.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &title,
                    egui::FontId::proportional(14.0),
                    look.text,
                );
                if response.has_focus() {
                    ui.painter().rect_stroke(
                        rect.expand(1.0),
                        Look::radius(look.r_s),
                        Stroke::new(1.0, look.accent),
                        StrokeKind::Outside,
                    );
                }
                response.widget_info(|| {
                    WidgetInfo::selected(Role::Tab, ui.is_enabled(), page.id == selected, &title)
                });
                if response.clicked() {
                    events.push(UiEvent::Page(page.id, None));
                }
            }
        });
}

fn search_field(ui: &mut Ui, engine: &Engine, look: &Look, events: &mut Vec<UiEvent>) {
    let mut query = engine.nav.query.clone();
    let frame = egui::Frame::NONE
        .fill(look.window)
        .corner_radius(Look::radius(look.r_xl))
        .inner_margin(egui::Margin::symmetric(8, 4));
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add(icons::image_of(ui.ctx(), Icon::Search, 16.0, look.icon));
            let edit = egui::TextEdit::singleline(&mut query)
                .id_salt("pages-search")
                .frame(egui::Frame::NONE)
                .hint_text(label("search-hint"))
                .desired_width(ui.available_width() - 24.0);
            let response = ui.add(edit);
            if engine.ui.search_focus {
                response.request_focus();
                events.push(UiEvent::SearchFocused);
            }
            if response.changed() {
                events.push(UiEvent::Query(query.clone()));
            }
            if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                events.push(UiEvent::CloseSearch);
            }
            let clear = ui.add(
                toolkit::button::IconButton::new(Icon::X)
                    .size(16.0)
                    .tooltip(label("search-clear")),
            );
            if clear.clicked() {
                events.push(UiEvent::CloseSearch);
            }
        });
    });
}

/// The Lucide icon a page names.
pub fn icon(name: &str) -> Icon {
    Icon::from_name(name).unwrap_or(Icon::Settings)
}

fn content(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    look: &Look,
    condensed: bool,
    events: &mut Vec<UiEvent>,
) {
    let side = if condensed { look.s } else { look.l };
    let full = ui.max_rect();
    let width = (full.width() - 2.0 * side).clamp(0.0, CONTENT_MAX);
    let left = full.center().x - width / 2.0;
    let column = Rect::from_min_max(pos2(left, full.min.y), pos2(left + width, full.max.y));
    let mut ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(column)
            .layout(Layout::top_down(Align::Min)),
    );
    let ui = &mut ui;
    if engine.nav.searching && !engine.nav.query.trim().is_empty() {
        results(ui, engine, look, events);
        return;
    }
    let Some(page) = pages::get(engine.nav.page) else {
        return;
    };
    title_block(ui, engine, page, look, events);
    if let Body::Custom { .. } = page.body {
        custom(ui, engine, commands, strings, page, events);
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt(("page", page.id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(width);
            notices(ui, engine, page, look);
            match &page.body {
                Body::List => sub_pages(ui, page, look, events),
                Body::Sections { sections, .. } => {
                    for (index, section) in sections.iter().enumerate() {
                        if !section.show.holds(&engine.store) {
                            continue;
                        }
                        let rows: Vec<&Row> = section
                            .rows
                            .iter()
                            .filter(|r| r.show.holds(&engine.store))
                            .collect();
                        if rows.is_empty() {
                            continue;
                        }
                        if index > 0 {
                            ui.add_space(look.m);
                        }
                        section_card(ui, engine, page, section, &rows, look, events);
                    }
                }
                Body::Pending => {
                    card(ui, look, |ui| {
                        plain_row(
                            ui,
                            look,
                            &label("page-pending"),
                            Some(&label("page-pending-desc")),
                        );
                    });
                }
                Body::Custom { .. } => {}
            }
            ui.add_space(look.m);
        });
}

/// The back link (sub-pages), the title, and the page's header buttons.
fn title_block(ui: &mut Ui, engine: &Engine, page: &Page, look: &Look, events: &mut Vec<UiEvent>) {
    if let Some(parent) = page.parent.and_then(pages::get) {
        let text = label(&parent.title());
        let response = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add(icons::image_of(
                    ui.ctx(),
                    Icon::ChevronLeft,
                    16.0,
                    look.accent,
                ));
                ui.add(
                    egui::Label::new(RichText::new(&text).color(look.accent).size(14.0))
                        .sense(Sense::click()),
                )
            })
            .inner;
        response.widget_info(|| WidgetInfo::labeled(Role::Link, ui.is_enabled(), &text));
        if response.clicked() {
            events.push(UiEvent::Back);
        }
        ui.add_space(6.0);
    } else {
        ui.add_space(0.0);
    }
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label(&page.title()))
                .size(24.0)
                .strong()
                .color(look.text),
        );
        ui.with_layout(Layout::right_to_left(Align::Max), |ui| {
            for extra in page.extras.iter().rev() {
                let enabled = extra.command.is_some() && extra.backend.is_none();
                let text = label(&format!("extra-{}-{}", page.id, extra.id));
                let response = ui
                    .add_enabled(enabled, controls::standard_button(&text, look))
                    .on_disabled_hover_text(
                        extra
                            .backend
                            .map(|b| label_with("backend-missing", &[("backend", &label(b))]))
                            .unwrap_or_else(|| label("extra-pending")),
                    );
                if response.clicked()
                    && let Some(command) = extra.command
                {
                    events.push(UiEvent::Command(command, Origin::Ui));
                }
            }
        });
    });
    ui.add_space(look.m);
    let _ = engine;
}

/// The page's backend line and the store's write block, before any row.
fn notices(ui: &mut Ui, engine: &Engine, page: &Page, look: &Look) {
    if let Some(backend) = page.backend {
        notice(
            ui,
            look,
            &label_with("backend-missing", &[("backend", &label(backend))]),
        );
    }
    let writes = page.rows().any(|(_, r)| r.control.key().is_some());
    if writes
        && let Some(blocked) = engine.blocked()
        && !matches!(blocked, preferences::store::Blocked::Unread)
    {
        notice(ui, look, &label(blocked.label()));
    }
    if writes && let Some(error) = &engine.store.error {
        notice(ui, look, error);
    }
}

pub fn notice(ui: &mut Ui, look: &Look, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.add(icons::image_of(
            ui.ctx(),
            Icon::CircleAlert,
            16.0,
            look.warning,
        ));
        ui.label(RichText::new(text).size(12.0).color(look.text));
    });
    ui.add_space(look.xs);
}

/// A rounded list card the full column wide.
pub fn card<R>(ui: &mut Ui, look: &Look, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(look.card)
        .corner_radius(Look::radius(look.r_s))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
}

pub fn divider(ui: &mut Ui, look: &Look) {
    let rect = ui.available_rect_before_wrap();
    let y = ui.cursor().min.y;
    ui.painter()
        .hline(rect.min.x..=rect.max.x, y, Stroke::new(1.0, look.divider));
    ui.add_space(1.0);
}

/// A plain text row: a title and an optional caption.
pub fn plain_row(ui: &mut Ui, look: &Look, title: &str, description: Option<&str>) {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(look.row_pad() as i8, 4))
        .show(ui, |ui| {
            ui.set_min_height(32.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).size(14.0).color(look.text));
                if let Some(d) = description {
                    ui.label(RichText::new(d).size(12.0).color(look.text));
                }
            });
        });
}

fn sub_pages(ui: &mut Ui, page: &Page, look: &Look, events: &mut Vec<UiEvent>) {
    for (index, child) in pages::children(page.id).enumerate() {
        if index > 0 {
            ui.add_space(look.s);
        }
        let title = label(&child.title());
        let description = label(&child.description());
        let height = 40.0 + 2.0 * look.s;
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
        let fill = if response.hovered() {
            look.card_hover
        } else {
            look.card
        };
        ui.painter().rect_filled(rect, Look::radius(look.r_s), fill);
        let x = rect.min.x + look.m;
        toolkit::button::paint_icon(
            ui,
            icon(child.icon),
            pos2(x + 8.0 + 10.0, rect.center().y),
            20.0,
            look.icon,
        );
        let text_x = x + 36.0 + 4.0;
        ui.painter().text(
            pos2(text_x, rect.center().y - 1.0),
            egui::Align2::LEFT_BOTTOM,
            &title,
            egui::FontId::proportional(14.0),
            look.text,
        );
        if description != child.description() {
            ui.painter().text(
                pos2(text_x, rect.center().y + 1.0),
                egui::Align2::LEFT_TOP,
                &description,
                egui::FontId::proportional(12.0),
                look.text,
            );
        }
        toolkit::button::paint_icon(
            ui,
            Icon::ChevronRight,
            pos2(rect.max.x - look.m - 18.0, rect.center().y),
            20.0,
            look.icon,
        );
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect,
                Look::radius(look.r_s),
                Stroke::new(1.0, look.accent),
                StrokeKind::Outside,
            );
        }
        response.widget_info(|| WidgetInfo::labeled(Role::Link, ui.is_enabled(), &title));
        if response.clicked() {
            events.push(UiEvent::Page(child.id, None));
        }
    }
}

/// One section: its heading, then its rows in one card.
pub fn section_card(
    ui: &mut Ui,
    engine: &Engine,
    page: &Page,
    section: &pages::Section,
    rows: &[&Row],
    look: &Look,
    events: &mut Vec<UiEvent>,
) {
    if section.titled {
        ui.label(
            RichText::new(label(&pages::section_title(page.id, section.id)))
                .size(14.0)
                .strong()
                .color(look.text),
        );
        ui.add_space(look.xs);
    }
    // A lone button row (Reset) sits on the page, not in a card.
    if let [row] = rows
        && let Control::Reset { keys } = row.control
    {
        if engine.authored(keys) {
            let blocked = engine.row_blocked(page, row);
            let text = label(&pages::row_title(page.id, row.id));
            if ui
                .add_enabled(blocked.is_none(), controls::standard_button(&text, look))
                .clicked()
            {
                events.push(UiEvent::Reset(keys));
            }
        }
        return;
    }
    card(ui, look, |ui| {
        let mut first = true;
        for row in rows {
            // A radio choice is one row per option, in the same card.
            if let Control::Choice {
                key,
                style: ChoiceStyle::Radio,
                options,
            } = &row.control
            {
                let blocked = engine.row_blocked(page, row);
                for option in *options {
                    if !first {
                        divider(ui, look);
                    }
                    first = false;
                    controls::radio_row(
                        ui,
                        engine,
                        page,
                        row,
                        key,
                        option,
                        blocked.as_deref(),
                        look,
                        events,
                    );
                }
                continue;
            }
            if !first {
                divider(ui, look);
            }
            first = false;
            controls::row(ui, engine, page, row, look, events);
        }
    });
}

/// Search results: per page a header (parent / page link), then each hit
/// row drawn live, so its control works in place.
fn results(ui: &mut Ui, engine: &Engine, look: &Look, events: &mut Vec<UiEvent>) {
    let hits = engine.find(&engine.nav.query);
    egui::ScrollArea::vertical()
        .id_salt("results")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if hits.is_empty() {
                ui.label(
                    RichText::new(label("search-none"))
                        .size(14.0)
                        .color(look.text),
                );
            }
            let mut index = 0;
            while index < hits.len() {
                let page_id = hits[index].page;
                let end = hits[index..]
                    .iter()
                    .position(|h| h.page != page_id)
                    .map_or(hits.len(), |n| index + n);
                let Some(page) = pages::get(page_id) else {
                    index = end;
                    continue;
                };
                ui.horizontal(|ui| {
                    if let Some(parent) = page.parent.and_then(pages::get) {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(label(&parent.title()))
                                .size(14.0)
                                .color(look.text),
                        );
                        ui.label(RichText::new("/").size(14.0).color(look.text));
                    }
                    let title = label(&page.title());
                    let link = ui.add(
                        egui::Label::new(RichText::new(&title).size(14.0).color(look.accent))
                            .sense(Sense::click()),
                    );
                    link.widget_info(|| WidgetInfo::labeled(Role::Link, true, &title));
                    if link.clicked() {
                        events.push(UiEvent::Page(page.id, hits[index].row));
                    }
                });
                ui.add_space(look.xs);
                let rows: Vec<&Row> = hits[index..end]
                    .iter()
                    .filter_map(|h| h.row.and_then(|r| page.row(r)))
                    .filter(|r| r.show.holds(&engine.store))
                    .collect();
                if !rows.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add_space(look.l);
                        ui.vertical(|ui| {
                            let section = pages::Section {
                                id: "results",
                                titled: false,
                                rows: Vec::new(),
                                show: pages::Cond::Always,
                            };
                            section_card(ui, engine, page, &section, &rows, look, events);
                        });
                    });
                }
                ui.add_space(look.m);
                index = end;
            }
        });
}

/// The editors that keep their own layout (Software, the app pages).
fn custom(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    page: &Page,
    events: &mut Vec<UiEvent>,
) {
    if page.id == "software" {
        crate::view::applications(ui, engine, commands, strings, events);
        return;
    }
    let width = ui.available_width();
    egui::ScrollArea::vertical()
        .id_salt(("custom", page.id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_max_width(width);
            match engine.section() {
                Section::Terminal => crate::view::terminal(ui, engine, commands, strings, events),
                Section::Email => crate::view::email_section(ui, engine, commands, strings, events),
                Section::Ced => crate::view::ced_section(ui, engine, events),
                Section::Wallpapers => {
                    crate::view::wallpaper_section(ui, engine, commands, strings, events)
                }
                _ => {}
            }
        });
}

/// Apply a row's write, refusing nothing here: the engine checks it.
pub fn set(events: &mut Vec<UiEvent>, key: &'static str, value: Value) {
    events.push(UiEvent::Set(key, value));
}

/// The value of `lit` for a write.
pub fn lit(lit: Lit) -> Value {
    lit.value()
}

/// The list a pick row draws from.
pub fn pick_list(ui: &Ui, list: PickList) -> Vec<String> {
    match list {
        PickList::Fonts => crate::font_families::installed(ui.ctx())
            .map(|f| f.all.clone())
            .unwrap_or_default(),
        PickList::MonoFonts => crate::font_families::installed(ui.ctx())
            .map(|f| f.mono.clone())
            .unwrap_or_default(),
        PickList::IconThemes => icon_themes().to_vec(),
    }
}

/// The icon themes installed under the XDG data directories, by directory
/// name (as COSMIC and the exporters name them), read once.
fn icon_themes() -> &'static [String] {
    static THEMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    THEMES.get_or_init(|| {
        let mut roots: Vec<std::path::PathBuf> = std::env::var("XDG_DATA_DIRS")
            .unwrap_or_else(|_| "/usr/local/share:/usr/share".into())
            .split(':')
            .filter(|d| !d.is_empty())
            .map(|d| std::path::Path::new(d).join("icons"))
            .collect();
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(std::path::Path::new(&home).join(".local/share/icons"));
        }
        let mut names = std::collections::BTreeSet::new();
        for root in roots {
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let index = entry.path().join("index.theme");
                let Ok(text) = std::fs::read_to_string(&index) else {
                    continue;
                };
                if text.lines().any(|l| l.trim() == "Hidden=true") {
                    continue;
                }
                if let Some(name) = entry.file_name().to_str() {
                    names.insert(name.to_owned());
                }
            }
        }
        names.into_iter().collect()
    })
}

/// The row's description, when the catalogue has one.
pub fn description(page: &str, row: &str) -> Option<String> {
    let key = pages::row_description(page, row);
    let text = label(&key);
    (text != key).then_some(text)
}

/// The row's title.
pub fn title(page: &str, row: &str) -> String {
    label(&pages::row_title(page, row))
}

/// A response's label for agents, then the response.
pub fn named(response: Response, role: Role, enabled: bool, name: &str) -> Response {
    response.widget_info(|| WidgetInfo::labeled(role, enabled, name));
    response
}

/// The JSON of a colour for a write.
pub fn hex(colour: Color32, alpha: bool) -> Value {
    let [r, g, b, a] = colour.to_srgba_unmultiplied();
    if alpha && a < 255 {
        json!(format!("#{r:02x}{g:02x}{b:02x}{a:02x}"))
    } else {
        json!(format!("#{r:02x}{g:02x}{b:02x}"))
    }
}

/// A `#rrggbb[aa]` value as a colour.
pub fn parse_hex(text: &str) -> Option<Color32> {
    let hex = text.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(
            byte(0)?,
            byte(2)?,
            byte(4)?,
            byte(6)?,
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sidebar_folds_below_cosmic_s_breakpoint() {
        assert!(condensed(647.0));
        assert!(!condensed(648.0));
    }

    #[test]
    fn colours_round_trip_through_their_hex() {
        let c = Color32::from_rgb(0x12, 0xab, 0xef);
        assert_eq!(hex(c, false), json!("#12abef"));
        assert_eq!(parse_hex("#12abef"), Some(c));
        assert_eq!(parse_hex("#12abef80").map(|c| c.a()), Some(0x80));
        assert_eq!(parse_hex("12abef"), None);
    }

    #[test]
    fn the_compact_look_draws_cosmic_s_measurements() {
        let mut e = Engine::new(crate::label_with);
        e.store
            .values
            .insert("appearance.density".into(), json!("compact"));
        let look = Look::of(&toolkit::Theme::embedded(), &e);
        assert_eq!((look.xs, look.s, look.m, look.l), (8.0, 8.0, 16.0, 24.0));
        assert_eq!(look.row_height(), 40.0);
        assert_eq!((look.r_s, look.r_xl), (8.0, 160.0));
        e.store
            .values
            .insert("appearance.roundness".into(), json!("square"));
        assert_eq!(Look::of(&toolkit::Theme::embedded(), &e).r_xl, 0.0);
    }
}
