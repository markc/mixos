// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
use super::{UiEvent, command_button, row};
use crate::label;
use egui::{RichText, Ui};
use preferences::{Engine, wallpaper::Kind};
use toolkit::action::Origin;
use toolkit::{
    Registry, Strings, bars,
    dialog::{self, Choice},
};

pub fn controls(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let editor = &engine.wallpaper;
    let fill = editor.fill();
    let kind = engine.ui.wallpaper_kind.unwrap_or_else(|| {
        if fill["image"].is_string() {
            Kind::Image
        } else if fill["colour"].is_string() {
            Kind::Colour
        } else {
            Kind::Theme
        }
    });
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            command_button(
                ui,
                engine,
                commands,
                strings,
                "wallpaper.apply",
                true,
                events,
            );
            command_button(
                ui,
                engine,
                commands,
                strings,
                "wallpaper.revert",
                false,
                events,
            );
            command_button(
                ui,
                engine,
                commands,
                strings,
                "wallpaper.refresh",
                false,
                events,
            );
        });
        bars::hairline(ui);
        ui.add_enabled_ui(engine.can_edit_wallpaper(), |ui| {
            row(
                ui,
                "wallpaper-output-row",
                &label("wallpaper-output"),
                |ui| {
                    let mut target = editor.target.clone();
                    egui::ComboBox::from_id_salt("wallpaper-output")
                        .selected_text(target.clone().unwrap_or_else(|| label("wallpaper-default")))
                        .show_ui(ui, |ui| {
                            if toolkit::selectable(ui, target.is_none(), label("wallpaper-default"))
                                .clicked()
                            {
                                target = None;
                                ui.close();
                            }
                            for name in &editor.outputs {
                                if toolkit::selectable(ui, target.as_deref() == Some(name), name)
                                    .clicked()
                                {
                                    target = Some(name.clone());
                                    ui.close();
                                }
                            }
                        });
                    if target != editor.target {
                        events.push(UiEvent::WallpaperTarget(target));
                    }
                },
            );
            row(ui, "wallpaper-kind-row", &label("wallpaper-kind"), |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (value, key) in [
                        (
                            Kind::Theme,
                            if editor.target.is_some() {
                                "wallpaper-inherit"
                            } else {
                                "wallpaper-theme"
                            },
                        ),
                        (Kind::Colour, "wallpaper-colour"),
                        (Kind::Image, "wallpaper-image"),
                    ] {
                        if toolkit::selectable(ui, kind == value, label(key)).clicked()
                            && kind != value
                        {
                            events.push(UiEvent::WallpaperKind(value));
                        }
                    }
                });
            });
            let fill_label = match kind {
                Kind::Colour => label("wallpaper-hex"),
                Kind::Image => label("wallpaper-path"),
                Kind::Theme => String::new(),
            };
            row(ui, "wallpaper-fill-row", &fill_label, |ui| match kind {
                Kind::Theme => {
                    ui.label(
                        RichText::new(label(if editor.target.is_some() {
                            "wallpaper-inherit-note"
                        } else {
                            "wallpaper-theme-note"
                        }))
                        .weak(),
                    );
                }
                Kind::Colour => {
                    let mut text = engine
                        .ui
                        .wallpaper_colour
                        .clone()
                        .or_else(|| fill["colour"].as_str().map(str::to_owned))
                        .unwrap_or_else(|| "#000000".into());
                    ui.horizontal(|ui| {
                        if let Some(mut rgb) = rgb(&text)
                            && ui.color_edit_button_srgb(&mut rgb).changed()
                        {
                            text = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                            events.push(UiEvent::WallpaperColour(text.clone()));
                        }
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut text)
                                    .desired_width(ui.spacing().interact_size.x * 3.0)
                                    .hint_text("#RRGGBB"),
                            )
                            .on_hover_text(label("wallpaper-hex"))
                            .changed()
                        {
                            events.push(UiEvent::WallpaperColour(text));
                        }
                    });
                }
                Kind::Image => {
                    let mut path = engine
                        .ui
                        .wallpaper_image
                        .clone()
                        .or_else(|| fill["image"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut path)
                                .desired_width(ui.available_width())
                                .hint_text(label("wallpaper-path-hint")),
                        )
                        .on_hover_text(label("wallpaper-path"))
                        .changed()
                    {
                        events.push(UiEvent::WallpaperImage(path));
                    }
                    command_button(
                        ui,
                        engine,
                        commands,
                        strings,
                        "wallpaper.browse",
                        false,
                        events,
                    );
                    ui.label(RichText::new(label("wallpaper-image-note")).weak());
                }
            });
        });
        let key = format!(
            "wallpaper-{}",
            editor
                .status
                .strip_prefix("status.")
                .unwrap_or("loading")
                .replace('_', "-")
        );
        let text = RichText::new(label(&key));
        ui.add(
            egui::Label::new(if editor.failed {
                text.color(ui.visuals().error_fg_color)
            } else {
                text.weak()
            })
            .wrap(),
        );
        if let Some(detail) = &editor.detail {
            ui.add(egui::Label::new(detail).wrap());
        }
    });
}

fn rgb(text: &str) -> Option<[u8; 3]> {
    if text.len() != 7 || !text.starts_with('#') {
        return None;
    }
    let mut out = [0; 3];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(text.get(index * 2 + 1..index * 2 + 3)?, 16).ok()?;
    }
    Some(out)
}

pub fn picker(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    let picker = &engine.picker;
    if !picker.open {
        return;
    }
    let id = egui::Id::new("wallpaper-picker");
    let shown = dialog::Dialog::new(id, label("wallpaper-browser"))
        .width(egui::Rangef::new(560.0, 640.0))
        .buttons(vec![
            Choice::new(label("wallpaper-use-image"), dialog::Role::Default)
                .enabled(engine.can_picker_use()),
            Choice::new(label("cancel"), dialog::Role::Cancel),
        ])
        .show(ui.ctx(), |ui| {
            ui.add_enabled_ui(engine.can_picker_go(), |ui| {
                let mut path = picker.directory.clone();
                let caption = ui.label(label("wallpaper-directory"));
                if ui
                    .add(egui::TextEdit::singleline(&mut path).desired_width(ui.available_width()))
                    .labelled_by(caption.id)
                    .changed()
                {
                    events.push(UiEvent::PickerDirectory(path));
                }
                ui.horizontal(|ui| {
                    for (id, key) in [
                        ("wallpaper.picker.go", "wallpaper-go"),
                        ("wallpaper.picker.parent", "wallpaper-parent"),
                    ] {
                        if ui.button(label(key)).clicked() {
                            events.push(UiEvent::Command(id, Origin::Ui));
                        }
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("wallpaper-files")
                    .max_height(ui.spacing().interact_size.y * 10.0)
                    .show(ui, |ui| {
                        for entry in &picker.entries {
                            let name = if entry.directory {
                                format!("{}/", entry.name)
                            } else {
                                entry.name.clone()
                            };
                            let clicked = toolkit::selectable(
                                ui,
                                picker.selected.as_deref() == Some(&entry.path),
                                name,
                            )
                            .clicked();
                            if clicked {
                                events.push(if entry.directory {
                                    UiEvent::PickerEnter(entry.path.clone())
                                } else {
                                    UiEvent::PickerSelect(entry.path.clone())
                                });
                            }
                        }
                    });
            });
            if picker.loading.is_some() {
                ui.label(label("wallpaper-browsing"));
            }
            if let Some(error) = &picker.error {
                ui.add(
                    egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color))
                        .wrap(),
                );
            }
            if picker.truncated {
                ui.add(egui::Label::new(label("wallpaper-truncated")).wrap());
            }
        });
    if shown.chosen.is_some() || shown.dismissed {
        dialog::Dialog::reset(ui.ctx(), id);
        events.push(UiEvent::Command(
            if shown.chosen == Some(0) {
                "wallpaper.picker.use"
            } else {
                "wallpaper.picker.cancel"
            },
            Origin::Ui,
        ));
    }
}
