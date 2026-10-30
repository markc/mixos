// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The window: the toolkit title bar with the menus, the document tabs, the
//! notice strip (set-aside edits, a detached document), the find
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
use editor_model::types::TabId;
use egui::{Align, Layout, RichText, Ui};
use toolkit::action::Origin;
use toolkit::dialog::{self, Choice, Role};
use toolkit::tabs::DocTab;
use toolkit::titlebar;
use toolkit::{Icon, Registry, Strings, bars, icons};

use crate::app::{Answer, App, Dialog, PreviewMode};
use crate::{label, label_with};

/// One interaction from this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    Command(&'static str, Origin),
    CloseMacros,
    MailHeaders(TabId, documents::mail::Headers),
    MailCommit(TabId, Option<&'static str>),
    MailRow(TabId, &'static str),
    MailAttach(TabId),
    SelectTab(TabId),
    CloseTab(TabId),
    Editor(TabId, editor::Event),
    Answer(Answer),
    /// A find-bar button: find next or previous, replace, replace all.
    Find(ActionId, Origin),
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
    connected: bool,
) -> Vec<UiEvent> {
    let mut events = Vec::new();
    // The tab strip continues the chrome, so no divider above it.
    titlebar::seamless(ui.ctx(), true);
    // The icon bar: the standard tail only.
    let fired = titlebar::show_bar(
        ui,
        &label("title"),
        Some(Icon::FileText),
        commands,
        &*app,
        strings,
        &[],
    );
    events.extend(
        fired
            .into_iter()
            .map(|(id, origin)| UiEvent::Command(id, origin)),
    );
    bars::status(ui, |ui| status(ui, app, connected));
    if app.ui.problems {
        egui::Panel::bottom("problems")
            .resizable(true)
            .default_size(140.0)
            .size_range(0.0..=ui.available_height() * 0.3)
            .frame(bars::dock_frame(ui.ctx()))
            .show(ui, |ui| problems(ui, app, &mut events));
    }
    if app.ui.output {
        egui::Panel::bottom("output")
            .resizable(true)
            .default_size(120.0)
            .size_range(0.0..=ui.available_height() * 0.3)
            .frame(bars::dock_frame(ui.ctx()))
            .show(ui, |ui| {
                ui.weak(label("output-empty"));
            });
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(palette.background))
        .show(ui, |ui| {
            // The tabs sit directly on the document, with no gap between.
            ui.spacing_mut().item_spacing.y = 0.0;
            tabs(ui, app, palette, &mut events);
            notices(ui, app, &mut events);
            find_bar(ui, app, &mut events);
            crate::mail_view::strip(ui, app, &mut events);
            let shown = document(ui, app, palette, &mut events);
            app.editor_id = shown.map(|(id, _)| id);
            app.editor_held = shown.is_some_and(|(_, held)| held);
        });
    dialogs(ui.ctx(), app, commands, strings, &mut events);
    crate::mail_view::chooser(ui.ctx(), app, &mut events);
    if toolkit::macros::show_app(ui.ctx(), "ced", app.ui.macros_open) {
        events.push(UiEvent::CloseMacros);
    }
    events
}

/// Run one frame's events against the app.
pub fn apply(app: &mut App, commands: &Registry<App>, events: Vec<UiEvent>) {
    // A header preview belongs to the frame that displayed it, regardless of
    // whether a menu/shortcut event was collected before or after the strip.
    let (headers, events): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|e| matches!(e, UiEvent::MailHeaders(..)));
    let (closed, events): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|e| matches!(e, UiEvent::CloseMacros));
    for event in headers.into_iter().chain(events).chain(closed) {
        apply_one(app, commands, event);
        record_noted(app, commands);
    }
}

/// Record the runs the app performed outside the registry, in order.
pub fn record_noted(app: &mut App, commands: &Registry<App>) {
    for (id, origin) in std::mem::take(&mut app.noted) {
        commands.note(&id, origin);
    }
}

fn apply_one(app: &mut App, commands: &Registry<App>, event: UiEvent) {
    {
        match event {
            UiEvent::MailHeaders(tab, headers) => {
                if app
                    .ctl
                    .tabs()
                    .iter()
                    .find(|t| t.id == tab)
                    .and_then(|t| t.mail.as_ref())
                    .is_some_and(|mail| !mail.sending && !mail.saving)
                {
                    // Validate OS paths before any JSON hop can lose bytes.
                    let _ = app.ctl.preview_headers(tab, headers);
                }
            }
            UiEvent::MailCommit(tab, field) => commit_mail_headers(app, tab, field),
            UiEvent::MailRow(tab, field) => app.ctl.show_mail_row(tab, field),
            UiEvent::MailAttach(tab) => {
                if !app
                    .ctl
                    .tabs()
                    .iter()
                    .find(|t| t.id == tab)
                    .and_then(|t| t.mail.as_ref())
                    .is_some_and(|mail| !mail.saving && !mail.sending)
                {
                    return;
                }
                let directory = app
                    .ctl
                    .tabs()
                    .iter()
                    .find(|t| t.id == tab)
                    .and_then(|t| t.path.as_deref())
                    .and_then(|p| std::path::Path::new(p).parent())
                    .unwrap_or(std::path::Path::new("/"))
                    .to_path_buf();
                app.mail_chooser = Some((
                    tab,
                    toolkit::file_chooser::FileChooser::new(
                        directory,
                        label("mail-attach"),
                        label("mail-attach"),
                    ),
                ));
            }
            UiEvent::CloseMacros => app.ui.macros_open = false,
            UiEvent::Command(id, origin) => {
                if let Some(tab) = app.active() {
                    commit_mail_headers(app, tab, None);
                }
                let _ = commands.execute(id, app, origin);
            }
            UiEvent::SelectTab(tab) => {
                let fx = app.ctl.select_tab(tab);
                app.absorb(fx);
            }
            UiEvent::CloseTab(tab) => {
                let fx = app.ctl.select_tab(tab);
                app.absorb(fx);
                let _ = commands.execute("file.close", app, Origin::Ui);
            }
            UiEvent::Editor(tab, event) => app.editor_event(tab, event),
            UiEvent::Answer(answer) => app.answer(answer),
            UiEvent::Find(action, origin) => app.find(action, origin),
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
                app.noted.push((ActionId::SearchGotoLine.id(), Origin::Ui));
                app.action_args(
                    Some(tab),
                    ActionId::SearchGotoLine,
                    serde_json::json!({ "line": line, "col": col }),
                );
            }
        }
    }
}

fn commit_mail_headers(app: &mut App, tab: TabId, field: Option<&str>) {
    if !app.ctl.tabs().iter().any(|t| {
        t.id == tab
            && t.mail
                .as_ref()
                .is_some_and(|mail| !mail.saving && !mail.sending)
    }) {
        return;
    }
    let committed = app.ctl.commit_header_field(tab, field);
    if committed {
        let headers = &app
            .ctl
            .tabs()
            .iter()
            .find(|t| t.id == tab)
            .unwrap()
            .mail
            .as_ref()
            .unwrap()
            .headers;
        let headers = serde_json::to_value(headers).expect("validated header paths");
        app.action_args(
            Some(tab),
            ActionId::MailHeaders,
            serde_json::json!({"headers":headers}),
        );
        app.noted.push((ActionId::MailHeaders.id(), Origin::Ui));
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

fn tabs(ui: &mut Ui, app: &App, palette: &Palette, events: &mut Vec<UiEvent>) {
    let list = app.ctl.tabs();
    if list.is_empty() {
        return;
    }
    let shown: Vec<DocTab> = list
        .iter()
        .map(|t| {
            let name = tab_name(t);
            let mirror = t.mirror.as_ref();
            let opening = app.ctl.open_error(t.id).is_none()
                && mirror.is_none_or(|m| matches!(m.phase(), Phase::Bootstrapping { .. }));
            let mut meta = mirror.map_or_else(String::new, |_| t.language().to_owned());
            if app.ctl.agent_since_focus(t.id) {
                meta.push_str(" ◆");
            }
            DocTab {
                title: name.clone(),
                name,
                meta,
                dirty: t.dirty(),
                progress: opening.then_some(0.0),
                marker: None,
            }
        })
        .collect();
    let selected = app
        .active()
        .and_then(|a| list.iter().position(|t| t.id == a))
        .unwrap_or(0);
    let picked = toolkit::tabs::documents_on(
        ui,
        egui::Id::unique("ced.tabs"),
        &shown,
        selected,
        None,
        palette.background,
    );
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
fn notices(ui: &mut Ui, app: &App, events: &mut Vec<UiEvent>) {
    let tab = app.active_tab();
    let mirror = tab.and_then(|t| t.mirror.as_ref());
    let has_notices =
        mirror.is_some_and(|m| !m.conflicts().is_empty() || m.detached_copy().is_some());
    if !has_notices {
        return;
    }
    bars::options(ui, |ui| {
        ui.vertical(|ui| {
            let (Some(tab), Some(m)) = (tab, mirror) else {
                return;
            };
            for conflict in m.conflicts() {
                ui.horizontal(|ui| {
                    ui.add(icons::image_of(
                        ui.ctx(),
                        Icon::Info,
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
                    ui.add(icons::image_of(
                        ui.ctx(),
                        Icon::Unplug,
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
                    events.push(UiEvent::Find(ActionId::SearchFindNext, Origin::Shortcut));
                }
                if ui.button(label("find-prev")).clicked() {
                    events.push(UiEvent::Find(ActionId::SearchFindPrev, Origin::Ui));
                }
                if ui.button(label("find-next")).clicked() {
                    events.push(UiEvent::Find(ActionId::SearchFindNext, Origin::Ui));
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
                        events.push(UiEvent::Find(ActionId::SearchReplace, Origin::Ui));
                    }
                    if ui.button(label("find-replace-all")).clicked() {
                        events.push(UiEvent::Find(ActionId::SearchReplaceAll, Origin::Ui));
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
    app: &mut App,
    palette: &Palette,
    events: &mut Vec<UiEvent>,
) -> Option<(egui::Id, bool)> {
    let mode = app
        .active()
        .map(|tab| app.preview_mode(tab))
        .unwrap_or_default();
    let preview = if mode != PreviewMode::Source {
        app.active().and_then(|tab| app.preview_document(tab))
    } else {
        None
    };
    let Some(tab) = app.active_tab() else {
        centred(ui, &label("no-documents"));
        return None;
    };
    let name = tab_name(tab);
    let Some(m) = tab.mirror.as_ref() else {
        let message = app
            .ctl
            .open_error(tab.id)
            .map(|message| {
                if message == documents::controller::EDIT_UNAVAILABLE {
                    label("edit-unavailable")
                } else {
                    message.to_owned()
                }
            })
            .unwrap_or_else(|| {
                if app.ctl.open_slow(tab.id) {
                    label("opening-slow")
                } else {
                    label_with("opening", &[("name", &name)])
                }
            });
        centred(ui, &message);
        return None;
    };
    if matches!(m.phase(), Phase::Bootstrapping { .. }) {
        centred(ui, &label_with("opening", &[("name", &name)]));
        return None;
    }
    let mut view = app.view_for(tab);
    view.label = Some(name.clone());
    // Preserve the editor's saved size and zoom; Larger text multiplies
    // the displayed source and preview, using the desktop value only.
    let font_px = f32::from(app.ui.font_px) * toolkit::ui_scale::of(ui.ctx()).text_scale as f32;
    view.font_size = Some(font_px);
    let doc = Doc {
        text: m.text(),
        identity: tab.id,
        revision: m.view_gen(),
        model: &tab.editor,
        highlight: Some(&tab.highlight),
        diagnostics: tab.diagnostics.items(),
    };
    let editable = body_editable(app, tab);
    // The editor takes its own saved padding and light mode; the rest of the
    // window keeps the global look.
    let draw_editor = |ui: &mut Ui| {
        ui.add_enabled_ui(editable, |ui| {
            let mut out = None;
            toolkit::content::surface_filled(ui, &app.editor_style, palette.background, |ui| {
                if mode == PreviewMode::Split
                    && let Some(baseline) = preview.as_ref().and_then(|document| {
                        toolkit::markdown::first_heading_baseline(ui, document)
                    })
                {
                    let font = egui::FontId::monospace(font_px);
                    let galley = ui.painter().layout_no_wrap("M".into(), font, palette.text);
                    if let Some(row) = galley.rows.first()
                        && let Some(glyph) = row.glyphs.first()
                    {
                        // Both panes keep the same card and baseline inset; the
                        // source row reserves the opening heading's extra ascent.
                        ui.painter().rect_filled(
                            ui.available_rect_before_wrap(),
                            0.0,
                            palette.background,
                        );
                        ui.add_space((baseline - row.pos.y - glyph.pos.y).max(0.0));
                    }
                }
                out = Some(editor::show(
                    ui,
                    ("ced.editor", tab.id),
                    &doc,
                    palette,
                    &view,
                ));
            });
            out.expect("the surface draws its body")
        })
        .inner
    };
    let draw_preview = |ui: &mut Ui| {
        toolkit::content::surface_filled(ui, &app.editor_style, palette.background, |ui| {
            for style in [egui::TextStyle::Body, egui::TextStyle::Monospace] {
                if let Some(font) = ui.style_mut().text_styles.get_mut(&style) {
                    font.size = font_px;
                }
            }
            ui.set_min_size(ui.available_size());
            let font = egui::FontId::monospace(font_px);
            let metrics = ui.fonts_mut(|fonts| editor::layout::Metrics {
                cell_w: fonts.glyph_width(&font, '0').max(1.0),
                row_h: (font.size * view.line_height).round().max(1.0),
            });
            let geometry = editor::layout::Geometry::new(
                ui.available_rect_before_wrap(),
                metrics,
                m.text().line_count(),
                view.line_numbers,
            );
            // Use the editor's field fill and text origin, including its gutter.
            ui.painter()
                .rect_filled(geometry.bounds, 0.0, palette.background);
            let top =
                ui.fonts_mut(|fonts| ((metrics.row_h - fonts.row_height(&font)) / 2.0).round());
            let mut text_rect = geometry.text_rect();
            text_rect.min.y += top;
            ui.scope_builder(egui::UiBuilder::new().max_rect(text_rect), |ui| {
                egui::ScrollArea::both()
                    .id_salt(("ced.preview", tab.id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(document) = &preview {
                            toolkit::markdown::show(ui, document);
                        }
                    });
            });
        });
    };
    let out = match mode {
        PreviewMode::Source => Some(draw_editor(ui)),
        PreviewMode::Preview => {
            draw_preview(ui);
            None
        }
        PreviewMode::Split => {
            let area = ui.available_rect_before_wrap();
            let middle = area.center().x;
            let left = egui::Rect::from_min_max(area.min, egui::pos2(middle, area.max.y));
            let right = egui::Rect::from_min_max(egui::pos2(middle, area.min.y), area.max);
            let out = ui
                .scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
                    ui.set_clip_rect(left.intersect(ui.clip_rect()));
                    draw_editor(ui)
                })
                .inner;
            ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
                ui.set_clip_rect(right.intersect(ui.clip_rect()));
                draw_preview(ui);
            });
            ui.painter().vline(
                middle,
                area.y_range(),
                egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            );
            ui.advance_cursor_after_rect(area);
            Some(out)
        }
    };
    let shown = out.as_ref().map(|out| (out.response.id, out.held));
    if let Some(out) = out {
        events.extend(out.events.into_iter().map(|e| UiEvent::Editor(tab.id, e)));
    }
    if let Phase::Detached {
        reason: DetachReason::OpenFailed { msg },
    } = m.phase()
    {
        ui.weak(label_with(
            "open-failed",
            &[("name", &name), ("error", msg)],
        ));
    }
    shown
}

fn body_editable(app: &App, tab: &Tab) -> bool {
    app.dialog.is_none()
        && tab.mail.as_ref().is_none_or(|mail| {
            (!mail.sending && !mail.saving) || app.ctl.mail_body_editable(tab.id)
        })
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
    let id = egui::Id::unique("ced.selected_chars");
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

fn status(ui: &mut Ui, app: &App, connected: bool) {
    let weak = ui.visuals().weak_text_color();
    let (icon, colour) = if connected {
        (Icon::Plug, weak)
    } else {
        (Icon::Unplug, ui.visuals().error_fg_color)
    };
    ui.add(icons::image_of(ui.ctx(), icon, ICON, colour));
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
        ui.small(tab.language());
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
        ui.small(if tab.dirty() {
            label("status-modified")
        } else if tab.path.is_some() {
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
        Dialog::Keys => (
            label("keys-title"),
            vec![Choice::new(label("close"), Role::Default)],
            None,
        ),
        Dialog::About => (
            label("about-title"),
            vec![Choice::new(label("close"), Role::Default)],
            None,
        ),
    };
    let id = egui::Id::unique(("ced.dialog", std::mem::discriminant(&dialog)));
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
            Dialog::Keys => {
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        egui::Grid::new("ced.keys").striped(true).show(ui, |ui| {
                            for command in commands.iter() {
                                if let Some(shortcut) = command.shortcut {
                                    ui.label(toolkit::action::label_text(strings, command.label));
                                    ui.monospace(ui.ctx().format_shortcut(&shortcut));
                                    ui.end_row();
                                }
                            }
                        });
                    });
            }
            Dialog::About => {
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

    fn mail_app() -> (crate::app::App, u64) {
        use documents::controller::{Controller, Effect};
        use editor_model::types::{Incoming, Intent};
        let config = documents::config::Config::default();
        let mut ctl = Controller::new(config.clone(), 1, true);
        let mut service = editor_model::fake::FakeEditd::new("0000e1e1");
        let path = "/home/user/letter.md";
        service.create(
            Some(path),
            "---\nmail-subject: Original\nmail-to: bob@example.org\n---\nBody",
        );
        let mut queue: std::collections::VecDeque<_> =
            ctl.open_paths(&[path.into()], Intent::ui(0)).into();
        while let Some(effect) = queue.pop_front() {
            if let Effect::Send { req, out } = effect {
                let args = serde_json::from_str(&out.body).unwrap();
                let answer = service.handle("local:ced", &out.command, &args);
                queue.extend(ctl.on_incoming(Incoming::Reply {
                    req,
                    rc: answer.rc,
                    body: answer.body.to_string(),
                }));
                for event in service.take_events() {
                    queue.extend(ctl.on_incoming(Incoming::Topic {
                        topic: "edit.changed".into(),
                        body: serde_json::to_string(&event).unwrap(),
                    }));
                }
            }
        }
        let tab = ctl.active().unwrap();
        (crate::app::App::new(ctl, config), tab)
    }

    #[test]
    fn body_typing_is_allowed_while_a_save_is_parked_but_not_during_the_write() {
        use documents::actions::ActionId;
        let (mut app, tab) = mail_app();
        let fx = app.ctl.on_editor(
            tab,
            documents::editor::EditorMsg::Command(editor_model::model::EditCommand::Insert(
                "First ".into(),
            )),
        );
        app.absorb(fx);
        app.action(ActionId::FileSave);
        assert!(app.ctl.mail_save_parked(tab));
        assert!(super::body_editable(&app, app.active_tab().unwrap()));
        let (mut app, tab) = mail_app();
        let fx = app.ctl.on_editor(
            tab,
            documents::editor::EditorMsg::Command(editor_model::model::EditCommand::Insert(
                "First ".into(),
            )),
        );
        app.absorb(fx);
        app.action(ActionId::FileSend);
        assert!(!super::body_editable(&app, app.active_tab().unwrap()));
        let (mut app, _) = mail_app();
        app.action(ActionId::FileSend);
        assert!(!super::body_editable(&app, app.active_tab().unwrap()));
    }

    #[test]
    fn focus_loss_only_publishes_a_committed_header_edit() {
        let (mut app, tab) = mail_app();
        super::commit_mail_headers(&mut app, tab, Some("subject"));
        assert!(app.noted.is_empty());
        let mut headers = app
            .active_tab()
            .unwrap()
            .mail
            .as_ref()
            .unwrap()
            .headers
            .clone();
        headers.subject = "Changed".into();
        app.ctl.preview_headers(tab, headers).unwrap();
        super::commit_mail_headers(&mut app, tab, Some("subject"));
        assert_eq!(app.noted.len(), 1);
        super::commit_mail_headers(&mut app, tab, Some("subject"));
        assert_eq!(app.noted.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn ui_attachment_paths_are_checked_before_the_json_hop() {
        use std::os::unix::ffi::OsStringExt;
        let (mut app, tab) = mail_app();
        let mut headers = app
            .active_tab()
            .unwrap()
            .mail
            .as_ref()
            .unwrap()
            .headers
            .clone();
        headers
            .attachments
            .push(std::ffi::OsString::from_vec(b"/tmp/report-\xff".to_vec()).into());
        let expected = maildoc::write(&headers.document("Body".into()))
            .unwrap_err()
            .to_string();
        super::apply(
            &mut app,
            &crate::actions::registry(),
            vec![super::UiEvent::MailHeaders(tab, headers)],
        );
        let t = app.active_tab().unwrap();
        assert_eq!(
            t.mail.as_ref().unwrap().notice.as_deref(),
            Some(expected.as_str())
        );
        assert!(t.mail.as_ref().unwrap().headers.attachments.is_empty());
        assert!(!t.dirty());
        assert!(app.effects.is_empty());
    }

    #[test]
    fn frame_headers_precede_save_or_send_in_either_event_order() {
        use documents::controller::Effect;
        for action in ["file.save", "file.send"] {
            for command_first in [false, true] {
                let (mut app, tab) = mail_app();
                let mut headers = app
                    .active_tab()
                    .unwrap()
                    .mail
                    .as_ref()
                    .unwrap()
                    .headers
                    .clone();
                headers.subject = "Shown in this frame".into();
                let mut events = vec![
                    super::UiEvent::MailHeaders(tab, headers),
                    super::UiEvent::Command(action, toolkit::action::Origin::Shortcut),
                ];
                if command_first {
                    events.reverse();
                }
                super::apply(&mut app, &crate::actions::registry(), events);
                assert!(app.effects.iter().any(|e| matches!(e, Effect::SaveMail { text, .. } if maildoc::parse(text).unwrap().subject == "Shown in this frame")));
            }
        }
    }

    #[test]
    fn ui_previews_publish_only_on_commit_and_busy_disables_headers_and_attach() {
        use documents::actions::ActionId;
        let (mut app, tab) = mail_app();
        let registry = crate::actions::registry();
        for subject in ["C", "Ch", "Changed"] {
            let mut headers = app
                .active_tab()
                .unwrap()
                .mail
                .as_ref()
                .unwrap()
                .headers
                .clone();
            headers.subject = subject.into();
            super::apply(
                &mut app,
                &registry,
                vec![super::UiEvent::MailHeaders(tab, headers)],
            );
            assert!(app.effects.is_empty(), "previews execute no action");
            assert!(registry.take_executed().0.is_empty());
        }
        super::apply(
            &mut app,
            &registry,
            vec![super::UiEvent::MailCommit(tab, None)],
        );
        assert_eq!(
            registry.take_executed().0.len(),
            1,
            "commit executes mail.headers once"
        );
        app.action(ActionId::FileSend);
        assert!(!app.enabled(ActionId::MailHeaders));
        super::apply(&mut app, &registry, vec![super::UiEvent::MailAttach(tab)]);
        assert!(app.mail_chooser.is_none());
        let fx = app.ctl.on_mail_saved(
            tab,
            "/home/user/letter.md".into(),
            "---\nmail-subject: Changed\nmail-to: bob@example.org\n---\nBody".into(),
            true,
            None,
            Ok(()),
        );
        app.absorb(fx);
        assert!(
            !app.enabled(ActionId::MailHeaders),
            "Send still in flight after its save"
        );
        super::apply(&mut app, &registry, vec![super::UiEvent::MailAttach(tab)]);
        assert!(app.mail_chooser.is_none());
        let fx = app.ctl.on_mail_sent(
            tab,
            None,
            Err(documents::mail::Failure::new(
                "INVALID_ARGUMENT",
                "server refused",
            )),
        );
        app.absorb(fx);
        assert!(app.enabled(ActionId::MailHeaders));
    }

    #[test]
    fn committing_the_old_field_does_not_split_typing_in_the_new_field() {
        let (mut app, tab) = mail_app();
        let registry = crate::actions::registry();
        let mut headers = app
            .active_tab()
            .unwrap()
            .mail
            .as_ref()
            .unwrap()
            .headers
            .clone();
        headers.subject = "Changed".into();
        super::apply(
            &mut app,
            &registry,
            vec![super::UiEvent::MailHeaders(tab, headers.clone())],
        );
        headers.to = "c".into();
        super::apply(
            &mut app,
            &registry,
            vec![
                super::UiEvent::MailHeaders(tab, headers.clone()),
                super::UiEvent::MailCommit(tab, Some("subject")),
            ],
        );
        headers.to = "carol@example.org".into();
        super::apply(
            &mut app,
            &registry,
            vec![super::UiEvent::MailHeaders(tab, headers)],
        );
        super::apply(
            &mut app,
            &registry,
            vec![super::UiEvent::Command(
                "edit.undo",
                toolkit::action::Origin::Shortcut,
            )],
        );
        let headers = &app.active_tab().unwrap().mail.as_ref().unwrap().headers;
        assert_eq!(headers.to, "bob@example.org");
        assert_eq!(headers.subject, "Changed");
    }

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
