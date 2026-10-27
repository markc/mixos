// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The egui rendering of the engine's state. [`view`] reads the engine and
//! returns what the person did as [`UiEvent`]s; the shell applies them after
//! the frame, so drawing never mutates state and a snapshot needs no Bus.
//!
//! The window: the toolkit title bar, a sidebar of editors (the AmigaOS
//! Prefs drawer, one window), the selected editor, and a status bar. The
//! registry supplies section order and search. Software holds the
//! followed apps, group selector and release notes. Appearance sets the
//! session's look through settingsd, each change at once; Apps holds Email,
//! Terminal and Text settings.
use crate::{label, label_with};
use design::{CaptionSide, Contrast, Mode, RequestedDecorations as Decorations, Scheme, Style};
use egui::{Align, Color32, Layout, RichText, ScrollArea, Ui};
use preferences::appearance::{AppearanceEdit, Availability as Settings, Axis};
use preferences::terminal::{
    BORDER_WIDTH_MAX, BORDER_WIDTH_MIN, CursorShape, FONT_SIZE_MAX, FONT_SIZE_MIN, FocusRing,
    PADDINGS, SCROLL_SPEED_MAX, SCROLL_SPEED_MIN, SCROLLBACK_MAX, Setting, Terminal,
    family_setting, family_text,
};
use preferences::text::InterfaceEdit;
use preferences::{
    AppRow, Availability, Category, Dialog, Engine, Look, RowState, Section, SoftwareGroup,
};
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::{Mutex, OnceLock};
use toolkit::action::Origin;
use toolkit::button::PushButton;
use toolkit::dialog::{self, Choice};
use toolkit::field::ValueField;
use toolkit::slider::Slider;
use toolkit::titlebar::{self, Action};
use toolkit::toggle::Toggle;
use toolkit::{Icon, Registry, Strings, bars, icons, panel, tooltip};
mod email;
mod text;
mod wallpaper;

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Command(&'static str, Origin),
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
    /// An edit in the Appearance panel: the whole look it leaves.
    Look(Look),
    Appearance(AppearanceEdit),
    /// An edit in the Terminal group: one `terminal.*` field.
    Terminal(Setting),
    /// An edit in the Text section: one prose surface's style.
    Text(preferences::text::Setting),
    /// An edit in the Fonts section: the session's interface weight or size.
    Interface(preferences::text::InterfaceEdit),
    WallpaperTarget(Option<String>),
    WallpaperKind(preferences::wallpaper::Kind),
    WallpaperColour(String),
    WallpaperImage(String),
    PickerDirectory(String),
    PickerSelect(String),
    PickerEnter(String),
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

const ICON: f32 = 14.0;
/// A scheme's swatch chip.
const SWATCH: f32 = 14.0;

/// Draw the whole window into `ui`.
pub fn view(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
) -> Vec<UiEvent> {
    let mut events = Vec::new();
    // The icon bar: refresh, then the standard tail.
    let fired = titlebar::show_bar(
        ui,
        &label("title"),
        Some(Icon::Settings),
        commands,
        engine,
        strings,
        &[Action::new("view.refresh", Icon::RefreshCw)],
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
        if !engine.connected {
            ui.add(icons::image_of(
                ui.ctx(),
                Icon::Unplug,
                ICON,
                ui.visuals().error_fg_color,
            ));
            ui.small(label("disconnected"));
        } else if engine.section() == Section::Software {
            let (icon, colour) = if !engine.connected {
                (Icon::Unplug, ui.visuals().error_fg_color)
            } else if engine.working().is_some() {
                (Icon::LoaderCircle, ui.visuals().weak_text_color())
            } else if engine.release_failed {
                (Icon::CircleAlert, ui.visuals().error_fg_color)
            } else {
                (Icon::Plug, ui.visuals().weak_text_color())
            };
            ui.add(icons::image_of(ui.ctx(), icon, ICON, colour));
            if engine.release_failed {
                ui.small(RichText::new(&engine.release_status).color(ui.visuals().error_fg_color));
            } else {
                ui.small(&engine.release_status);
            }
        } else {
            ui.add(icons::image_of(
                ui.ctx(),
                Icon::Plug,
                ICON,
                ui.visuals().weak_text_color(),
            ));
        }
    });
    egui::Panel::left("editors")
        .resizable(true)
        .default_size(220.0)
        .min_size(184.0)
        .max_size(320.0)
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| editors(ui, engine, commands, &mut events));
    egui::CentralPanel::default()
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| {
            page_header(ui, engine.section());
            if let Some(task) = &engine.email.task {
                let key = match task.stage {
                    preferences::email::Stage::Sending => "email-task-sending",
                };
                ui.small(RichText::new(label(key)).weak());
            } else if engine.section() == engine.look.status_source
                && let Some(status) = &engine.look.settings_status
            {
                ui.small(RichText::new(status).weak());
            }
            problem_feedback(ui, engine, &mut events);
            match engine.section() {
                Section::Software => applications(ui, engine, commands, strings, &mut events),
                section => {
                    ScrollArea::vertical()
                        .id_salt(section.name())
                        .auto_shrink(false)
                        .show(ui, |ui| match section {
                            Section::Appearance => {
                                appearance(ui, engine, commands, strings, &mut events)
                            }
                            Section::Terminal => {
                                terminal(ui, engine, commands, strings, &mut events)
                            }
                            Section::Ced => text::section(
                                ui,
                                engine,
                                &[preferences::text::Surface::CedEditor],
                                &mut events,
                            ),
                            Section::Wallpapers => {
                                wallpaper::controls(ui, engine, commands, strings, &mut events)
                            }
                            Section::TextAndFonts => {
                                fonts(ui, engine, commands, strings, &mut events)
                            }
                            Section::Windows => windows(ui, engine, commands, strings, &mut events),
                            Section::Accessibility => {
                                motion(ui, engine, commands, strings, &mut events)
                            }
                            Section::Email => {
                                email::section(ui, engine, commands, strings, &mut events)
                            }
                            _ => unreachable!(),
                        });
                }
            }
        });
    dialogs(ui, engine, commands, strings, &mut events);
    wallpaper::picker(ui, engine, &mut events);
    if toolkit::macros::show_app(ui.ctx(), "prefs", engine.ui.macros_open) {
        events.push(UiEvent::CloseMacros);
    }
    toolkit::titlebar::edges(ui);
    events
}

fn page_header(ui: &mut Ui, section: Section) {
    ui.heading(label(section.label()));
    ui.label(RichText::new(label(&format!("affects-{}", section.name()))).weak());
    bars::hairline(ui);
}

fn problem_feedback(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    if let Some(problem) = &engine.look.problem {
        let source = engine.look.problem_source.unwrap_or(Section::Appearance);
        if engine.section() == source {
            ui.horizontal(|ui| {
                if ui.button(label("problem-dismiss")).clicked() {
                    events.push(UiEvent::DismissSettingsProblem);
                }
                ui.add(
                    egui::Label::new(RichText::new(problem).color(ui.visuals().error_fg_color))
                        .wrap(),
                );
            });
        } else {
            let text = label_with("problem-on", &[("page", &label(source.label()))]);
            if ui.link(text).clicked() {
                events.push(UiEvent::Command(source.command(), Origin::Ui));
            }
        }
    }
    if engine.wallpaper.has_problem()
        && engine.section() != Section::Wallpapers
        && ui
            .link(label_with(
                "problem-on",
                &[("page", &label(Section::Wallpapers.label()))],
            ))
            .clicked()
    {
        events.push(UiEvent::Command(Section::Wallpapers.command(), Origin::Ui));
    }
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

fn signpost(ui: &mut Ui, engine: &Engine, key: &str, target: Section, events: &mut Vec<UiEvent>) {
    row(ui, key, &label(key), |ui| {
        let text = label(if target == Section::Wallpapers {
            "open-wallpaper"
        } else if target == Section::Appearance {
            "change-appearance"
        } else {
            "change-text-fonts"
        });
        if ui
            .add_enabled(engine.can_navigate(), egui::Link::new(text))
            .clicked()
        {
            events.push(UiEvent::Command(target.command(), Origin::Ui));
        }
    });
}

/// The flat sidebar and its searchable section list.
fn editors(ui: &mut Ui, engine: &Engine, commands: &Registry<Engine>, events: &mut Vec<UiEvent>) {
    let mut query = engine.ui.search.clone();
    let id = egui::Id::unique("preferences-search");
    let focused = ui.memory(|m| m.has_focus(id));
    // Escape clears a live filter even after Enter moved focus into the opened section,
    // but never while a dialog, the picker or the macros drawer owns it.
    let escape = engine.can_navigate()
        && !engine.ui.macros_open
        && (focused || !engine.ui.search.is_empty())
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let enter = focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    let field = ui.add_enabled(
        engine.can_navigate(),
        toolkit::field::text_edit(&mut query, &label("search-hint"), ui.available_width()).id(id),
    );
    if engine.ui.search_focus {
        field.request_focus();
        events.push(UiEvent::SearchFocused);
    }
    if escape {
        query.clear();
        field.surrender_focus();
    }
    let matches = engine.search(&query);
    if enter && let Some(first) = matches.first() {
        events.push(UiEvent::Command(first.command, Origin::Ui));
    }
    if query != engine.ui.search {
        events.push(UiEvent::Search(query.clone()));
    }
    ScrollArea::vertical()
        .id_salt("navigation-tree")
        .show(ui, |ui| {
            let mut previous = None;
            for section in matches
                .iter()
                .filter(|s| s.section != Section::Wallpapers || !query.is_empty())
            {
                if previous.is_some_and(|category| category != section.category) {
                    bars::hairline(ui);
                }
                if section.category == Category::Apps && previous != Some(Category::Apps) {
                    panel::section_label(ui, &label("category-apps"));
                }
                previous = Some(section.category);
                ui.add_enabled_ui(engine.can_navigate(), |ui| {
                    ui.horizontal(|ui| {
                        let command = commands.get(section.command).expect("section command");
                        ui.add(icons::image_of(
                            ui.ctx(),
                            command.icon.expect("section icon"),
                            ICON,
                            ui.visuals().text_color(),
                        ));
                        if toolkit::selectable(
                            ui,
                            sidebar_selected(engine.section(), section.section, &query),
                            label(section.title),
                        )
                        .clicked()
                        {
                            events.push(UiEvent::Command(section.command, Origin::Ui));
                        }
                    });
                });
            }
        });
}

fn sidebar_selected(current: Section, item: Section, query: &str) -> bool {
    current == item
        || (current == Section::Wallpapers && item == Section::Appearance && query.is_empty())
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

/// Preview the actual face stacks installed on this rendering context.
fn fonts(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    ui.vertical(|ui| {
        let Some(look) = appearance_state(ui, engine, commands, strings, events) else {
            return;
        };
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
        group_heading(ui, "fonts-families", true);
        egui::Grid::new("font-faces")
            .num_columns(2)
            .spacing([24.0, 14.0])
            .show(ui, |ui| {
                for (key, face) in ["fonts-interface", "fonts-headings", "fonts-monospace"]
                    .into_iter()
                    .zip(faces)
                {
                    field_label(ui, &label(key));
                    ui.label(face);
                    ui.end_row();
                }
            });
        font_family_controls(ui, engine, &look, events);
        group_heading(ui, "fonts-interface", false);
        interface_font_controls(ui, engine, events);
        group_heading(ui, "fonts-preview", false);
        ui.label(label("fonts-sample"));
        ui.monospace(label("fonts-mono-sample"));
    });
}

fn font_family_controls(ui: &mut Ui, engine: &Engine, look: &Look, events: &mut Vec<UiEvent>) {
    let families = crate::font_families::installed(ui.ctx());
    ui.add_enabled_ui(engine.can_edit_look(), |ui| {
        for (key, default, current, installed, mono) in [
            (
                "fonts-family",
                "fonts-family-design",
                look.font_family.as_deref(),
                families.map(|f| f.all.as_slice()),
                false,
            ),
            (
                "fonts-mono-family",
                "fonts-mono-family-design",
                look.mono_family.as_deref(),
                families.map(|f| f.mono.as_slice()),
                true,
            ),
        ] {
            row(ui, key, &label(key), |ui| {
                if let Some(family) = family_picker(ui, key, current, installed, label(default)) {
                    let edit = if mono {
                        AppearanceEdit::MonoFamily(family)
                    } else {
                        AppearanceEdit::FontFamily(family)
                    };
                    events.push(UiEvent::Appearance(edit));
                }
            });
        }
    });
    row(ui, "fonts-family-help", "", |ui| {
        ui.add(egui::Label::new(RichText::new(label("fonts-family-note")).weak()).wrap());
    });
}

fn family_picker(
    ui: &mut Ui,
    key: &str,
    current: Option<&str>,
    installed: Option<&[String]>,
    default: String,
) -> Option<Option<String>> {
    let mut values = vec![None];
    let mut options = vec![default];
    if let Some(installed) = installed {
        for family in installed {
            values.push(Some(family.clone()));
            options.push(family.clone());
        }
    }
    let mut selected = values
        .iter()
        .position(|family| family.as_deref() == current)
        .unwrap_or_else(|| {
            values.push(current.map(str::to_owned));
            let family = current.unwrap_or_default();
            options.push(if installed.is_some() {
                label_with("fonts-family-missing", &[("family", family)])
            } else {
                family.to_owned()
            });
            values.len() - 1
        });
    let width = ui.available_width();
    toolkit::combo::show(ui, key, &mut selected, &options, Some(width))
        .accessible_name(label(key))
        .changed()
        .then(|| values[selected].clone())
}

fn density(ui: &mut Ui, engine: &Engine, look: &Look, events: &mut Vec<UiEvent>) {
    ui.add_enabled_ui(engine.can_edit_look(), |ui| {
        row(ui, "density-row", &label("size-density"), |ui| {
            ui.horizontal_wrapped(|ui| {
                let step = toolkit::ui_scale::density_step(look.density);
                for (index, key) in ["size-compact", "size-comfortable", "size-spacious"]
                    .into_iter()
                    .enumerate()
                {
                    if toolkit::selectable(ui, step == Some(index), label(key)).clicked() {
                        events.push(UiEvent::Appearance(AppearanceEdit::Density(
                            toolkit::ui_scale::DENSITY_STEPS[index],
                        )));
                    }
                }
                if step.is_none() {
                    ui.weak(label_with(
                        "size-custom",
                        &[("value", &format!("{:.2}", look.density))],
                    ));
                }
            });
        });
    });
}

fn windows(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    if let Some(look) = appearance_state(ui, engine, commands, strings, events) {
        ui.add_enabled_ui(engine.can_edit_look(), |ui| {
            egui::Grid::new("windows-controls")
                .num_columns(2)
                .spacing([24.0, 10.0])
                .show(ui, |ui| {
                    decoration_controls(ui, look, events);
                });
        });
    }
}

fn scale_control(
    ui: &mut Ui,
    key: &'static str,
    current: f64,
    range: RangeInclusive<f64>,
    percent: bool,
) -> Option<f64> {
    let id = egui::Id::unique(key);
    let mut value = ui.data(|d| d.get_temp::<f64>(id)).unwrap_or(current);
    let mut commit = false;
    row(ui, key, &label(key), |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(PushButton::secondary(label("size-reset"))).clicked() {
                    value = 1.0;
                    commit = true;
                }
                ui.label(if percent {
                    format!("{:.0}%", value * 100.0)
                } else {
                    format!("{value:.2}")
                });
                let response = ui.add(
                    Slider::new(&mut value, range)
                        .step(0.05)
                        .wheel(0.05)
                        .reset_to(1.0)
                        .unmodified_arrows()
                        .label(label(key)),
                );
                value = (value * 20.0).round() / 20.0;
                // Hold the value while the pointer is down; apply on release.
                if response.dragged() || response.is_pointer_button_down_on() {
                    ui.data_mut(|d| d.insert_temp(id, value));
                } else if response.drag_stopped() || response.changed() || response.clicked() {
                    commit = true;
                }
            });
        });
    });
    if commit {
        ui.data_mut(|d| d.remove::<f64>(id));
        (value != current).then_some(value)
    } else {
        None
    }
}

fn motion(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    ui.vertical(|ui| {
        let Some(look) = appearance_state(ui, engine, commands, strings, events) else {
            return;
        };
        ui.add_enabled_ui(engine.can_edit_look(), |ui| {
            if let Some(scale) =
                scale_control(ui, "size-text-scale", look.text_scale, 0.5..=3.0, true)
            {
                events.push(UiEvent::Appearance(AppearanceEdit::TextScale(scale)));
            }
            row(ui, "text-scale-note", "", |ui| {
                ui.weak(label("size-text-scale-note"));
            });
            row(ui, "reduced-motion", &label("motion-reduce"), |ui| {
                let mut reduced = look.reduced_motion;
                if toggle(ui, &mut reduced, "motion-reduce") {
                    events.push(UiEvent::Appearance(AppearanceEdit::ReducedMotion(reduced)));
                }
                ui.add(egui::Label::new(RichText::new(label("motion-note")).weak()).wrap());
            });
        });
        signpost(ui, engine, "related-contrast", Section::Appearance, events);
        signpost(
            ui,
            engine,
            "related-text-size",
            Section::TextAndFonts,
            events,
        );
    });
}

/// The session's interface weight and size. Each applies as it is made; the
/// values are settingsd's, as its last read gave them.
fn interface_font_controls(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    let Some(font) = engine.look.interface_font.clone() else {
        ui.label(RichText::new(label("look-reading")).weak());
        return;
    };
    ui.add_enabled_ui(engine.can_edit_interface_font(), |ui| {
        row(ui, "interface-weight-row", &label("fonts-weight"), |ui| {
            let weights = [
                (None, label("fonts-weight-design")),
                (Some(300), label("fonts-weight-light")),
                (Some(400), label("fonts-weight-regular")),
                (Some(500), label("fonts-weight-medium")),
                (Some(600), label("fonts-weight-semibold")),
            ];
            if let Some(weight) = choices(ui, font.weight, &weights) {
                events.push(UiEvent::Interface(InterfaceEdit::Weight(weight)));
            }
        });

        let design = design::default_typography(design::TypographyRole::Ui).font_size;
        {
            let size = font.size.unwrap_or(design);
            let spec = Numeric {
                key: "fonts-size",
                title: label("fonts-size"),
                unit: "px",
                places: 1,
                slider: true,
            };
            let range =
                preferences::text::INTERFACE_SIZE_MIN..=preferences::text::INTERFACE_SIZE_MAX;
            if let Some(value) = number(ui, spec, size, range) {
                events.push(UiEvent::Interface(InterfaceEdit::Size(Some(value))));
            }
            row(ui, "interface-size-default", "", |ui| {
                let text = label("fonts-size-design");
                if ui
                    .add_enabled(font.size.is_some(), PushButton::secondary(text))
                    .clicked()
                {
                    events.push(UiEvent::Interface(InterfaceEdit::Size(None)));
                }
            });
        }
    });
}

fn applications(
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

/// Equal-width columns, with choices spread evenly across the required rows.
fn choice_grid<T>(
    ui: &mut Ui,
    options: &[(T, String)],
    extra_width: f32,
    mut render: impl FnMut(&mut Ui, &T, &str),
) {
    if options.is_empty() {
        return;
    }
    let gap = ui.spacing().item_spacing.x;
    let widest = options.iter().fold(0.0_f32, |width, (_, text)| {
        let galley = ui.painter().layout_no_wrap(
            text.clone(),
            egui::TextStyle::Button.resolve(ui.style()),
            ui.visuals().text_color(),
        );
        width.max(galley.size().x + 2.0 * ui.spacing().button_padding.x + extra_width)
    });
    let available = ui.available_width();
    let columns = (((available + gap) / (widest + gap)).floor() as usize).clamp(1, options.len());
    let rows = options.len().div_ceil(columns);
    let width = ((available - gap * (columns - 1) as f32) / columns as f32).max(0.0);
    let mut start = 0;
    ui.vertical(|ui| {
        for row in 0..rows {
            let count = options.len() / rows + usize::from(row < options.len() % rows);
            ui.horizontal(|ui| {
                for (value, text) in &options[start..start + count] {
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, ui.spacing().interact_size.y),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.set_min_width(width);
                            render(ui, value, text);
                        },
                    );
                }
            });
            start += count;
        }
    });
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
    ui.vertical(|ui| {
        if let Some(look) = appearance_state(ui, engine, commands, strings, events) {
            group_heading(ui, "appearance-theme", true);
            look_controls(ui, engine, strings, look.clone(), events);
            row(ui, "style-note", "", |ui| {
                ui.add(egui::Label::new(RichText::new(label("look-style-note")).weak()).wrap());
            });
            density(ui, engine, &look, events);
            signpost(ui, engine, "related-wallpaper", Section::Wallpapers, events);
        }
    });
}

/// Shared state and recovery controls for the session appearance editors.
fn appearance_state(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) -> Option<Look> {
    if engine.look.waiting() || engine.look.uncertain.is_some() {
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
        });
        bars::hairline(ui);
    }
    if engine.look.settings == Settings::Missing {
        ui.add(egui::Label::new(RichText::new(label("look-missing")).weak()).wrap());
        return None;
    }
    let Some(look) = engine.look.shown() else {
        ui.label(RichText::new(label("look-reading")).weak());
        return None;
    };
    if !engine.look.conflicts.is_empty() || engine.look.custom_source || engine.look.hold {
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
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
            if engine.look.hold {
                note(ui, "look-held", None);
            }
        });
        bars::hairline(ui);
    }
    Some(look)
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

/// The panel label naming `axis`.
fn axis_label(axis: Axis) -> &'static str {
    match axis {
        Axis::Scheme => "look-scheme",
        Axis::Style => "look-style",
        Axis::Mode => "look-mode",
        Axis::Contrast => "look-contrast",
        Axis::Decorations => "look-titlebar",
        Axis::CaptionSide => "look-captions",
        Axis::WindowStyle => "look-window-style",
        Axis::FontFamily => "fonts-family",
        Axis::MonoFamily => "fonts-mono-family",
        Axis::TextScale => "size-text-scale",
        Axis::Density => "size-density",
        Axis::ReducedMotion => "motion-reduce",
    }
}

/// The six theme axes, each edit sent as the whole look it leaves.
fn look_controls(
    ui: &mut Ui,
    engine: &Engine,
    strings: &Strings,
    look: Look,
    events: &mut Vec<UiEvent>,
) {
    ui.add_enabled_ui(engine.can_edit_look(), |ui| {
        row(ui, "look-scheme", &label("look-scheme"), |ui| {
            let schemes: Vec<_> = Scheme::ALL
                .into_iter()
                .map(|scheme| {
                    let name = toolkit::action::label_text(
                        strings,
                        &format!("toolkit-theme-{}", scheme.name()),
                    );
                    (scheme, name)
                })
                .collect();
            choice_grid(
                ui,
                &schemes,
                SWATCH + ui.spacing().item_spacing.x,
                |ui, &scheme, name| {
                    if let Some(swatch) = swatch(scheme, look.mode) {
                        paint_swatch(ui, swatch);
                    }
                    if toolkit::selectable(ui, look.scheme == scheme, name).clicked()
                        && look.scheme != scheme
                    {
                        events.push(UiEvent::Look(Look {
                            scheme,
                            ..look.clone()
                        }));
                    }
                },
            );
        });

        row(ui, "look-mode", &label("look-mode"), |ui| {
            let modes: Vec<_> = Mode::ALL
                .into_iter()
                .map(|m| (m, label(&format!("look-mode-{}", m.name()))))
                .collect();
            if let Some(mode) = choices(ui, look.mode, &modes) {
                events.push(UiEvent::Look(Look {
                    mode,
                    ..look.clone()
                }));
            }
        });

        row(ui, "look-contrast", &label("look-contrast"), |ui| {
            let contrasts: Vec<_> = Contrast::ALL
                .into_iter()
                .map(|c| (c, label(&format!("look-contrast-{}", c.name()))))
                .collect();
            if let Some(contrast) = choices(ui, look.contrast, &contrasts) {
                events.push(UiEvent::Look(Look {
                    contrast,
                    ..look.clone()
                }));
            }
        });
        row(ui, "look-style", &label("look-style"), |ui| {
            let styles: Vec<(Option<Style>, String)> =
                std::iter::once((None, label("look-style-own")))
                    .chain(
                        Style::ALL
                            .into_iter()
                            .map(|s| (Some(s), label(&format!("look-style-{}", s.name())))),
                    )
                    .collect();
            choice_grid(ui, &styles, 0.0, |ui, &style, text| {
                if toolkit::selectable(ui, look.style == style, text).clicked()
                    && look.style != style
                {
                    events.push(UiEvent::Look(Look {
                        style,
                        ..look.clone()
                    }));
                }
            });
        });
    });
}

fn decoration_controls(ui: &mut Ui, look: Look, events: &mut Vec<UiEvent>) {
    let resolved = look.decoration_resolution();
    field_label(ui, &label("look-titlebar"));
    ui.vertical(|ui| {
        let frames: Vec<_> = Decorations::ALL
            .into_iter()
            .map(|d| (d, label(&format!("look-titlebar-{}", d.name()))))
            .collect();
        if let Some(decorations) = choices(ui, look.decorations, &frames) {
            events.push(UiEvent::Look(Look {
                decorations,
                ..look.clone()
            }));
        }
        if look.decorations == Decorations::Auto {
            ui.label(label(&format!(
                "look-titlebar-resolved-{}",
                resolved.mode.name()
            )));
        }
    });
    ui.end_row();

    field_label(ui, &label("look-window-style"));
    let styles: Vec<_> = design::WindowStyle::ALL
        .into_iter()
        .map(|style| (style.name(), label(style.label_id())))
        .collect();
    if let Some(style) = choices(ui, look.window_style.as_str(), &styles) {
        events.push(UiEvent::Look(Look {
            window_style: style.into(),
            ..look.clone()
        }));
    }
    ui.end_row();

    // The system's title bar places its own buttons.
    field_label(ui, &label("look-captions"));
    ui.add_enabled_ui(resolved.mode == design::Decorations::Client, |ui| {
        let sides: Vec<_> = [CaptionSide::Left, CaptionSide::Right]
            .into_iter()
            .map(|c| (c, label(&format!("look-captions-{}", c.name()))))
            .collect();
        if let Some(captions) = choices(ui, look.captions, &sides) {
            events.push(UiEvent::Look(Look {
                captions,
                ..look.clone()
            }));
        }
    });
    ui.end_row();
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

#[cfg(test)]
mod numeric_tests {
    use super::*;
    use egui::accesskit::Role;
    use egui_kittest::{Harness, kittest::NodeT, kittest::Queryable};

    fn ready_engine() -> Engine {
        let mut engine = Engine::new(crate::label_with);
        engine.start_appearance("example");
        let ticket = engine
            .take_effects()
            .into_iter()
            .find_map(|effect| match effect {
                preferences::Effect::Settings { ticket, .. } => Some(ticket),
                _ => None,
            })
            .unwrap();
        let body = serde_json::json!({"summary":{"schema":1,"binding":{"instance":"example","profile":"default"},
            "incarnation":"example","revision":"1","design_revision":"1","source_digest":"example",
            "appearance":{"scheme":"studio","mode":"dark","contrast":"normal","source":null},
            "custom_source":false,"terminal":{}},"recovering":false});
        engine.settled(
            ticket,
            Ok(citizen::Reply {
                rc: 0,
                body: body.to_string(),
                error: None,
            }),
        );
        engine.take_effects();
        engine
    }

    #[test]
    fn interface_size_is_always_one_pixel_control_even_at_the_design_default() {
        for size in [None, Some(16.0)] {
            let mut engine = ready_engine();
            engine.look.interface_font =
                Some(preferences::appearance::InterfaceFont { weight: None, size });
            let mut h = Harness::builder()
                .with_size(egui::vec2(720.0, 300.0))
                .build_ui_state(
                    |ui, state: &mut (Engine, Vec<UiEvent>)| {
                        interface_font_controls(ui, &state.0, &mut state.1);
                    },
                    (engine, Vec::new()),
                );
            toolkit::install(&h.ctx, &toolkit::Theme::embedded());
            h.run();
            assert_eq!(h.get_all_by_role(Role::Slider).count(), 1);
            h.key_press(egui::Key::Tab);
            h.run();
            h.key_press(egui::Key::Tab);
            h.run();
            assert!(h.state().1.is_empty(), "viewing and tabbing write nothing");
            h.get_by_label("Use design default");
        }
    }

    #[test]
    fn interface_size_reset_emits_null() {
        let mut engine = ready_engine();
        engine.look.interface_font = Some(preferences::appearance::InterfaceFont {
            weight: None,
            size: Some(16.0),
        });
        engine.look.settings = Settings::Available;
        let mut h = Harness::builder()
            .with_size(egui::vec2(720.0, 360.0))
            .build_ui_state(
                |ui, state: &mut (Engine, Vec<UiEvent>)| {
                    interface_font_controls(ui, &state.0, &mut state.1);
                },
                (engine, Vec::new()),
            );
        toolkit::install(&h.ctx, &toolkit::Theme::embedded());
        h.run();
        h.get_by_label("Use design default").click();
        h.run();
        assert_eq!(h.state().1, [UiEvent::Interface(InterfaceEdit::Size(None))]);
        assert_eq!(
            InterfaceEdit::Size(None).edit().value,
            serde_json::Value::Null
        );
    }

    #[test]
    fn size_fields_focus_and_blur_without_typing_emit_no_edits() {
        for size in [None, Some(16.0)] {
            for interface in [true, false] {
                let mut engine = ready_engine();
                engine.look.interface_font =
                    Some(preferences::appearance::InterfaceFont { weight: None, size });
                let mut h = Harness::builder()
                    .with_size(egui::vec2(720.0, 360.0))
                    .build_ui_state(
                        move |ui, state: &mut (Engine, Vec<UiEvent>)| {
                            if interface {
                                interface_font_controls(ui, &state.0, &mut state.1);
                            } else if let Some(value) = scale_control(
                                ui,
                                "size-text-scale",
                                if size.is_some() { 1.25 } else { 1.0 },
                                0.5..=3.0,
                                true,
                            ) {
                                state
                                    .1
                                    .push(UiEvent::Appearance(AppearanceEdit::TextScale(value)));
                            }
                            let _ = ui.button("Blur target");
                        },
                        (engine, Vec::new()),
                    );
                toolkit::install(&h.ctx, &toolkit::Theme::embedded());
                h.run();
                if interface {
                    h.get_by_role(Role::SpinButton).click();
                    h.run();
                    h.get_by_role(Role::TextInput);
                } else {
                    h.get_by_role(Role::Slider).focus();
                    h.run();
                }
                h.get_by_label("Blur target").click();
                h.run();
                assert!(h.state().1.is_empty(), "unchanged field emitted an edit");
            }
        }
    }

    #[test]
    fn writing_light_toggle_changes_only_writing() {
        use preferences::text::{Setting, Surface};
        let mut engine = ready_engine();
        engine.look.email = Some(Default::default());
        engine.look.ced = Some(Default::default());
        let mut h = Harness::builder()
            .with_size(egui::vec2(900.0, 900.0))
            .build_ui_state(
                |ui, state: &mut (Engine, Vec<UiEvent>)| {
                    let mut events = Vec::new();
                    text::section(
                        ui,
                        &state.0,
                        &[Surface::EmailReader, Surface::EmailComposer],
                        &mut events,
                    );
                    for event in events {
                        if let UiEvent::Text(Setting::Light(surface, value)) = event {
                            let email = state.0.look.email.as_mut().unwrap();
                            match surface {
                                Surface::EmailReader => email.reader.light = value,
                                Surface::EmailComposer => email.composer.light = value,
                                _ => unreachable!(),
                            }
                        }
                        state.1.push(event);
                    }
                },
                (engine, Vec::new()),
            );
        toolkit::install(&h.ctx, &toolkit::Theme::embedded());
        h.run();
        let reading = h.state().0.look.email.as_ref().unwrap().reader.light;
        let writing = h.state().0.look.email.as_ref().unwrap().composer.light;
        h.get_all_by_role(Role::CheckBox).nth(1).unwrap().click();
        h.run();
        assert_eq!(
            h.state().1,
            [UiEvent::Text(Setting::Light(
                Surface::EmailComposer,
                !writing
            ))]
        );
        let email = h.state().0.look.email.as_ref().unwrap();
        assert_eq!(email.reader.light, reading);
        assert_eq!(email.composer.light, !writing);
    }

    #[test]
    fn empty_choice_grid_is_safe() {
        let mut h = Harness::builder().build_ui(|ui| {
            choice_grid::<()>(ui, &[], 0.0, |_, _, _| panic!("empty choice rendered"));
        });
        h.run();
    }

    #[test]
    fn settings_problem_banner_emits_dismiss() {
        let mut engine = ready_engine();
        engine.set_section(Section::Terminal);
        engine.look.problem = Some("Refused terminal edit".into());
        engine.look.problem_source = Some(Section::Terminal);
        let mut h = Harness::builder().build_ui_state(
            |ui, state: &mut (Engine, Vec<UiEvent>)| {
                problem_feedback(ui, &state.0, &mut state.1);
            },
            (engine, Vec::new()),
        );
        h.run();
        h.get_by_label("×").click();
        h.run();
        assert_eq!(h.state().1, [UiEvent::DismissSettingsProblem]);
    }

    #[test]
    fn absent_backdrop_has_no_links_but_apply_refusal_does() {
        for section in preferences::SECTIONS {
            for refused in [false, true] {
                let mut engine = ready_engine();
                engine.set_section(section.section);
                engine.wallpaper.read(100);
                engine.wallpaper.complete(
                    100,
                    Err(citizen::CallError {
                        message: "absent".into(),
                        outcome_unknown: false,
                    }),
                    101,
                );
                if refused {
                    let state = || {
                        Ok(citizen::Reply { rc: 0, body: serde_json::json!({"schema":"backdrop.v1", "config":{"schema":1}, "pending":null, "outputs":[]}).to_string(), error: None })
                    };
                    engine.wallpaper.read(102);
                    engine.wallpaper.complete(102, state(), 103);
                    engine.wallpaper.edit_colour("#123456".into()).unwrap();
                    engine.wallpaper.apply(104).unwrap();
                    engine.wallpaper.complete(104, state(), 105).unwrap();
                    engine.wallpaper.complete(
                        105,
                        Ok(citizen::Reply {
                            rc: 10,
                            body: "{}".into(),
                            error: Some("refused".into()),
                        }),
                        106,
                    );
                }
                let mut h = Harness::builder().build_ui(move |ui| {
                    problem_feedback(ui, &engine, &mut Vec::new());
                });
                h.run();
                assert_eq!(
                    h.query_by_label("Problem on Wallpaper ›").is_some(),
                    refused && section.section != Section::Wallpapers
                );
            }
        }
    }

    #[test]
    fn settings_banner_is_scoped_to_its_page_and_links_from_others() {
        for page in [Section::Appearance, Section::Terminal, Section::Software] {
            let mut engine = Engine::new(crate::label_with);
            engine.set_section(page);
            engine.look.problem = Some("Refused terminal edit".into());
            engine.look.problem_source = Some(Section::Terminal);
            let mut h = Harness::builder().build_ui(move |ui| {
                problem_feedback(ui, &engine, &mut Vec::new());
            });
            h.run();
            assert_eq!(
                h.query_by_label("Refused terminal edit").is_some(),
                page == Section::Terminal
            );
            assert_eq!(
                h.query_by_label("Problem on Term ›").is_some(),
                page != Section::Terminal
            );
        }
    }

    #[test]
    fn wallpaper_sidebar_selection_and_filter_stay_in_appearance() {
        assert!(sidebar_selected(
            Section::Wallpapers,
            Section::Appearance,
            ""
        ));
        assert!(!sidebar_selected(
            Section::Wallpapers,
            Section::Appearance,
            "wallpapers"
        ));
        assert!(sidebar_selected(
            Section::Wallpapers,
            Section::Wallpapers,
            "wallpapers"
        ));
        let engine = Engine::new(crate::label_with);
        let matches = engine.search("appearance");
        assert_eq!(matches[2].section, Section::Wallpapers);
        assert!(
            matches
                .iter()
                .position(|s| s.section == Section::Wallpapers)
                .unwrap()
                < preferences::SECTIONS
                    .iter()
                    .position(|s| s.section == Section::Windows)
                    .unwrap()
        );
        for query in ["", "wallpapers", "appearance"] {
            let mut engine = ready_engine();
            engine.set_section(Section::Wallpapers);
            engine.ui.search = query.into();
            let commands = crate::actions::registry();
            let mut h = Harness::builder()
                .with_size(egui::vec2(300.0, 600.0))
                .build_ui(move |ui| {
                    editors(ui, &engine, &commands, &mut Vec::new());
                });
            toolkit::install(&h.ctx, &toolkit::Theme::embedded());
            h.run();
            if query.is_empty() {
                assert!(h.query_by_label("Wallpaper").is_none());
                assert_eq!(
                    h.get_by_label("Appearance").accesskit_node().toggled(),
                    Some(egui::accesskit::Toggled::True)
                );
            } else {
                assert_eq!(
                    h.get_by_label("Wallpaper").accesskit_node().toggled(),
                    Some(egui::accesskit::Toggled::True)
                );
            }
        }
    }

    #[test]
    fn off_step_density_stays_visible_as_custom() {
        let engine = Engine::new(crate::label_with);
        let mut h = Harness::builder()
            .with_size(egui::vec2(720.0, 200.0))
            .build_ui(move |ui| {
                density(
                    ui,
                    &engine,
                    &Look {
                        density: 1.1,
                        ..Look::default()
                    },
                    &mut Vec::new(),
                );
            });
        toolkit::install(&h.ctx, &toolkit::Theme::embedded());
        h.run();
        h.get_by_label("Custom (1.10)");
    }

    #[test]
    fn scale_drag_applies_once_steps_are_discrete_and_reset_restores_one() {
        struct State {
            value: f64,
            commits: Vec<f64>,
        }
        let mut h = Harness::builder()
            .with_size(egui::vec2(640.0, 200.0))
            .build_ui_state(
                |ui, state: &mut State| {
                    if let Some(value) =
                        scale_control(ui, "size-text-scale", state.value, 0.5..=3.0, true)
                    {
                        state.value = value;
                        state.commits.push(value);
                    }
                },
                State {
                    value: 1.0,
                    commits: Vec::new(),
                },
            );
        toolkit::install(&h.ctx, &toolkit::Theme::embedded());
        h.run();
        let from = h.get_by_role(Role::Slider).rect().center();
        let to = from + egui::vec2(40.0, 0.0);
        h.hover_at(from);
        h.run();
        h.drag_at(from);
        h.run();
        h.hover_at(to);
        h.run();
        assert!(h.state().commits.is_empty());
        h.drop_at(to);
        h.run();
        assert_eq!(h.state().commits.len(), 1);
        assert_eq!(h.state().value * 20.0, (h.state().value * 20.0).round());
        h.get_by_label("Reset").click();
        h.run();
        assert_eq!(h.state().value, 1.0);
        assert_eq!(h.state().commits.len(), 2);
        let slider = h.get_by_role(Role::Slider).rect().center();
        h.hover_at(slider);
        h.run();
        h.get_by_role(Role::Slider).focus();
        h.run();
        h.key_press(egui::Key::ArrowRight);
        h.run();
        assert_eq!(h.state().value, 1.05);
        for modifiers in [
            egui::Modifiers::CTRL,
            egui::Modifiers::ALT,
            egui::Modifiers::SHIFT,
        ] {
            h.key_press_modifiers(modifiers, egui::Key::ArrowRight);
            h.run();
            assert_eq!(
                h.state().value,
                1.05,
                "modified arrows belong to the person"
            );
        }
        // A click on the track, with no drag, applies on release.
        let commits = h.state().commits.len();
        let rect = h.get_by_role(Role::Slider).rect();
        let at = egui::pos2(rect.left() + rect.width() * 0.75, rect.center().y);
        h.drag_at(at);
        h.run();
        assert_eq!(h.state().commits.len(), commits);
        h.drop_at(at);
        h.run();
        assert_eq!(h.state().commits.len(), commits + 1);
        assert!(h.state().value > 1.05);
    }

    #[test]
    fn a_missing_stored_family_stays_visible_and_can_reset_to_null() {
        let mut h = Harness::builder()
            .with_size(egui::vec2(640.0, 200.0))
            .build_ui_state(
                |ui, family: &mut Option<String>| {
                    let installed = vec!["Example Sans".to_owned()];
                    if let Some(next) = family_picker(
                        ui,
                        "family-test",
                        family.as_deref(),
                        Some(&installed),
                        label("fonts-family-design"),
                    ) {
                        *family = next;
                    }
                },
                Some("Missing Sans".to_owned()),
            );
        toolkit::install(&h.ctx, &toolkit::Theme::embedded());
        h.run();
        let combo = h.get_by_role(Role::ComboBox);
        assert_eq!(combo.value().as_deref(), Some("Missing Sans (missing)"));
        combo.click();
        h.run();
        h.get_by_label("Design default (Inter)").click();
        h.run();
        assert!(h.state().is_none());
    }

    #[test]
    fn window_style_picker_labels_and_choices_follow_the_registry() {
        let mut h = Harness::builder()
            .with_size(egui::vec2(720.0, 300.0))
            .build_ui_state(
                |ui, look: &mut Look| {
                    let mut events = Vec::new();
                    egui::Grid::new("test-window-styles")
                        .num_columns(2)
                        .show(ui, |ui| decoration_controls(ui, look.clone(), &mut events));
                    for event in events {
                        if let UiEvent::Look(next) = event {
                            *look = next;
                        }
                    }
                },
                Look {
                    window_style: "future-style".into(),
                    ..Look::default()
                },
            );
        toolkit::install(&h.ctx, &toolkit::Theme::embedded());
        h.run();
        h.get_by_label("Window style");
        assert_eq!(label(design::WindowStyle::Mac.label_id()), "MacOS");
        assert_eq!(label(design::WindowStyle::Win11.label_id()), "Win11");
        assert_eq!(label(design::WindowStyle::Mixos.label_id()), "MixOS");
        h.get_by_label("MacOS").click();
        h.run();
        assert_eq!(h.state().window_style, "mac");
        for style in design::WindowStyle::ALL.into_iter().rev() {
            h.get_by_label(&label(style.label_id())).click();
            h.run();
            assert_eq!(h.state().window_style, style.name());
        }
    }

    #[test]
    fn automatic_titlebar_shows_effective_mode_and_gates_caption_side() {
        use design::decorations::{Resolver, SessionFacts, with_resolver};
        use std::sync::Arc;
        for (desktop, expected, enabled) in [
            ("KDE", "Currently drawn by the system", false),
            ("MixOS", "Currently drawn by the app", true),
        ] {
            let resolver = Arc::new(Resolver::new(
                move || SessionFacts {
                    wayland: true,
                    desktops: desktop.into(),
                    decoration_global: Some(true),
                },
                || None,
                |_| {},
            ));
            with_resolver(resolver, || {
                let mut h = Harness::builder()
                    .with_size(egui::vec2(720.0, 240.0))
                    .build_ui_state(
                        |ui, look: &mut Look| {
                            let mut events = Vec::new();
                            egui::Grid::new("test-decorations")
                                .num_columns(2)
                                .show(ui, |ui| {
                                    decoration_controls(ui, look.clone(), &mut events);
                                });
                            for event in events {
                                if let UiEvent::Look(next) = event {
                                    *look = next;
                                }
                            }
                        },
                        Look {
                            decorations: Decorations::Client,
                            ..Look::default()
                        },
                    );
                toolkit::install(&h.ctx, &toolkit::Theme::embedded());
                h.run();
                h.get_by_label("Automatic").click();
                h.run();
                assert_eq!(h.state().decorations, Decorations::Auto);
                h.get_by_label(expected);
                assert_eq!(
                    h.get_by_label("Left").accesskit_node().is_disabled(),
                    !enabled
                );
                assert_eq!(
                    h.get_by_label("Right").accesskit_node().is_disabled(),
                    !enabled
                );
            });
        }
    }

    #[test]
    fn scrubbing_and_enter_commit_once_and_escape_cancels_typing() {
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
        h.get_by_role(Role::SpinButton).click();
        h.run();
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.get_by_role(Role::TextInput).type_text("350");
        h.run();
        assert_eq!(h.state().commits.len(), 2);
        h.key_press(egui::Key::Escape);
        h.run();
        assert_eq!(h.state().commits.len(), 2, "Escape sends no apply");
        assert_eq!(h.state().value, 250.0);
        assert!(
            h.ctx
                .data(|d| d.get_temp::<f64>(egui::Id::unique("numeric-test")))
                .is_none()
        );
        h.get_by_role(Role::SpinButton).click();
        h.run();
        assert_eq!(
            h.get_by_role(Role::TextInput).value().as_deref(),
            Some("250")
        );
        h.key_press(egui::Key::Escape);
        h.run();
        assert_eq!(h.state().commits.len(), 2);
    }
}
