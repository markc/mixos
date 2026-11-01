// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The editors that keep their own layout inside the COSMIC-shaped window
//! ([`crate::cosmic`]): Software over the releases service, and the app
//! pages (Term, Email, Ced) until the stage 5 work
//! list ports them onto page data. Drawing reads the engine and returns
//! what the person did as [`UiEvent`]s; the shell applies them after the
//! frame, so drawing never mutates state and a snapshot needs no Bus.
use crate::{label, label_with};
use design::{Mode, Scheme};
use egui::{Align, Color32, Layout, RichText, ScrollArea, Ui};
use preferences::appearance::Availability as Settings;
use preferences::terminal::{
    BORDER_WIDTH_MAX, BORDER_WIDTH_MIN, CursorShape, FONT_SIZE_MAX, FONT_SIZE_MIN, FocusRing,
    PADDINGS, SCROLL_SPEED_MAX, SCROLL_SPEED_MIN, SCROLLBACK_MAX, Setting, Terminal,
    family_setting, family_text,
};
use preferences::{AppRow, Availability, Dialog, Engine, RowState, SoftwareGroup};
use serde_json::Value;
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::{Mutex, OnceLock};
use toolkit::action::Origin;
use toolkit::button::PushButton;
use toolkit::dialog::{self, Choice};
use toolkit::field::ValueField;
use toolkit::slider::Slider;
use toolkit::toggle::Toggle;
use toolkit::{Registry, Strings, bars, panel, tooltip};
mod email;
mod text;

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Command(&'static str, Origin),
    /// Open a page, landing on a row.
    Page(&'static str, Option<&'static str>),
    /// The sub-page's back link.
    Back,
    /// A control's new value for its key (null resets it).
    Set(&'static str, Value),
    /// "Reset to default": the authored keys among these.
    Reset(&'static [&'static str]),
    /// Open a drawer (a page drawer or a row's), or close it.
    Drawer(Option<&'static str>),
    /// The search query as typed.
    Query(String),
    CloseSearch,
    /// The landing row was brought into view.
    FocusDone,
    /// A value the window refused before sending (a bad hex colour).
    Problem(String),
    /// A live item's button: (source, item, action).
    Item(&'static str, String, String),
    /// An outputs selector's tab (none: the default).
    Output(Option<String>),
    SoftwareGroup(SoftwareGroup),
    Search(String),
    SearchFocused,
    CloseMacros,
    DismissSettingsProblem,
    /// Select this app in the Applications table.
    Select(String),
    CloseDialog,
    /// The remove dialog's Remove button.
    ConfirmRemove,
    /// The title bar's light/dark toggle.
    SetMode(design::Mode),
    /// An edit in the Terminal group: one `terminal.*` field.
    Terminal(Setting),
    /// An edit in the Text section: one prose surface's style.
    Text(preferences::text::Setting),
    /// Wallpaper's chooser shows this category.
    Category(preferences::gallery::Category),
    /// A folder chosen with "Open new folder".
    OpenFolder(String),
    /// Images chosen with "Add image".
    AddImages(Vec<String>),
    /// Forget a custom image or colour: (its list's key, the value).
    RemoveCustom(&'static str, String),
    /// The email remove or reset dialog's confirm button.
    ConfirmEmail,
    /// Select this email account by id.
    EmailSelect(String),
    /// A field of the account form as typed.
    EmailField(preferences::email::Field, String),
    /// The identity list of the selected account: a field as typed, a row
    /// picked, added or removed, the list saved or reverted.
    EmailIdentityEdit(preferences::email::IdentityField, String),
    EmailIdentitySelect(usize),
    EmailIdentityAdd,
    EmailIdentityRemove,
    EmailIdentitySave,
    EmailIdentityRevert,
    /// The password field as typed (never shown, never stored).
    EmailPassword(preferences::email::Secret),
    /// A change that applies at once: enable, a mail option.
    EmailChange(preferences::email::Change),
}

/// Everything drawn over the page: dialogs, the macros drawer and the window's resize edges.
pub fn overlays(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    dialogs(ui, engine, commands, strings, events);
    if toolkit::macros::show_app(ui.ctx(), "prefs", engine.ui.macros_open) {
        events.push(UiEvent::CloseMacros);
    }
    toolkit::titlebar::edges(ui);
}

pub fn email_section(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    email::section(ui, engine, commands, strings, events);
}

pub fn ced_section(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    text::section(ui, engine, &[preferences::text::Surface::CedEditor], events);
}

/// A scheme's chip colours in `mode`: its accent, and the ink on it.
pub fn scheme_swatch(scheme: Scheme, mode: Mode) -> Option<(Color32, Option<Color32>)> {
    swatch(scheme, mode).map(|s| (s.fill, s.ink))
}

const LABEL_WIDTH: f32 = 144.0;

fn field_label(ui: &mut Ui, text: &str) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(LABEL_WIDTH, ui.spacing().interact_size.y),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_width(LABEL_WIDTH);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            ui.label(text)
        },
    )
    .inner
}

fn toggle(ui: &mut Ui, value: &mut bool, key: &str) -> bool {
    let response = ui.add(Toggle::new(value, ""));
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::CheckBox, ui.is_enabled(), *value, label(key))
    });
    response.changed()
}

fn row<R>(ui: &mut Ui, key: &str, text: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Grid::new(key)
        .num_columns(2)
        .spacing([24.0, 10.0])
        .show(ui, |ui| {
            let caption = field_label(ui, text);
            let result = ui
                .vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.label_inputs_by(caption.id, control)
                })
                .inner;
            ui.end_row();
            result
        })
        .inner
}

fn group_heading(ui: &mut Ui, key: &str, first: bool) {
    if !first {
        bars::hairline(ui);
    }
    ui.add_space(ui.spacing().item_spacing.y);
    let size = egui::TextStyle::Body.resolve(ui.style()).size;
    let font = toolkit::fonts::bound(ui.ctx(), toolkit::fonts::heading(size * 1.1));
    ui.label(RichText::new(label(key)).font(font).strong());
    ui.add_space(ui.spacing().item_spacing.y);
}

/// A button for registry command `id`, enabled as the registry says.
fn command_button(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    id: &'static str,
    primary: bool,
    events: &mut Vec<UiEvent>,
) {
    let Some(command) = commands.get(id) else {
        return;
    };
    let text = strings.get(command.label);
    let button = if primary {
        PushButton::primary(text)
    } else {
        PushButton::secondary(text)
    };
    let response = ui.add_enabled((command.enabled)(engine), button);
    if response
        .on_hover_text(tooltip::for_command(ui.ctx(), command, strings))
        .clicked()
    {
        events.push(UiEvent::Command(command.id, Origin::Ui));
    }
}

fn status_text(ui: &Ui, row: &AppRow) -> RichText {
    let visuals = ui.visuals();
    match row.state() {
        RowState::Current => {
            RichText::new(label("status-current")).color(visuals.weak_text_color())
        }
        RowState::Update => RichText::new(label("status-update")).color(visuals.warn_fg_color),
        RowState::NotInstalled => RichText::new(label("status-not-installed")),
        RowState::Unchecked => {
            RichText::new(label("status-unchecked")).color(visuals.weak_text_color())
        }
        RowState::Error => RichText::new(label("status-error")).color(visuals.error_fg_color),
    }
}

pub fn applications(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let groups: Vec<_> = SoftwareGroup::ALL
        .into_iter()
        .filter(|g| {
            *g != SoftwareGroup::Other
                || *g == engine.ui.software_group
                || engine.apps.iter().any(|r| g.matches(r))
        })
        .map(|g| {
            let title = engine
                .group_label(g)
                .map(str::to_owned)
                .unwrap_or_else(|| label(g.label()));
            (g, title)
        })
        .collect();
    ui.add_enabled_ui(engine.can_navigate(), |ui| {
        if let Some(group) = choices(ui, engine.ui.software_group, &groups) {
            events.push(UiEvent::SoftwareGroup(group));
        }
    });
    let height = ui.available_height();
    // The table takes the upper part; the selected app's notes the rest.
    ui.allocate_ui(egui::vec2(ui.available_width(), height * 0.55), |ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                command_button(ui, engine, commands, strings, "apps.check", false, events);
                command_button(
                    ui,
                    engine,
                    commands,
                    strings,
                    "apps.update_all",
                    false,
                    events,
                );
            });
            bars::hairline(ui);
            ui.add(
                egui::Label::new(
                    RichText::new(label_with(
                        "apps-scope",
                        &[(
                            "prefix",
                            &::paths::root(::paths::Root::Prefix).display().to_string(),
                        )],
                    ))
                    .weak(),
                )
                .wrap(),
            );
            if engine.releases == Availability::Missing {
                ui.add(egui::Label::new(RichText::new(label("missing")).weak()).wrap());
                return;
            }
            if engine.visible_apps().next().is_none() {
                if engine.working().is_none() {
                    ui.add(egui::Label::new(RichText::new(label("group-empty")).weak()).wrap());
                }
                return;
            }
            ScrollArea::vertical()
                .id_salt("apps")
                .auto_shrink(false)
                .show(ui, |ui| table(ui, engine, events));
        });
    });
    let notes_title = engine.ui.selected.clone().unwrap_or_else(|| label("notes"));
    panel::group(ui, "notes", &[notes_title.as_str()], |ui, _| {
        ui.horizontal(|ui| {
            command_button(ui, engine, commands, strings, "apps.install", true, events);
            command_button(
                ui,
                engine,
                commands,
                strings,
                "apps.rollback",
                false,
                events,
            );
            command_button(ui, engine, commands, strings, "apps.remove", false, events);
        });
        bars::hairline(ui);
        ScrollArea::vertical()
            .id_salt("notes")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                notes(ui, engine);
            });
    });
}

/// A scheme's swatch in `mode`: the accent it draws (its own style) and the
/// ink on its accent pair, compiled once per scheme and mode. A hue
/// scheme's accent is a quiet fill; its ink is the colour the eye reads.
#[derive(Clone, Copy)]
struct Swatch {
    fill: Color32,
    ink: Option<Color32>,
}

fn swatch(scheme: Scheme, mode: Mode) -> Option<Swatch> {
    type Cache = Mutex<HashMap<(Scheme, Mode), Option<Swatch>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let rgba = |[r, g, b, a]: [u8; 4]| Color32::from_rgba_unmultiplied(r, g, b, a);
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *cache.entry((scheme, mode)).or_insert_with(|| {
        let theme = toolkit::Theme::for_context(design::DesignContext {
            scheme,
            mode,
            ..design::DesignContext::default()
        });
        let ink = theme
            .dictionary()
            .colours
            .pairs
            .get("accent")
            .map(|pair| rgba(pair.rendered_foreground.to_srgba8()));
        theme.accent().map(|fill| Swatch {
            fill: rgba(fill.to_srgba8()),
            ink,
        })
    })
}

/// One row of mutually exclusive choices; the one picked, if any.
fn choices<T: Copy + PartialEq>(ui: &mut Ui, current: T, options: &[(T, String)]) -> Option<T> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        for (value, text) in options {
            if toolkit::selectable(ui, *value == current, text).clicked() && *value != current {
                picked = Some(*value);
            }
        }
    });
    picked
}

/// The Terminal group: one control per `terminal.*` field, each applied as
/// it is made. The values are settingsd's, as its last read gave them.
pub fn terminal(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            command_button(
                ui,
                engine,
                commands,
                strings,
                "terminal.reset",
                false,
                events,
            );
        });
        bars::hairline(ui);
        if engine.look.settings == Settings::Missing {
            ui.add(egui::Label::new(RichText::new(label("look-missing")).weak()).wrap());
            return;
        }
        let Some(term) = engine.look.terminal.clone() else {
            ui.label(RichText::new(label("look-reading")).weak());
            return;
        };
        ui.add_enabled_ui(engine.can_edit_terminal(), |ui| {
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                terminal_controls(
                    ui,
                    &term,
                    engine.look.shown().and_then(|look| look.mono_family),
                    events,
                );
            });
        });
    });
}

/// The controls for each field, in the order the settings are read.
fn terminal_controls(
    ui: &mut Ui,
    term: &Terminal,
    mono: Option<String>,
    events: &mut Vec<UiEvent>,
) {
    group_heading(ui, "term-text", true);
    row(ui, "terminal-family-row", &label("terminal-family"), |ui| {
        let family = family_text(term.font.family.as_deref());
        let hint = label_with(
            "terminal-family-system",
            &[("family", mono.as_deref().unwrap_or("JetBrains Mono"))],
        );
        if let Some(setting) = checked_text(ui, "terminal-family", &family, &hint, family_setting) {
            events.push(UiEvent::Terminal(setting));
        }
    });
    let size = Numeric {
        key: "terminal-size",
        title: label("terminal-size"),
        unit: "px",
        places: 1,
        slider: true,
    };
    if let Some(value) = number(ui, size, term.font.size, FONT_SIZE_MIN..=FONT_SIZE_MAX) {
        events.push(UiEvent::Terminal(Setting::Size(value)));
    }

    row(ui, "terminal-weight-row", &label("terminal-weight"), |ui| {
        let weights = [
            (300, label("terminal-weight-light")),
            (400, label("terminal-weight-regular")),
            (500, label("terminal-weight-medium")),
        ];
        if let Some(weight) = choices(ui, term.font.weight, &weights) {
            events.push(UiEvent::Terminal(Setting::Weight(weight)));
        }
    });

    row(
        ui,
        "terminal-padding-row",
        &label("terminal-padding"),
        |ui| {
            let paddings: Vec<(u32, String)> =
                PADDINGS.iter().map(|&p| (p, p.to_string())).collect();
            if let Some(padding) = choices(ui, term.padding, &paddings) {
                events.push(UiEvent::Terminal(Setting::Padding(padding)));
            }
        },
    );

    group_heading(ui, "term-behaviour", false);
    let scrollback = Numeric {
        key: "terminal-scrollback",
        title: label("terminal-scrollback"),
        unit: "",
        places: 0,
        slider: false,
    };
    let range = 0.0..=f64::from(SCROLLBACK_MAX);
    if let Some(lines) = number(ui, scrollback, f64::from(term.scrollback.lines), range) {
        events.push(UiEvent::Terminal(Setting::Scrollback(lines as u32)));
    }

    let speed = Numeric {
        key: "terminal-scroll-speed",
        title: label("terminal-scroll-speed"),
        unit: "",
        places: 0,
        slider: true,
    };
    let range = f64::from(SCROLL_SPEED_MIN)..=f64::from(SCROLL_SPEED_MAX);
    if let Some(value) = number(ui, speed, f64::from(term.scroll.speed), range) {
        events.push(UiEvent::Terminal(Setting::ScrollSpeed(value as u32)));
    }

    row(
        ui,
        "terminal-cursor-shape-row",
        &label("terminal-cursor-shape"),
        |ui| {
            let shapes = [
                (CursorShape::Block, label("terminal-cursor-block")),
                (CursorShape::Underline, label("terminal-cursor-underline")),
                (CursorShape::Bar, label("terminal-cursor-bar")),
            ];
            if let Some(shape) = choices(ui, term.cursor.shape, &shapes) {
                events.push(UiEvent::Terminal(Setting::CursorShape(shape)));
            }
        },
    );
    row(
        ui,
        "terminal-cursor-blink-row",
        &label("terminal-cursor-blink"),
        |ui| {
            let mut blink = term.cursor.blink;
            if toggle(ui, &mut blink, "terminal-cursor-blink") {
                events.push(UiEvent::Terminal(Setting::CursorBlink(blink)));
            }
        },
    );

    row(
        ui,
        "terminal-copy-on-select-row",
        &label("terminal-copy-on-select"),
        |ui| {
            let mut copy = term.mouse.copy_on_select;
            if toggle(ui, &mut copy, "terminal-copy-on-select") {
                events.push(UiEvent::Terminal(Setting::CopyOnSelect(copy)));
            }
        },
    );

    row(
        ui,
        "terminal-osc52-read-row",
        &label("terminal-osc52-read"),
        |ui| {
            let mut read = term.clipboard.osc52_read;
            if toggle(ui, &mut read, "terminal-osc52-read") {
                events.push(UiEvent::Terminal(Setting::Osc52Read(read)));
            }
        },
    );

    group_heading(ui, "term-advanced", false);
    row(ui, "terminal-border-row", &label("terminal-border"), |ui| {
        let widths: Vec<(u32, String)> = (BORDER_WIDTH_MIN..=BORDER_WIDTH_MAX)
            .map(|w| (w, w.to_string()))
            .collect();
        if let Some(width) = choices(ui, term.border.width, &widths) {
            events.push(UiEvent::Terminal(Setting::BorderWidth(width)));
        }
    });

    row(
        ui,
        "terminal-focus-ring-row",
        &label("terminal-focus-ring"),
        |ui| {
            let rings = [
                (FocusRing::Neutral, label("terminal-focus-neutral")),
                (FocusRing::Accent, label("terminal-focus-accent")),
            ];
            if let Some(ring) = choices(ui, term.focus.ring, &rings) {
                events.push(UiEvent::Terminal(Setting::FocusRing(ring)));
            }
        },
    );
    if term.focus.ring == FocusRing::Accent {
        let strength = Numeric {
            key: "terminal-focus-strength",
            title: label("terminal-focus-strength"),
            unit: "",
            places: 2,
            slider: true,
        };
        if let Some(value) = number(ui, strength, term.focus.strength, 0.0..=1.0) {
            events.push(UiEvent::Terminal(Setting::FocusStrength(value)));
        }
    }

    row(ui, "terminal-term-row", &label("terminal-term"), |ui| {
        let hint = "xterm-256color";
        let to_term = |text: &str| Setting::Term(text.to_owned());
        if let Some(setting) = checked_text(ui, "terminal-term", &term.env.term, hint, to_term) {
            events.push(UiEvent::Terminal(setting));
        }
        ui.add(egui::Label::new(RichText::new(label("terminal-term-note")).weak()).wrap());
    });
}

/// A numeric control's identity and how it draws and rounds its value.
struct Numeric {
    /// The memory key for a drag in progress.
    key: &'static str,
    title: String,
    unit: &'static str,
    /// Decimal places the value is rounded to when it is applied.
    places: i32,
    /// A slider beside the typed value, or the typed value alone.
    slider: bool,
}

/// A number, typed or dragged. A typed value applies when its field loses
/// focus, a drag when the pointer lets go, so no keystroke is sent. `Some`
/// is the value to apply, rounded, when it differs from `current`.
fn number(ui: &mut Ui, spec: Numeric, current: f64, range: RangeInclusive<f64>) -> Option<f64> {
    let id = egui::Id::unique(spec.key);
    let mut value = ui.data(|d| d.get_temp::<f64>(id)).unwrap_or(current);
    let mut commit = false;
    row(ui, spec.key, &spec.title, |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
                let field = ValueField::new(&mut value, range.clone()).unit(spec.unit);
                let response = ui.add(field).accessible_name(spec.title.clone());
                let cancelled = response.lost_focus() && escaped;
                if cancelled {
                    ui.data_mut(|d| d.remove::<f64>(id));
                    value = current;
                } else if response.has_focus() || response.dragged() {
                    ui.data_mut(|d| d.insert_temp(id, value));
                }
                if !cancelled && (response.lost_focus() || response.drag_stopped()) {
                    commit = true;
                }
                if spec.slider {
                    let slider = Slider::new(&mut value, range.clone()).label(spec.title.as_str());
                    let response = ui.add(slider);
                    if response.dragged() {
                        ui.data_mut(|d| d.insert_temp(id, value));
                    } else if response.drag_stopped() || response.changed() {
                        commit = true;
                    }
                }
            });
        });
    });
    if !commit {
        return None;
    }
    ui.data_mut(|d| d.remove::<f64>(id));
    let scale = 10f64.powi(spec.places);
    let rounded = (value * scale).round() / scale;
    (rounded != (current * scale).round() / scale).then_some(rounded)
}

/// A text field that applies when Enter is pressed or it loses focus, not
/// per keystroke. While it is not being edited it shows `current`. `Some`
/// is the trimmed text to apply when it differs from `current`.
fn text_field(ui: &mut Ui, key: &'static str, current: &str, hint: &str) -> Option<String> {
    let id = egui::Id::unique(key);
    let mut text = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| current.to_owned());
    let response = toolkit::field::text(ui, &mut text, hint, ui.available_width());
    if response.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(id));
        let text = text.trim();
        return (text != current.trim()).then(|| text.to_owned());
    }
    if response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    None
}

/// A [`text_field`] for a terminal setting. Text settingsd would refuse is
/// not sent: the reason shows under the field until a value is accepted.
fn checked_text(
    ui: &mut Ui,
    key: &'static str,
    current: &str,
    hint: &str,
    setting: impl Fn(&str) -> Setting,
) -> Option<Setting> {
    let refused_id = egui::Id::unique(key).with("refused");
    let committed = text_field(ui, key, current, hint).map(|text| setting(&text));
    if let Some(committed) = &committed {
        ui.data_mut(|d| match committed.refusal() {
            Some(refusal) => {
                d.insert_temp(refused_id, refusal);
            }
            None => {
                d.remove::<&'static str>(refused_id);
            }
        });
    }
    if let Some(refusal) = ui.data(|d| d.get_temp::<&'static str>(refused_id)) {
        let text = RichText::new(label(refusal)).color(ui.visuals().error_fg_color);
        ui.add(egui::Label::new(text).wrap());
    }
    committed.filter(|setting| setting.refusal().is_none())
}

fn table(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    egui::Grid::new("apps-table")
        .num_columns(5)
        .striped(true)
        .spacing([18.0, 6.0])
        .show(ui, |ui| {
            for key in [
                "col-app",
                "col-installed",
                "col-latest",
                "col-published",
                "col-status",
            ] {
                panel::section_label(ui, &label(key));
            }
            ui.end_row();
            for row in engine.visible_apps() {
                let selected = engine.ui.selected.as_deref() == Some(row.app.as_str());
                if toolkit::selectable(ui, selected, &row.app).clicked() {
                    events.push(UiEvent::Select(row.app.clone()));
                }
                let dash = || "—".to_owned();
                ui.monospace(row.installed.clone().unwrap_or_else(dash));
                ui.monospace(row.latest.clone().unwrap_or_else(dash));
                ui.label(row.published.clone().unwrap_or_else(dash));
                let status = status_text(ui, row);
                ui.label(status).on_hover_text(&row.status);
                ui.end_row();
            }
        });
}

fn notes(ui: &mut Ui, engine: &Engine) {
    let Some(app) = engine.ui.selected.as_deref() else {
        ui.label(RichText::new(label("select-app")).weak());
        return;
    };
    if let Some(error) = &engine.notes_error {
        ui.add(
            egui::Label::new(
                RichText::new(label_with("notes-failed", &[("message", error)]))
                    .color(ui.visuals().error_fg_color),
            )
            .wrap(),
        );
        return;
    }
    let Some(notes) = engine.notes.as_ref().filter(|n| n.app == app) else {
        ui.label(RichText::new(label("notes-loading")).weak());
        return;
    };
    ui.horizontal(|ui| {
        ui.strong(&notes.tag);
        if let Some(published) = &notes.published {
            ui.label(RichText::new(published.get(..10).unwrap_or(published)).weak());
        }
        if let Some(url) = &notes.url {
            ui.hyperlink_to(RichText::new(url).small(), url);
        }
    });
    if notes.notes.trim().is_empty() {
        ui.label(RichText::new(label("notes-empty")).weak());
    } else {
        release_notes(ui, &notes.notes);
    }
}

/// Release notes as GitHub writes them, read simply: `#` headings bold,
/// `*`/`-` items as bullets, everything else as wrapped text. Not a
/// Markdown renderer: links and emphasis stay as written.
fn release_notes(ui: &mut Ui, text: &str) {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            ui.add_space(ui.spacing().item_spacing.y);
        } else if let Some(heading) = trimmed.strip_prefix('#') {
            ui.add(
                egui::Label::new(RichText::new(heading.trim_start_matches('#').trim()).strong())
                    .wrap(),
            );
        } else if let Some(item) = trimmed
            .strip_prefix("* ")
            .or_else(|| trimmed.strip_prefix("- "))
        {
            ui.add(egui::Label::new(format!("•  {}", item.trim())).wrap());
        } else {
            ui.add(egui::Label::new(trimmed).wrap());
        }
    }
}

fn dialogs(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let Some(dialog) = &engine.ui.dialog else {
        return;
    };
    match dialog {
        Dialog::RemoveAccount(_) | Dialog::ResetEmail | Dialog::ForgetPassword(_) => {
            email::confirmation(ui, engine, events)
        }
        Dialog::About | Dialog::Shortcuts => {
            let about = *dialog == Dialog::About;
            let title = if about { "about" } else { "shortcuts" };
            let id = egui::Id::unique("dialog").with(title);
            // The shortcuts in force, from the registry: the profile can rebind them.
            let shown = dialog::Dialog::new(id, label(title))
                .buttons(vec![Choice::new(label("done"), dialog::Role::Default)])
                .show(ui.ctx(), |ui| {
                    if about {
                        ui.add(egui::Label::new(label("about-body")).wrap());
                    } else {
                        commands.shortcut_grid(ui, strings);
                    }
                });
            if shown.chosen.is_some() || shown.dismissed {
                // Each opening starts centred again.
                dialog::Dialog::reset(ui.ctx(), id);
                events.push(UiEvent::CloseDialog);
            }
        }
        Dialog::Remove(app) => {
            let id = egui::Id::unique("dialog").with("remove");
            let args = [("app", app.as_str())];
            // Cancel is the default: Enter never removes.
            let shown = dialog::Dialog::new(id, label_with("remove-title", &args))
                .buttons(vec![
                    Choice::new(label("remove-confirm"), dialog::Role::Other),
                    Choice::new(label("cancel"), dialog::Role::Default),
                ])
                .show(ui.ctx(), |ui| {
                    ui.add(egui::Label::new(label_with("remove-body", &args)).wrap())
                });
            if shown.chosen.is_some() || shown.dismissed {
                dialog::Dialog::reset(ui.ctx(), id);
                events.push(if shown.chosen == Some(0) {
                    UiEvent::ConfirmRemove
                } else {
                    UiEvent::CloseDialog
                });
            }
        }
    }
}
