// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window: the toolkit title bar with the menus, the document tabs, the
//! notice strip (warnings, set-aside edits, a detached document), the find
//! bar, the editor, the Problems and Output panels, and the status bar.
//! [`view`] draws the state and returns what the person did as
//! [`UiEvent`]s; [`apply`] runs them after the frame, so a snapshot needs no
//! Bus. The find bar and dialog fields are the one exception: their text is
//! edited in place, as any text field is.

use documents::actions::ActionId;
use documents::controller::Tab;
use editor::{Doc, Palette};
use editor_model::diag::Severity;
use editor_model::mirror::{DetachReason, Phase};
use editor_model::types::{Level, TabId};
use egui::{Align, Layout, RichText, Ui};
use toolkit::dialog::{self, Choice, Role};
use toolkit::tabs::DocTab;
use toolkit::titlebar::{self, Control};
use toolkit::{Icon, Registry, Strings, bars, icons};

use crate::app::{Answer, App, Dialog};
use crate::{label, label_with};

/// One interaction from this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    Command(&'static str),
    SelectTab(TabId),
    CloseTab(TabId),
    Editor(TabId, editor::Event),
    Answer(Answer),
    DismissNotice(usize),
    /// A find-bar button: find next or previous, replace, replace all.
    Find(ActionId),
    CloseFind,
    KeepMine(TabId),
    TakeTheirs(TabId),
    /// Re-insert set-aside text at the caret, and forget the conflict.
    Reinsert(TabId, u64, String),
    DismissConflict(TabId, u64),
    /// Go to a problem: line and column.
    Goto(TabId, usize, usize),
}

const ICON: f32 = 14.0;

/// Draw the window into `ui`.
pub fn view(
    ui: &mut Ui,
    app: &mut App,
    commands: &Registry<App>,
    strings: &Strings,
    palette: &Palette,
    stroke: f32,
    connected: bool,
) -> Vec<UiEvent> {
    let mut events = Vec::new();
    let mut controls = [Control::Link {
        command: "help.keys",
        icon: Icon::Keyboard,
    }];
    let fired = titlebar::show_with(
        ui,
        &label("title"),
        Some(Icon::FileText),
        stroke,
        commands,
        &*app,
        strings,
        &mut controls,
    );
    events.extend(fired.into_iter().map(UiEvent::Command));
    bars::status(ui, |ui| status(ui, app, stroke, connected));
    if app.ui.problems {
        egui::Panel::bottom("problems")
            .resizable(true)
            .default_size(140.0)
            .frame(bars::dock_frame(ui.ctx()))
            .show(ui, |ui| problems(ui, app, &mut events));
    }
    if app.ui.output {
        egui::Panel::bottom("output")
            .resizable(true)
            .default_size(120.0)
            .frame(bars::dock_frame(ui.ctx()))
            .show(ui, |ui| {
                ui.weak(label("output-empty"));
            });
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::new())
        .show(ui, |ui| {
            tabs(ui, app, &mut events);
            notices(ui, app, stroke, &mut events);
            find_bar(ui, app, &mut events);
            let shown = document(ui, app, palette, &mut events);
            app.editor_id = shown.map(|(id, _)| id);
            app.editor_held = shown.is_some_and(|(_, held)| held);
        });
    dialogs(ui.ctx(), app, commands, strings, &mut events);
    events
}

/// Run one frame's events against the app.
pub fn apply(app: &mut App, commands: &Registry<App>, events: Vec<UiEvent>) {
    for event in events {
        match event {
            UiEvent::Command(id) => {
                let _ = commands.execute(id, app);
            }
            UiEvent::SelectTab(tab) => {
                let fx = app.ctl.select_tab(tab);
                app.absorb(fx);
            }
            UiEvent::CloseTab(tab) => {
                let fx = app.ctl.select_tab(tab);
                app.absorb(fx);
                app.action(ActionId::FileClose);
            }
            UiEvent::Editor(tab, event) => app.editor_event(tab, event),
            UiEvent::Answer(answer) => app.answer(answer),
            UiEvent::DismissNotice(i) => app.dismiss_notice(i),
            UiEvent::Find(action) => app.find(action),
            UiEvent::CloseFind => app.close_find(),
            UiEvent::KeepMine(tab) => {
                let intent = editor_model::types::Intent::ui(tab);
                let fx = app.ctl.keep_mine(tab, intent);
                app.absorb(fx);
            }
            UiEvent::TakeTheirs(tab) => {
                let fx = app.ctl.take_service(tab);
                app.absorb(fx);
            }
            UiEvent::Reinsert(tab, rev, text) => app.reinsert(tab, rev, text),
            UiEvent::DismissConflict(tab, rev) => app.ctl.dismiss_conflict(tab, rev),
            UiEvent::Goto(tab, line, col) => {
                app.action_args(
                    Some(tab),
                    ActionId::SearchGotoLine,
                    serde_json::json!({ "line": line, "col": col }),
                );
            }
        }
    }
}

/// A tab's display name: its file name, the service's name, or untitled.
pub fn tab_name(tab: &Tab) -> String {
    let from_path = |p: &str| {
        std::path::Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    };
    tab.path
        .as_deref()
        .and_then(from_path)
        .or_else(|| {
            let meta = tab.mirror.as_ref()?.meta();
            meta.path
                .as_deref()
                .and_then(from_path)
                .or_else(|| meta.name.clone())
        })
        .unwrap_or_else(|| format!("untitled-{}", tab.id))
}

fn tabs(ui: &mut Ui, app: &App, events: &mut Vec<UiEvent>) {
    let list = app.ctl.tabs();
    if list.is_empty() {
        return;
    }
    let shown: Vec<DocTab> = list
        .iter()
        .map(|t| {
            let name = tab_name(t);
            let mirror = t.mirror.as_ref();
            let opening = mirror.is_none_or(|m| matches!(m.phase(), Phase::Bootstrapping { .. }));
            let mut meta = mirror.map_or_else(String::new, |m| m.meta().language.clone());
            if app.ctl.agent_since_focus(t.id) {
                meta.push_str(" ◆");
            }
            DocTab {
                title: name.clone(),
                name,
                meta,
                dirty: mirror.is_some_and(|m| m.meta().dirty),
                progress: opening.then_some(0.0),
            }
        })
        .collect();
    let selected = app
        .active()
        .and_then(|a| list.iter().position(|t| t.id == a))
        .unwrap_or(0);
    let picked = toolkit::tabs::documents(ui, egui::Id::new("ced.tabs"), &shown, selected, None);
    if let Some(i) = picked.selected
        && let Some(t) = list.get(i)
    {
        events.push(UiEvent::SelectTab(t.id));
    }
    if let Some(i) = picked.closed
        && let Some(t) = list.get(i)
    {
        events.push(UiEvent::CloseTab(t.id));
    }
}

/// Warnings and errors, set-aside edits, and a detached document.
fn notices(ui: &mut Ui, app: &App, stroke: f32, events: &mut Vec<UiEvent>) {
    let tab = app.active_tab();
    let mirror = tab.and_then(|t| t.mirror.as_ref());
    let has_notices = !app.notices.is_empty()
        || mirror.is_some_and(|m| !m.conflicts().is_empty() || m.detached_copy().is_some());
    if !has_notices {
        return;
    }
    bars::options(ui, |ui| {
        ui.vertical(|ui| {
            for (i, shown) in app.notices.iter().enumerate() {
                ui.horizontal(|ui| {
                    let colour = match shown.level {
                        Level::Error => ui.visuals().error_fg_color,
                        _ => ui.visuals().warn_fg_color,
                    };
                    ui.add(icons::image(Icon::CircleAlert, stroke, ICON, colour));
                    ui.label(&shown.text);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(label("dismiss")).clicked() {
                            events.push(UiEvent::DismissNotice(i));
                        }
                    });
                });
            }
            let (Some(tab), Some(m)) = (tab, mirror) else {
                return;
            };
            for conflict in m.conflicts() {
                ui.horizontal(|ui| {
                    ui.add(icons::image(
                        Icon::Info,
                        stroke,
                        ICON,
                        ui.visuals().warn_fg_color,
                    ));
                    let n = conflict.texts.len().to_string();
                    ui.label(label_with("conflict-notice", &[("n", &n)]));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(label("dismiss")).clicked() {
                            events.push(UiEvent::DismissConflict(tab.id, conflict.rev));
                        }
                        if ui.small_button(label("reinsert")).clicked() {
                            events.push(UiEvent::Reinsert(
                                tab.id,
                                conflict.rev,
                                conflict.texts.concat(),
                            ));
                        }
                    });
                });
            }
            if m.detached_copy().is_some() {
                ui.horizontal(|ui| {
                    ui.add(icons::image(
                        Icon::Unplug,
                        stroke,
                        ICON,
                        ui.visuals().warn_fg_color,
                    ));
                    ui.label(label("detached-notice"));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(label("take-theirs")).clicked() {
                            events.push(UiEvent::TakeTheirs(tab.id));
                        }
                        if ui.small_button(label("keep-mine")).clicked() {
                            events.push(UiEvent::KeepMine(tab.id));
                        }
                    });
                });
            }
        });
    });
}

fn find_bar(ui: &mut Ui, app: &mut App, events: &mut Vec<UiEvent>) {
    let focus = std::mem::take(&mut app.focus_find);
    // Esc belongs to a dialog while one is open.
    let modal = app.dialog.is_some();
    let Some(bar) = app.ui.find.as_mut() else {
        return;
    };
    bars::options(ui, |ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let field =
                    toolkit::field::search(ui, &mut bar.pattern, &label("find-pattern"), 220.0);
                if focus {
                    field.request_focus();
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    events.push(UiEvent::Find(ActionId::SearchFindNext));
                }
                if ui.button(label("find-prev")).clicked() {
                    events.push(UiEvent::Find(ActionId::SearchFindPrev));
                }
                if ui.button(label("find-next")).clicked() {
                    events.push(UiEvent::Find(ActionId::SearchFindNext));
                }
                ui.checkbox(&mut bar.regex, label("find-regex"));
                ui.checkbox(&mut bar.case, label("find-case"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button(label("close")).clicked()
                        || (!modal && ui.input(|i| i.key_pressed(egui::Key::Escape)))
                    {
                        events.push(UiEvent::CloseFind);
                    }
                });
            });
            if bar.replace {
                ui.horizontal(|ui| {
                    toolkit::field::text(
                        ui,
                        &mut bar.replacement,
                        &label("find-replacement"),
                        220.0,
                    );
                    if ui.button(label("find-replace")).clicked() {
                        events.push(UiEvent::Find(ActionId::SearchReplace));
                    }
                    if ui.button(label("find-replace-all")).clicked() {
                        events.push(UiEvent::Find(ActionId::SearchReplaceAll));
                    }
                });
            }
        });
    });
}

/// The active document in the editor, or why there is none. Returns the
/// editor's widget id and whether it holds input for its next frame, when
/// one is shown.
fn document(
    ui: &mut Ui,
    app: &App,
    palette: &Palette,
    events: &mut Vec<UiEvent>,
) -> Option<(egui::Id, bool)> {
    let Some(tab) = app.active_tab() else {
        centred(ui, &label("no-documents"));
        return None;
    };
    let name = tab_name(tab);
    let Some(m) = tab.mirror.as_ref() else {
        centred(ui, &label_with("opening", &[("name", &name)]));
        return None;
    };
    if matches!(m.phase(), Phase::Bootstrapping { .. }) {
        centred(ui, &label_with("opening", &[("name", &name)]));
        return None;
    }
    let view = app.view_for(tab);
    let doc = Doc {
        text: m.text(),
        identity: tab.id,
        revision: m.view_gen(),
        model: &tab.editor,
        highlight: Some(&tab.highlight),
        diagnostics: tab.diagnostics.items(),
    };
    let editable = app.dialog.is_none();
    let out = ui
        .add_enabled_ui(editable, |ui| {
            editor::show(ui, ("ced.editor", tab.id), &doc, palette, &view)
        })
        .inner;
    let shown = (out.response.id, out.held);
    events.extend(out.events.into_iter().map(|e| UiEvent::Editor(tab.id, e)));
    if let Phase::Detached {
        reason: DetachReason::OpenFailed { msg },
    } = m.phase()
    {
        ui.weak(label_with(
            "open-failed",
            &[("name", &name), ("error", msg)],
        ));
    }
    Some(shown)
}

fn centred(ui: &mut Ui, text: &str) {
    ui.centered_and_justified(|ui| {
        ui.weak(text);
    });
}

fn problems(ui: &mut Ui, app: &App, events: &mut Vec<UiEvent>) {
    let Some(tab) = app.active_tab() else {
        ui.weak(label("problems-empty"));
        return;
    };
    let items = tab.diagnostics.items();
    if items.is_empty() {
        ui.weak(label("problems-empty"));
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for d in items {
            let colour = match d.severity {
                Severity::Error => ui.visuals().error_fg_color,
                Severity::Warning => ui.visuals().warn_fg_color,
                Severity::Note => ui.visuals().weak_text_color(),
            };
            let col = tab
                .mirror
                .as_ref()
                .map_or(1, |m| column_of(m.text(), d.range.start));
            let text = format!("{}:{}  {}  {}", d.line, col, d.code, d.message);
            if ui
                .add(
                    egui::Label::new(RichText::new(text).color(colour)).sense(egui::Sense::click()),
                )
                .clicked()
            {
                events.push(UiEvent::Goto(tab.id, d.line, col));
            }
        }
    });
}

/// The 1-based column of `offset`, counted in characters as the edit
/// service counts them.
pub fn column_of(text: &edit::text::Text, offset: usize) -> usize {
    text.point(offset).col
}

/// Characters selected in a tab, counted once per selection and
/// text revision (a large selection is not walked every frame).
fn selected_chars(ctx: &egui::Context, tab: &Tab) -> Option<usize> {
    let m = tab.mirror.as_ref()?;
    let sel = tab.editor.sel;
    if sel.anchor == sel.head {
        return None;
    }
    let text = m.text();
    let (a, b) = (
        sel.anchor.min(sel.head).min(text.len()),
        sel.anchor.max(sel.head).min(text.len()),
    );
    let key = (tab.id, m.view_gen(), a, b);
    let id = egui::Id::new("ced.selected_chars");
    if let Some((k, n)) = ctx.data(|d| d.get_temp::<((TabId, u64, usize, usize), usize)>(id))
        && k == key
    {
        return Some(n);
    }
    // Columns are counted the same way: characters from the line start.
    let n = count_chars(text, a, b);
    ctx.data_mut(|d| d.insert_temp(id, (key, n)));
    Some(n)
}

/// Unicode scalars in `a..b` (CR and LF are one each), read through the
/// text's chunks rather than a copy: every byte that does not continue a
/// UTF-8 sequence starts a scalar.
fn count_chars(text: &edit::text::Text, a: usize, b: usize) -> usize {
    let mut n = 0;
    let mut at = a;
    while at < b {
        let chunk = text.chunk_at(at);
        if chunk.is_empty() {
            break;
        }
        let take = chunk.len().min(b - at);
        n += chunk[..take].iter().filter(|&&c| c & 0xC0 != 0x80).count();
        at += take;
    }
    n
}

fn status(ui: &mut Ui, app: &App, stroke: f32, connected: bool) {
    let weak = ui.visuals().weak_text_color();
    let (icon, colour) = if connected {
        (Icon::Plug, weak)
    } else {
        (Icon::Unplug, ui.visuals().error_fg_color)
    };
    ui.add(icons::image(icon, stroke, ICON, colour));
    if !connected {
        ui.small(RichText::new(label("not-connected")).color(ui.visuals().error_fg_color));
    }
    if let Some(text) = app.status() {
        ui.small(text);
    }
    let Some(tab) = app.active_tab() else {
        return;
    };
    let Some(m) = tab.mirror.as_ref() else {
        return;
    };
    let text = m.text();
    let head = tab.editor.sel.head.min(text.len());
    let line = editor_model::model::line_of(text, head);
    let col = column_of(text, head);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let meta = m.meta();
        ui.small(if tab.editor.overwrite {
            label("status-overwrite")
        } else {
            label("status-insert")
        });
        ui.small(&meta.language);
        let eol = match meta.eol {
            edit::wire::Eol::Crlf => "CRLF",
            _ => "LF",
        };
        ui.small(if meta.bom {
            format!("UTF-8 BOM · {eol}")
        } else {
            format!("UTF-8 · {eol}")
        });
        ui.small(label_with("status-rev", &[("rev", &m.rev().to_string())]));
        ui.small(if meta.dirty {
            label("status-modified")
        } else if meta.path.is_some() {
            label("status-saved")
        } else {
            label("status-unsaved")
        });
        if !matches!(m.phase(), Phase::Live) {
            ui.small(RichText::new(label("reconnecting")).color(ui.visuals().warn_fg_color));
        }
        if m.pending() > 0 {
            ui.small(label_with(
                "status-pending",
                &[("n", &m.pending().to_string())],
            ));
        }
        if let Some(chars) = selected_chars(ui.ctx(), tab) {
            ui.small(label_with(
                "status-selection",
                &[("chars", &chars.to_string())],
            ));
        }
        ui.small(label_with(
            "status-line-col",
            &[("line", &line.to_string()), ("col", &col.to_string())],
        ));
    });
}

fn dialogs(
    ctx: &egui::Context,
    app: &mut App,
    commands: &Registry<App>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let Some(dialog) = app.dialog.clone() else {
        return;
    };
    let name_of = |tab: TabId, app: &App| {
        app.ctl
            .tabs()
            .iter()
            .find(|t| t.id == tab)
            .map(tab_name)
            .unwrap_or_default()
    };
    let ok_cancel = |ok: &str| {
        vec![
            Choice::new(label(ok), Role::Default),
            Choice::new(label("cancel"), Role::Cancel),
        ]
    };
    let (title, buttons, text) = match &dialog {
        Dialog::Open { .. } => (
            label("open-title"),
            ok_cancel("open"),
            Some(label("open-hint")),
        ),
        Dialog::SaveAs { .. } => (
            label("save-as-title"),
            ok_cancel("save"),
            Some(label("save-as-hint")),
        ),
        Dialog::Goto { .. } => (
            label("goto-title"),
            ok_cancel("go"),
            Some(label("goto-hint")),
        ),
        Dialog::CloseDirty { .. } => (
            label("close-dirty-title"),
            vec![
                Choice::new(label("save"), Role::Default),
                Choice::new(label("discard"), Role::Other),
                Choice::new(label("cancel"), Role::Cancel),
            ],
            None,
        ),
        Dialog::DiskModified { .. } => {
            (label("disk-modified-title"), ok_cancel("save-anyway"), None)
        }
        Dialog::Recovered { .. } => (
            label("recovered-title"),
            vec![Choice::new(label("close"), Role::Cancel)],
            None,
        ),
        Dialog::Keys { .. } => (
            label("keys-title"),
            vec![Choice::new(label("close"), Role::Default)],
            None,
        ),
        Dialog::About { .. } => (
            label("about-title"),
            vec![Choice::new(label("close"), Role::Default)],
            None,
        ),
    };
    let id = egui::Id::new(("ced.dialog", std::mem::discriminant(&dialog)));
    let response = dialog::Dialog::new(id, title)
        .buttons(buttons)
        .show(ctx, |ui| match &dialog {
            Dialog::Open { .. } | Dialog::SaveAs { .. } | Dialog::Goto { .. } => {
                if let (Some(field), Some(hint)) = (app.dialog_text(), text.as_deref()) {
                    let response = toolkit::field::text(ui, field, hint, 420.0);
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        events.push(UiEvent::Answer(Answer::Accept));
                    } else if !response.has_focus() && !response.lost_focus() {
                        response.request_focus();
                    }
                }
            }
            Dialog::CloseDirty { tab, .. } => {
                ui.label(label_with(
                    "close-dirty-body",
                    &[("name", &name_of(*tab, app))],
                ));
            }
            Dialog::DiskModified { tab, .. } => {
                ui.label(label_with(
                    "disk-modified-body",
                    &[("name", &name_of(*tab, app))],
                ));
            }
            Dialog::Recovered { rows } => {
                ui.label(label("recovered-body"));
                for row in rows {
                    ui.horizontal(|ui| {
                        ui.label(row.path.clone().unwrap_or_else(|| row.name.clone()));
                        if ui.button(label("open-recovered")).clicked() {
                            events.push(UiEvent::Answer(Answer::Recovered {
                                buffer: row.buffer.clone(),
                                open: true,
                            }));
                        }
                        if ui.button(label("discard-recovered")).clicked() {
                            events.push(UiEvent::Answer(Answer::Recovered {
                                buffer: row.buffer.clone(),
                                open: false,
                            }));
                        }
                    });
                }
            }
            Dialog::Keys { .. } => {
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        egui::Grid::new("ced.keys").striped(true).show(ui, |ui| {
                            for command in commands.iter() {
                                if let Some(shortcut) = command.shortcut {
                                    ui.label(toolkit::command::label_text(strings, command.label));
                                    ui.monospace(ui.ctx().format_shortcut(&shortcut));
                                    ui.end_row();
                                }
                            }
                        });
                    });
            }
            Dialog::About { .. } => {
                ui.label(label("about-body"));
                ui.weak(label_with(
                    "about-version",
                    &[("version", env!("CARGO_PKG_VERSION"))],
                ));
            }
        });
    let answer = match response.chosen {
        Some(0) => Some(match dialog {
            Dialog::Recovered { .. } => Answer::Cancel,
            _ => Answer::Accept,
        }),
        Some(1) if matches!(dialog, Dialog::CloseDirty { .. }) => Some(Answer::Other),
        Some(_) => Some(Answer::Cancel),
        None if response.dismissed => Some(Answer::Cancel),
        None => None,
    };
    if let Some(answer) = answer {
        events.push(UiEvent::Answer(answer));
    }
}

#[cfg(test)]
mod tests {
    use super::count_chars;
    use edit::text::Text;

    #[test]
    fn selections_count_scalars_across_line_endings() {
        let text = Text::from_text("a\r\nb\u{e9}").unwrap();
        // Between the CR and the LF: a valid endpoint, one character.
        assert_eq!(count_chars(&text, 2, 3), 1);
        assert_eq!(count_chars(&text, 1, 3), 2);
        assert_eq!(count_chars(&text, 0, text.len()), 5);
        assert_eq!(count_chars(&text, 3, text.len()), 2);
    }
}
