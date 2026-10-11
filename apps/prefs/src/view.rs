// SPDX-License-Identifier: MIT OR Apache-2.0
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
use preferences::{AppRow, Availability, Dialog, Engine, Look, Panel, RowState};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use toolkit::button::PushButton;
use toolkit::dialog::{self, Choice};
use toolkit::titlebar::{self, Control};
use toolkit::{Icon, Registry, Strings, bars, icons, panel, tooltip};

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Command(&'static str),
    /// Select this app in the Applications table.
    Select(String),
    CloseDialog,
    /// The remove dialog's Remove button.
    ConfirmRemove,
    /// The title bar's light/dark toggle.
    SetMode(design::Mode),
    /// An edit in the Appearance panel: the whole look it leaves.
    Look(Look),
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
    for id in fired {
        if id == "view.mode" {
            if !mode_set {
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
            events.push(UiEvent::Command(id));
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
        .resizable(false)
        .default_size(176.0)
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| editors(ui, engine, commands, stroke, &mut events));
    egui::CentralPanel::default()
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| match engine.ui.panel {
            Panel::Applications => applications(ui, engine, commands, strings, &mut events),
            Panel::Appearance => appearance(ui, engine, commands, strings, &mut events),
        });
    dialogs(ui, engine, &mut events);
    toolkit::titlebar::edges(ui);
    events
}

/// The sidebar: one row per editor, as the Prefs drawer held one tool each.
fn editors(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    stroke: f32,
    events: &mut Vec<UiEvent>,
) {
    let title = label("panels");
    panel::group(ui, "editors", &[title.as_str()], |ui, _| {
        for panel in Panel::ALL {
            let (command, icon, text) = match panel {
                Panel::Applications => (
                    "view.panel.applications",
                    Icon::Package,
                    label("panel-applications"),
                ),
                Panel::Appearance => (
                    "view.panel.appearance",
                    Icon::Palette,
                    label("panel-appearance"),
                ),
            };
            let enabled = commands.get(command).is_some_and(|c| (c.enabled)(engine));
            ui.horizontal(|ui| {
                let colour = ui.visuals().text_color();
                ui.add(icons::image(icon, stroke, ICON, colour));
                let selected = engine.ui.panel == panel;
                let row = ui.add_enabled_ui(enabled, |ui| ui.selectable_label(selected, text));
                if row.inner.clicked() {
                    events.push(UiEvent::Command(command));
                }
            });
        }
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
        events.push(UiEvent::Command(command.id));
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

fn applications(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let height = ui.available_height();
    let title = label("apps-title");
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
            if engine.apps.is_empty() {
                if engine.working().is_none() {
                    ui.add(egui::Label::new(RichText::new(label("empty")).weak()).wrap());
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
            if ui.selectable_label(*value == current, text).clicked() && *value != current {
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
    let title = label("look-title");
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
        ScrollArea::vertical()
            .id_salt("look")
            .auto_shrink(false)
            .show(ui, |ui| {
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
                        egui::Label::new(RichText::new(text).color(ui.visuals().warn_fg_color))
                            .wrap(),
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
                                if ui.selectable_label(look.scheme == scheme, name).clicked()
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
            for row in &engine.apps {
                let selected = engine.ui.selected.as_deref() == Some(row.app.as_str());
                if ui.selectable_label(selected, &row.app).clicked() {
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

fn dialogs(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    let Some(dialog) = &engine.ui.dialog else {
        return;
    };
    match dialog {
        Dialog::About | Dialog::Shortcuts => {
            let (title, body) = if *dialog == Dialog::About {
                ("about", "about-body")
            } else {
                ("shortcuts", "shortcut-body")
            };
            let id = egui::Id::new("dialog").with(title);
            let shown = dialog::Dialog::new(id, label(title))
                .buttons(vec![Choice::new(label("done"), dialog::Role::Default)])
                .show(ui.ctx(), |ui| ui.add(egui::Label::new(label(body)).wrap()));
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
