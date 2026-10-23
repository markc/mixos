// SPDX-License-Identifier: MIT OR Apache-2.0
//! A macro panel with plain display data in and requested actions out.
//! Filesystem and process work belongs to the caller, never the widget.
use crate::button::PushButton;
use crate::dialog::{Choice, Dialog, Role};
use crate::strings::own;

#[derive(Clone, Debug)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub tier: String,
    pub path: String,
    pub shadows: String,
}

#[derive(Default)]
pub struct Data {
    pub rows: Vec<Row>,
    pub diagnostics: Vec<String>,
    pub busy: bool,
    pub running: bool,
    pub status: String,
    pub output: String,
    pub dropped: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Run(usize),
    Edit(usize),
    New(bool),
    OpenFolder(bool),
    Reload,
    Stop,
    Create,
}

#[derive(Default)]
pub struct Draft {
    pub target_app: Option<bool>,
    pub id: String,
    pub title: String,
}

/// Draw with the toolkit's dialog, button, field and section styles.
pub fn show(ctx: &egui::Context, open: &mut bool, data: &Data, draft: &mut Draft) -> Vec<Action> {
    let visible = egui::Id::new("macros-panel-visible");
    if !*open {
        ctx.data_mut(|d| d.remove::<bool>(visible));
        return Vec::new();
    }
    let was_open = ctx.data(|d| d.get_temp::<bool>(visible)).unwrap_or(false);
    ctx.data_mut(|d| d.insert_temp(visible, true));
    let mut actions = Vec::new();
    let shown = Dialog::new("macros-panel", own("toolkit-macros"))
        .buttons(vec![Choice::new(own("macros-close"), Role::Cancel)])
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(!data.busy, |ui| {
                    for (key, action) in [
                        ("macros-new-global", Action::New(false)),
                        ("macros-new-app", Action::New(true)),
                        ("macros-open-global", Action::OpenFolder(false)),
                        ("macros-open-app", Action::OpenFolder(true)),
                        ("macros-reload", Action::Reload),
                    ] {
                        if ui.add(PushButton::secondary(own(key))).clicked() {
                            actions.push(action);
                        }
                    }
                });
            });
            if draft.target_app.is_some() {
                crate::panel::section_label(ui, &own("macros-create"));
                let width = ui.available_width();
                crate::field::text(ui, &mut draft.id, &own("macros-id"), width);
                crate::field::text(ui, &mut draft.title, &own("macros-title"), width);
                if ui
                    .add_enabled(
                        !data.busy && !draft.id.is_empty(),
                        PushButton::primary(own("macros-create")),
                    )
                    .clicked()
                {
                    actions.push(Action::Create);
                }
            }
            let max_height = ctx.content_rect().height()
                * crate::chrome::Chrome::of(ctx).style.dialog_resize_max_share as f32;
            egui::ScrollArea::vertical()
                .id_salt("macro-list")
                .max_height(max_height / 3.0)
                .show(ui, |ui| {
                    if data.rows.is_empty() {
                        ui.add(crate::text::dim(own("macros-empty")));
                    }
                    for (index, row) in data.rows.iter().enumerate() {
                        ui.push_id(&row.id, |ui| {
                            crate::panel::section_label(ui, &row.tier);
                            ui.label(&row.title).on_hover_text(&row.path);
                            ui.add(crate::text::dim(&row.id));
                            if !row.shadows.is_empty() {
                                ui.add(crate::text::dim(own("macros-shadows")))
                                    .on_hover_text(&row.shadows);
                            }
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        !data.running,
                                        PushButton::primary(own("macros-run")),
                                    )
                                    .clicked()
                                {
                                    actions.push(Action::Run(index));
                                }
                                if ui
                                    .add_enabled(
                                        !data.busy,
                                        PushButton::secondary(own("macros-edit")),
                                    )
                                    .clicked()
                                {
                                    actions.push(Action::Edit(index));
                                }
                            });
                        });
                    }
                    for diagnostic in &data.diagnostics {
                        ui.label(diagnostic);
                    }
                });
            if !data.status.is_empty() {
                ui.label(&data.status);
            }
            if data.running && ui.add(PushButton::secondary(own("macros-stop"))).clicked() {
                actions.push(Action::Stop);
            }
            if data.dropped {
                ui.add(crate::text::dim(own("macros-output-dropped")));
            }
            if !data.output.is_empty() {
                egui::ScrollArea::both()
                    .id_salt("macro-output")
                    .max_height(max_height / 3.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&data.output).monospace())
                                .selectable(true),
                        );
                    });
            }
        });
    // The modal backdrop can consume a title-bar click before the app's
    // toggle sees it. Dismiss on that outside click as well, but ignore the
    // opening click when an app executes its toggle before drawing us.
    let outside = was_open && ctx.input(|i| {
        i.pointer.primary_clicked()
            && i.pointer.interact_pos().is_some_and(|pos| !shown.rect.contains(pos))
    });
    if shown.chosen.is_some() || shown.dismissed || outside {
        *open = false;
        ctx.data_mut(|d| d.remove::<bool>(visible));
        Dialog::reset(ctx, "macros-panel");
    }
    actions
}
