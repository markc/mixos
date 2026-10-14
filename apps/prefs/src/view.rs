// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The egui rendering of the engine's state. [`view`] reads the engine and
//! returns what the person did as [`UiEvent`]s; the shell applies them after
//! the frame, so drawing never mutates state and a snapshot needs no Bus.
//!
//! The window: the toolkit title bar, a sidebar of editors (the AmigaOS
//! Prefs drawer, one window), the selected editor, and a status bar. The
//! Applications editor is a panel group with the followed apps as a table,
//! a second group with the selected app's actions and release notes, and a
//! confirmation dialog before anything is removed. The Appearance editor
//! sets the session's look through settingsd, each change at once.
use crate::{label, label_with};
use design::{CaptionSide, Contrast, Decorations, Mode, Scheme, Style};
use egui::{Align, Color32, Layout, RichText, ScrollArea, Ui};
use preferences::appearance::{Availability as Settings, Axis};
use preferences::terminal::{
    BORDER_WIDTH_MAX, BORDER_WIDTH_MIN, CursorShape, FONT_SIZE_MAX, FONT_SIZE_MIN, FocusRing,
    PADDINGS, SCROLL_SPEED_MAX, SCROLL_SPEED_MIN, SCROLLBACK_MAX, Setting, Terminal,
    family_setting, family_text,
};
use preferences::{AppRow, Availability, Dialog, Engine, Look, Panel, RowState, Section};
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::{Mutex, OnceLock};
use toolkit::button::PushButton;
use toolkit::command::Origin;
use toolkit::dialog::{self, Choice};
use toolkit::field::ValueField;
use toolkit::slider::Slider;
use toolkit::titlebar::{self, Control};
use toolkit::toggle::Toggle;
use toolkit::{Icon, Registry, Strings, bars, icons, panel, tooltip};
mod wallpaper;

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Command(&'static str, Origin),
    /// Select this app in the Applications table.
    Select(String),
    CloseDialog,
    /// The remove dialog's Remove button.
    ConfirmRemove,
    /// The title bar's light/dark toggle.
    SetMode(design::Mode),
    /// An edit in the Appearance panel: the whole look it leaves.
    Look(Look),
    /// An edit in the Terminal group: one `terminal.*` field.
    Terminal(Setting),
    WallpaperTarget(Option<String>),
    WallpaperKind(preferences::wallpaper::Kind),
    WallpaperColour(String),
    WallpaperImage(String),
    PickerDirectory(String),
    PickerSelect(String),
    PickerEnter(String),
}

const ICON: f32 = 14.0;
/// A scheme's swatch chip.
const SWATCH: f32 = 14.0;

/// Draw the whole window into `ui`.
pub fn view(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    stroke: f32,
) -> Vec<UiEvent> {
    let mut events = Vec::new();
    let dark = ui.visuals().dark_mode;
    let mut controls = [
        Control::Icon {
            command: "view.mode",
            icon: titlebar::theme_icon(dark),
            selected: false,
        },
        Control::Link {
            command: "help.shortcuts",
            icon: Icon::Keyboard,
        },
    ];
    let fired = titlebar::show_with(
        ui,
        &label("title"),
        Some(Icon::Settings),
        stroke,
        commands,
        engine,
        strings,
        &mut controls,
    );
    // The toggle goes to the mode opposite the one this frame drew, so a
    // click reported twice cannot flip it back.
    let mut mode_set = false;
    for (id, origin) in fired {
        if id == "view.mode" {
            if !mode_set {
                commands.note(id, origin);
                let shown = engine.effective_theme().1;
                let target = if shown == design::Mode::Dark {
                    design::Mode::Light
                } else {
                    design::Mode::Dark
                };
                events.push(UiEvent::SetMode(target));
                mode_set = true;
            }
        } else {
            events.push(UiEvent::Command(id, origin));
        }
    }
    bars::status(ui, |ui| {
        let (icon, colour) = if !engine.connected {
            (Icon::Unplug, ui.visuals().error_fg_color)
        } else if engine.working().is_some() || engine.look.working() {
            (Icon::LoaderCircle, ui.visuals().weak_text_color())
        } else if engine.failed {
            (Icon::CircleAlert, ui.visuals().error_fg_color)
        } else {
            (Icon::Plug, ui.visuals().weak_text_color())
        };
        ui.add(icons::image(icon, stroke, ICON, colour));
        if engine.failed {
            ui.small(RichText::new(&engine.status).color(ui.visuals().error_fg_color));
        } else {
            ui.small(&engine.status);
        }
    });
    egui::Panel::left("editors")
        .resizable(true)
        .default_size(220.0)
        .min_size(184.0)
        .max_size(320.0)
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| editors(ui, engine, commands, stroke, &mut events));
    egui::CentralPanel::default()
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| match engine.section() {
            Section::Desktop | Section::Server | Section::Creative | Section::Other => {
                applications(ui, engine, commands, strings, &mut events)
            }
            section => {
                ScrollArea::vertical()
                    .id_salt(section.name())
                    .auto_shrink(false)
                    .show(ui, |ui| match section {
                        Section::Themes => appearance(ui, engine, commands, strings, &mut events),
                        Section::Wallpapers => {
                            wallpaper::controls(ui, engine, commands, strings, &mut events)
                        }
                        Section::Fonts => fonts(ui),
                        _ => unreachable!(),
                    });
            }
        });
    dialogs(ui, engine, commands, strings, &mut events);
    wallpaper::picker(ui, engine, &mut events);
    toolkit::titlebar::edges(ui);
    events
}

/// A tree whose selection and expansion live in the serialisable engine.
fn editors(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    let title = label("panels");
    panel::group(ui, "editors", &[title.as_str()], |ui, _| {
        ScrollArea::vertical()
            .id_salt("navigation-tree")
            .show(ui, |ui| {
                for (parent, heading, expanded, toggle) in [
                    (
                        Panel::Appearance,
                        "panel-appearance",
                        engine.ui.appearance_expanded,
                        "view.tree.appearance",
                    ),
                    (
                        Panel::Applications,
                        "panel-applications",
                        engine.ui.applications_expanded,
                        "view.tree.applications",
                    ),
                ] {
                    let enabled = commands.get(toggle).is_some_and(|c| (c.enabled)(engine));
                    let shown = ui.add_enabled_ui(enabled, |ui| {
                        egui::CollapsingHeader::new(label(heading))
                            .id_salt(heading)
                            .open(Some(expanded))
                            .show(ui, |ui| {
                                for section in
                                    Section::ALL.into_iter().filter(|s| s.panel() == parent)
                                {
                                    if section == Section::Other
                                        && !engine.apps.iter().any(|r| Section::Other.matches(r))
                                    {
                                        continue;
                                    }
                                    let command =
                                        commands.get(section.command()).expect("section command");
                                    ui.horizontal(|ui| {
                                        ui.add(icons::image(
                                            command.icon.expect("section icon"),
                                            stroke,
                                            ICON,
                                            ui.visuals().text_color(),
                                        ));
                                        if toolkit::selectable(
                                            ui,
                                            engine.section() == section,
                                            section_title(engine, section),
                                        )
                                        .clicked()
                                        {
                                            events.push(UiEvent::Command(
                                                section.command(),
                                                Origin::Ui,
                                            ));
                                        }
                                    });
                                }
                            })
                    });
                    if shown.inner.header_response.clicked() {
                        events.push(UiEvent::Command(toggle, Origin::Ui));
                    }
                }
            });
    });
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

fn section_title(engine: &Engine, section: Section) -> String {
    engine
        .category_label(section)
        .map(str::to_owned)
        .unwrap_or_else(|| label(section.label()))
}

/// Preview the actual face stacks installed on this rendering context.
fn fonts(ui: &mut Ui) {
    let title = label("section-fonts");
    panel::group(ui, "fonts", &[title.as_str()], |ui, _| {
        let faces = ui.ctx().fonts(|fonts| {
            let defs = fonts.definitions();
            [
                egui::FontFamily::Proportional,
                egui::FontFamily::Name(toolkit::fonts::HEADING.into()),
                egui::FontFamily::Monospace,
            ]
            .map(|family| {
                defs.families
                    .get(&family)
                    .and_then(|stack| stack.first())
                    .cloned()
                    .unwrap_or_default()
            })
        });
        egui::Grid::new("font-faces")
            .num_columns(2)
            .spacing([24.0, 14.0])
            .show(ui, |ui| {
                for (key, face) in ["fonts-interface", "fonts-headings", "fonts-monospace"]
                    .into_iter()
                    .zip(faces)
                {
                    panel::section_label(ui, &label(key));
                    ui.label(face);
                    ui.end_row();
                }
            });
        bars::hairline(ui);
        ui.heading(label("fonts-preview"));
        ui.label(label("fonts-sample"));
        ui.monospace(label("fonts-mono-sample"));
        bars::hairline(ui);
        ui.add(egui::Label::new(RichText::new(label("fonts-note")).weak()).wrap());
    });
}

fn applications(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let height = ui.available_height();
    let title = section_title(engine, engine.section());
    // The table takes the upper part; the selected app's notes the rest.
    ui.allocate_ui(egui::vec2(ui.available_width(), height * 0.55), |ui| {
        panel::group(ui, "applications", &[title.as_str()], |ui, _| {
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
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    command_button(ui, engine, commands, strings, "view.refresh", false, events);
                });
            });
            bars::hairline(ui);
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

/// Paint `swatch` as a two-tone chip: the accent on the left, its ink on
/// the right, with a hairline edge so no swatch vanishes into the panel
/// (and none reads as a radio button or a switch).
fn paint_swatch(ui: &mut Ui, swatch: Swatch) {
    let size = egui::vec2(SWATCH, SWATCH);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let radius = SWATCH / 4.0;
    painter.rect_filled(rect, radius, swatch.fill);
    if let Some(ink) = swatch.ink {
        let right = egui::Rect::from_min_max(egui::pos2(rect.center().x, rect.min.y), rect.max);
        let corners = egui::CornerRadius {
            nw: 0,
            sw: 0,
            ne: radius as u8,
            se: radius as u8,
        };
        painter.rect_filled(right, corners, ink);
    }
    let edge = ui.visuals().weak_text_color().gamma_multiply(0.5);
    painter.rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0, edge),
        egui::StrokeKind::Inside,
    );
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

/// The Appearance editor: the session's look, edited as a draft the window
/// previews, applied to settingsd in one change.
fn appearance(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let title = label("section-themes");
    panel::group(ui, "appearance", &[title.as_str()], |ui, _| {
        ui.horizontal(|ui| {
            // Changes apply as they are made; these appear only when an
            // edit waits for the person: a refused apply (Apply tries
            // again), a conflict to reconcile, an uncertain apply to check.
            for (id, primary, shown) in [
                ("appearance.apply", true, engine.look.hold),
                ("appearance.keep", true, !engine.look.conflicts.is_empty()),
                ("appearance.revert", false, engine.look.waiting()),
                ("appearance.recheck", false, engine.look.uncertain.is_some()),
            ] {
                if shown {
                    command_button(ui, engine, commands, strings, id, primary, events);
                }
            }
            if let Some(revision) = engine.look.revision() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let text = label_with("look-revision", &[("revision", &revision.to_string())]);
                    ui.label(RichText::new(text).weak());
                });
            }
        });
        bars::hairline(ui);
        if engine.look.settings == Settings::Missing {
            ui.add(egui::Label::new(RichText::new(label("look-missing")).weak()).wrap());
            return;
        }
        let Some(look) = engine.look.shown() else {
            ui.label(RichText::new(label("look-reading")).weak());
            return;
        };
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            look_controls(ui, engine, strings, look, events);
            bars::hairline(ui);
            let note = |ui: &mut Ui, key: &str, colour: Option<Color32>| {
                let mut text = RichText::new(label(key));
                text = match colour {
                    Some(colour) => text.color(colour),
                    None => text.weak(),
                };
                ui.add(egui::Label::new(text).wrap());
            };
            if !engine.look.conflicts.is_empty() {
                let axes: Vec<String> = engine
                    .look
                    .conflicts
                    .iter()
                    .map(|a| label(axis_label(*a)))
                    .collect();
                let text = label_with("look-conflicts", &[("axes", &axes.join(", "))]);
                ui.add(
                    egui::Label::new(RichText::new(text).color(ui.visuals().warn_fg_color)).wrap(),
                );
            }
            if engine.look.custom_source {
                note(ui, "look-custom", None);
            }
            note(
                ui,
                if engine.look.hold {
                    "look-held"
                } else {
                    "look-current"
                },
                None,
            );
        });
    });
    terminal(ui, engine, commands, strings, events);
}

/// The Terminal group: one control per `terminal.*` field, each applied as
/// it is made. The values are settingsd's, as its last read gave them.
fn terminal(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let title = label("section-terminal");
    panel::group(ui, "terminal", &[title.as_str()], |ui, _| {
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
                terminal_controls(ui, &term, events);
            });
        });
    });
}

/// The controls for each field, in the order the settings are read.
fn terminal_controls(ui: &mut Ui, term: &Terminal, events: &mut Vec<UiEvent>) {
    panel::section_label(ui, &label("terminal-family"));
    let family = family_text(term.font.family.as_deref());
    let hint = label("terminal-family-system");
    if let Some(setting) = checked_text(ui, "terminal-family", &family, &hint, family_setting) {
        events.push(UiEvent::Terminal(setting));
    }
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

    panel::section_label(ui, &label("terminal-weight"));
    let weights = [
        (300, label("terminal-weight-light")),
        (400, label("terminal-weight-regular")),
        (500, label("terminal-weight-medium")),
    ];
    if let Some(weight) = choices(ui, term.font.weight, &weights) {
        events.push(UiEvent::Terminal(Setting::Weight(weight)));
    }

    panel::section_label(ui, &label("terminal-padding"));
    let paddings: Vec<(u32, String)> = PADDINGS.iter().map(|&p| (p, p.to_string())).collect();
    if let Some(padding) = choices(ui, term.padding, &paddings) {
        events.push(UiEvent::Terminal(Setting::Padding(padding)));
    }

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

    panel::section_label(ui, &label("terminal-border"));
    let widths: Vec<(u32, String)> = (BORDER_WIDTH_MIN..=BORDER_WIDTH_MAX)
        .map(|w| (w, w.to_string()))
        .collect();
    if let Some(width) = choices(ui, term.border.width, &widths) {
        events.push(UiEvent::Terminal(Setting::BorderWidth(width)));
    }

    panel::section_label(ui, &label("terminal-cursor-shape"));
    let shapes = [
        (CursorShape::Block, label("terminal-cursor-block")),
        (CursorShape::Underline, label("terminal-cursor-underline")),
        (CursorShape::Bar, label("terminal-cursor-bar")),
    ];
    if let Some(shape) = choices(ui, term.cursor.shape, &shapes) {
        events.push(UiEvent::Terminal(Setting::CursorShape(shape)));
    }
    let mut blink = term.cursor.blink;
    if ui
        .add(Toggle::new(&mut blink, label("terminal-cursor-blink")))
        .changed()
    {
        events.push(UiEvent::Terminal(Setting::CursorBlink(blink)));
    }

    panel::section_label(ui, &label("terminal-focus-ring"));
    let rings = [
        (FocusRing::Neutral, label("terminal-focus-neutral")),
        (FocusRing::Accent, label("terminal-focus-accent")),
    ];
    if let Some(ring) = choices(ui, term.focus.ring, &rings) {
        events.push(UiEvent::Terminal(Setting::FocusRing(ring)));
    }
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

    let mut copy = term.mouse.copy_on_select;
    if ui
        .add(Toggle::new(&mut copy, label("terminal-copy-on-select")))
        .changed()
    {
        events.push(UiEvent::Terminal(Setting::CopyOnSelect(copy)));
    }

    panel::section_label(ui, &label("terminal-term"));
    let hint = "xterm-256color";
    let to_term = |text: &str| Setting::Term(text.to_owned());
    if let Some(setting) = checked_text(ui, "terminal-term", &term.env.term, hint, to_term) {
        events.push(UiEvent::Terminal(setting));
    }
    ui.add(egui::Label::new(RichText::new(label("terminal-term-note")).weak()).wrap());
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
    let id = egui::Id::new(spec.key);
    let mut value = ui.data(|d| d.get_temp::<f64>(id)).unwrap_or(current);
    let mut commit = false;
    panel::section_label(ui, &spec.title);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let field = ValueField::new(&mut value, range.clone()).unit(spec.unit);
            let response = ui.add(field);
            if response.has_focus() || response.dragged() {
                ui.data_mut(|d| d.insert_temp(id, value));
            }
            if response.lost_focus() || response.drag_stopped() {
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
    if !commit {
        return None;
    }
    ui.data_mut(|d| d.remove::<f64>(id));
    let scale = 10f64.powi(spec.places);
    let rounded = (value * scale).round() / scale;
    (rounded != (current * scale).round() / scale).then_some(rounded)
}

#[cfg(test)]
mod numeric_tests {
    use super::*;
    use egui::accesskit::Role;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn scrubbing_commits_once_on_release_and_typing_once_on_focus_loss() {
        #[derive(Default)]
        struct State {
            value: f64,
            commits: Vec<f64>,
        }
        let mut h = Harness::builder()
            .with_size(egui::vec2(640.0, 200.0))
            .build_ui_state(
                |ui, state: &mut State| {
                    let spec = Numeric {
                        key: "numeric-test",
                        title: "Number".into(),
                        unit: "",
                        places: 0,
                        slider: false,
                    };
                    if let Some(value) = number(ui, spec, state.value, 0.0..=1000.0) {
                        state.value = value;
                        state.commits.push(value);
                    }
                },
                State {
                    value: 100.0,
                    ..Default::default()
                },
            );
        toolkit::install(&h.ctx, &toolkit::Theme::embedded());
        h.run();
        let from = h.get_by_role(Role::SpinButton).rect().center();
        let to = from + egui::vec2(30.0, 0.0);
        h.hover_at(from);
        h.run();
        h.drag_at(from);
        h.run();
        h.hover_at(to);
        h.run();
        assert!(
            h.state().commits.is_empty(),
            "dragging never applies per frame"
        );
        h.drop_at(to);
        h.run();
        assert_eq!(h.state().commits.len(), 1, "one release, one apply");
        assert!(
            h.state().value > 100.0,
            "the dragged value survives the release frame"
        );
        h.get_by_role(Role::SpinButton).click();
        h.run();
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.get_by_role(Role::TextInput).type_text("250");
        h.run();
        assert_eq!(h.state().commits.len(), 1, "typing waits for focus loss");
        h.key_press(egui::Key::Enter);
        h.run();
        assert_eq!(h.state().commits.len(), 2);
        assert_eq!(h.state().value, 250.0);
    }
}

/// A text field that applies when Enter is pressed or it loses focus, not
/// per keystroke. While it is not being edited it shows `current`. `Some`
/// is the trimmed text to apply when it differs from `current`.
fn text_field(ui: &mut Ui, key: &'static str, current: &str, hint: &str) -> Option<String> {
    let id = egui::Id::new(key);
    let mut text = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| current.to_owned());
    let response = toolkit::field::text(ui, &mut text, hint, 240.0);
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
    let refused_id = egui::Id::new(key).with("refused");
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

/// The panel label naming `axis`.
fn axis_label(axis: Axis) -> &'static str {
    match axis {
        Axis::Scheme => "look-scheme",
        Axis::Style => "look-style",
        Axis::Mode => "look-mode",
        Axis::Contrast => "look-contrast",
        Axis::Decorations => "look-titlebar",
        Axis::CaptionSide => "look-captions",
    }
}

/// The six axes, each edit sent as the whole look it leaves.
fn look_controls(
    ui: &mut Ui,
    engine: &Engine,
    strings: &Strings,
    look: Look,
    events: &mut Vec<UiEvent>,
) {
    ui.add_enabled_ui(engine.can_edit_look(), |ui| {
        egui::Grid::new("look-grid")
            .num_columns(2)
            .spacing([24.0, 14.0])
            .show(ui, |ui| {
                // Two rows, as the Theme menu groups them: the hue schemes,
                // then the chrome schemes.
                panel::section_label(ui, &label("look-scheme"));
                ui.vertical(|ui| {
                    for row in Scheme::ALL.chunks(6) {
                        ui.horizontal(|ui| {
                            for &scheme in row {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                if let Some(swatch) = swatch(scheme, look.mode) {
                                    paint_swatch(ui, swatch);
                                }
                                let name = toolkit::command::label_text(
                                    strings,
                                    &format!("toolkit-theme-{}", scheme.name()),
                                );
                                if toolkit::selectable(ui, look.scheme == scheme, name).clicked()
                                    && look.scheme != scheme
                                {
                                    events.push(UiEvent::Look(Look { scheme, ..look }));
                                }
                                ui.add_space(8.0);
                            }
                        });
                    }
                });
                ui.end_row();

                panel::section_label(ui, &label("look-style"));
                let styles: Vec<(Option<Style>, String)> =
                    std::iter::once((None, label("look-style-own")))
                        .chain(
                            Style::ALL
                                .into_iter()
                                .map(|s| (Some(s), label(&format!("look-style-{}", s.name())))),
                        )
                        .collect();
                if let Some(style) = choices(ui, look.style, &styles) {
                    events.push(UiEvent::Look(Look { style, ..look }));
                }
                ui.end_row();

                panel::section_label(ui, &label("look-mode"));
                let modes: Vec<_> = Mode::ALL
                    .into_iter()
                    .map(|m| (m, label(&format!("look-mode-{}", m.name()))))
                    .collect();
                if let Some(mode) = choices(ui, look.mode, &modes) {
                    events.push(UiEvent::Look(Look { mode, ..look }));
                }
                ui.end_row();

                panel::section_label(ui, &label("look-contrast"));
                let contrasts: Vec<_> = Contrast::ALL
                    .into_iter()
                    .map(|c| (c, label(&format!("look-contrast-{}", c.name()))))
                    .collect();
                if let Some(contrast) = choices(ui, look.contrast, &contrasts) {
                    events.push(UiEvent::Look(Look { contrast, ..look }));
                }
                ui.end_row();

                panel::section_label(ui, &label("look-titlebar"));
                let frames: Vec<_> = Decorations::ALL
                    .into_iter()
                    .map(|d| (d, label(&format!("look-titlebar-{}", d.name()))))
                    .collect();
                if let Some(decorations) = choices(ui, look.decorations, &frames) {
                    events.push(UiEvent::Look(Look {
                        decorations,
                        ..look
                    }));
                }
                ui.end_row();

                // The system's title bar places its own buttons.
                panel::section_label(ui, &label("look-captions"));
                ui.add_enabled_ui(look.decorations == Decorations::Client, |ui| {
                    let sides: Vec<_> = [CaptionSide::Left, CaptionSide::Right]
                        .into_iter()
                        .map(|c| (c, label(&format!("look-captions-{}", c.name()))))
                        .collect();
                    if let Some(captions) = choices(ui, look.captions, &sides) {
                        events.push(UiEvent::Look(Look { captions, ..look }));
                    }
                });
                ui.end_row();
            });
    });
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
        Dialog::About | Dialog::Shortcuts => {
            let about = *dialog == Dialog::About;
            let title = if about { "about" } else { "shortcuts" };
            let id = egui::Id::new("dialog").with(title);
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
            let id = egui::Id::new("dialog").with("remove");
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
