// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The egui rendering of dopus. [`view`] reads the app and returns what the
//! person did as [`UiEvent`]s; [`apply`] runs them after the frame, so
//! drawing never changes state.
//!
//! The window, top to bottom: the toolkit title bar with the menus (every
//! action is there, with its shortcut), the body, and the status bar. The
//! body is the Places sidebar, the two panes with a divider between them,
//! and the Properties sidebar, each titled at its top. Each pane's top row
//! is its path in a text field; the Places toggle sits left of the left
//! pane's path, the Properties toggle right of the right pane's. Each
//! pane has its location field, the column
//! headers (click to sort), the file list and a summary footer.
//!
//! The file list is virtualised: only the rows on screen are laid out.
//! Rows are a tree: a chevron or a double-click opens or closes a folder
//! in place; a double-click on a file opens it.
//! Dragging rows onto a folder (or the background) of the other pane asks
//! Move, Copy or Cancel.

use std::path::PathBuf;

use dopus_core::config::Sidebar;
use dopus_core::properties::Properties;
use dopus_core::{PaneId, SortColumn, VisibleRow};
use egui::{
    Align, Align2, FontId, Id, Key, Layout, Rect, RichText, ScrollArea, Sense, TextStyle, Ui,
    UiBuilder, pos2, vec2,
};
use toolkit::action::Origin;
use toolkit::dialog::{self, Choice, Role};
use toolkit::titlebar::Action;
use toolkit::{Icon, Registry, Strings, bars, icons};

use crate::app::{Answer, App, Dialog};
use crate::{label, label_with};

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// A command, and how it was reached (for `dopus.action.executed`).
    Command(&'static str, Origin),
    CloseMacros,
    CloseSearch,
    /// A context-menu action on a pane (it activates first).
    CommandIn(&'static str, PaneId),
    Activate(PaneId),
    Click {
        pane: PaneId,
        path: PathBuf,
        ctrl: bool,
        shift: bool,
    },
    /// Right-click: select the row unless it is in the selection already.
    Point {
        pane: PaneId,
        path: PathBuf,
    },
    Toggle {
        pane: PaneId,
        path: PathBuf,
    },
    /// A double-click on a file: select it and open it.
    OpenFile {
        pane: PaneId,
        path: PathBuf,
    },
    Sort {
        pane: PaneId,
        column: SortColumn,
    },
    EditLocation(PaneId),
    LocationText(String),
    SubmitLocation,
    CancelLocation,
    LocationFocused,
    Go(PathBuf),
    Split(f32),
    SidebarWidth(Sidebar, f32),
    Drop {
        source: PaneId,
        paths: Vec<PathBuf>,
        destination: PathBuf,
    },
    PromptText(String),
    Answer(Answer),
}

/// What a row drag carries.
#[derive(Clone, Debug)]
struct Dragged {
    pane: PaneId,
    paths: Vec<PathBuf>,
    name: String,
}

const ICON: f32 = 16.0;
const INDENT: f32 = 14.0;
#[derive(Clone, Copy)]
struct ColumnWidths {
    size: f32,
    modified: f32,
}

#[derive(Clone, PartialEq)]
struct ColumnKey {
    body: FontId,
    header: FontId,
    labels: (String, String),
    generation: u64,
    revision: u64,
    sort: SortColumn,
    ascending: bool,
    hidden: bool,
    expanded: std::collections::HashSet<PathBuf>,
    rows: usize,
}

#[derive(Clone)]
struct ColumnCache {
    key: ColumnKey,
    widths: ColumnWidths,
    #[cfg(test)]
    measured_rows: usize,
}

fn cached_column_widths(
    ui: &Ui,
    model: &dopus_core::PaneModel,
    rows: &[VisibleRow],
) -> ColumnWidths {
    let key = ColumnKey {
        body: TextStyle::Body.resolve(ui.style()),
        header: TextStyle::Small.resolve(ui.style()),
        labels: (label("column-size"), label("column-modified")),
        generation: model.generation,
        revision: model.listing_revision,
        sort: model.sort,
        ascending: model.ascending,
        hidden: model.show_hidden,
        expanded: model.expanded.clone(),
        rows: rows.len(),
    };
    widths_for_key(ui, key, rows)
}

fn widths_for_key(ui: &Ui, key: ColumnKey, rows: &[VisibleRow]) -> ColumnWidths {
    let id = ui.scope_id().with("metadata-widths");
    let previous = ui.data(|d| d.get_temp::<ColumnCache>(id));
    if let Some(cache) = &previous
        && cache.key == key
    {
        return cache.widths;
    }
    let widths = column_widths(ui, rows);
    ui.data_mut(|d| {
        d.insert_temp(
            id,
            ColumnCache {
                key,
                widths,
                #[cfg(test)]
                measured_rows: previous.map_or(0, |c| c.measured_rows) + rows.len(),
            },
        )
    });
    widths
}

fn column_widths(ui: &Ui, rows: &[VisibleRow]) -> ColumnWidths {
    let body = TextStyle::Body.resolve(ui.style());
    let font = FontId::new(body.size * 0.92, body.family);
    let measure = |text: String, font: FontId| {
        ui.painter()
            .layout_no_wrap(text, font, ui.visuals().text_color())
            .size()
            .x
    };
    let header = TextStyle::Small.resolve(ui.style());
    let mut widths = ColumnWidths {
        size: measure(format!("{} ▼", label("column-size")), header.clone()),
        modified: measure(format!("{} ▼", label("column-modified")), header),
    };
    for row in rows {
        let entry = &row.entry;
        let size = if entry.is_dir {
            dopus_core::format_child_count(entry.child_count)
        } else {
            entry
                .size
                .map_or_else(|| "—".to_owned(), dopus_core::format_size)
        };
        widths.size = widths.size.max(measure(size, font.clone()));
        widths.modified = widths.modified.max(measure(
            entry
                .modified
                .map_or_else(|| "—".to_owned(), dopus_core::format_modified_at),
            font.clone(),
        ));
    }
    // Six pixels on both sides, including the gap before right-aligned text.
    widths.size = widths.size.ceil() + 12.0;
    widths.modified = widths.modified.ceil() + 12.0;
    widths
}
/// The Name column's narrowest before Modified, then Size, hide.
const NAME_MIN: f32 = 140.0;
const DIVIDER: f32 = 6.0;

#[cfg(test)]
mod column_tests {
    use super::*;

    #[test]
    fn fifty_thousand_rows_are_measured_only_when_the_cache_key_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = dopus_core::DOpusConfig::default();
        config.left.path = dir.path().to_owned();
        config.right.path = dir.path().to_owned();
        let (core, _rx) = dopus_core::DopusCore::new(config, None);
        let model = core.pane(PaneId::Left);
        let rows: Vec<_> = (0..50_000)
            .map(|i| VisibleRow {
                entry: dopus_core::FileEntry {
                    path: PathBuf::from(format!("sample-{i}.mkv")),
                    name: format!("sample-{i}.mkv"),
                    is_dir: false,
                    is_file: true,
                    size: Some(i * 1024 * 1024),
                    child_count: None,
                    modified: Some(std::time::SystemTime::UNIX_EPOCH),
                },
                depth: 0,
            })
            .collect();
        let ctx = egui::Context::default();
        let mut key = None;
        for frame in 0..5 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                if key.is_none() {
                    let _ = cached_column_widths(ui, model, &rows);
                    key = Some(
                        ui.data(|d| {
                            d.get_temp::<ColumnCache>(ui.scope_id().with("metadata-widths"))
                        })
                        .unwrap()
                        .key,
                    );
                }
                let key = key.as_mut().unwrap();
                if frame == 2 {
                    key.revision += 1;
                }
                if frame == 3 {
                    key.body = FontId::proportional(18.0);
                }
                // Keep the model stable; exercise the exact production key
                // lookup with controlled revision/font changes.
                let _ = widths_for_key(ui, key.clone(), &rows);
                let measured = ui
                    .data(|d| d.get_temp::<ColumnCache>(ui.scope_id().with("metadata-widths")))
                    .unwrap()
                    .measured_rows;
                assert_eq!(
                    measured,
                    match frame {
                        0 | 1 => 50_000,
                        2 => 100_000,
                        _ => 150_000,
                    }
                );
            });
            output.textures_delta.clear();
        }
    }

    #[test]
    fn long_name_leaves_measured_size_and_modified_a_gap() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.style_mut()
                .text_styles
                .insert(TextStyle::Body, FontId::proportional(14.667));
            let rows = vec![VisibleRow {
                entry: dopus_core::FileEntry {
                    path: PathBuf::from("Blue Murder Motel extended episode name.mkv"),
                    name: "Blue Murder Motel extended episode name.mkv".into(),
                    is_dir: false,
                    is_file: true,
                    size: Some(565 * 1024 * 1024),
                    child_count: None,
                    modified: Some(std::time::SystemTime::UNIX_EPOCH),
                },
                depth: 0,
            }];
            let widths = column_widths(ui, &rows);
            let name_font = TextStyle::Body.resolve(ui.style());
            let font = FontId::proportional(14.667 * 0.92);
            let measure = |text: &str| {
                ui.painter()
                    .layout_no_wrap(text.into(), font.clone(), ui.visuals().text_color())
                    .size()
                    .x
            };
            for modified in [false, true] {
                let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(360.0, 24.0));
                let (name, size, date) = column_rects(rect, widths, true, modified);
                let size = size.unwrap();
                let name_x = 44.0;
                let text = crate::elide::fit(
                    ui,
                    &rows[0].entry.name,
                    &name_font,
                    name.right() - name_x - 4.0,
                );
                assert!(text.contains('…') && text.ends_with(".mkv"));
                let name_right = name_x
                    + ui.painter()
                        .layout_no_wrap(text, name_font.clone(), ui.visuals().text_color())
                        .size()
                        .x;
                let size_left = size.right() - 6.0 - measure("565.0 MiB");
                assert!(
                    size_left - name_right >= 6.0,
                    "name overlaps size: {name_right} / {size_left}"
                );
                if let Some(date) = date {
                    let text = dopus_core::format_modified_at(std::time::SystemTime::UNIX_EPOCH);
                    assert!(date.right() - 6.0 - measure(&text) >= size.right() + 6.0);
                }
            }
        });
        output.textures_delta.clear();
    }
}

/// The window title: the active path.
pub fn title(app: &App) -> String {
    let pane = app.core.pane(app.core.active());
    format!(
        "{} — MixOS DOpus",
        dopus_core::sanitise_display_path(&pane.path)
    )
}

/// Draw the whole window into `ui`.
pub fn view(ui: &mut Ui, app: &App, commands: &Registry<App>, strings: &Strings) -> Vec<UiEvent> {
    let mut events = Vec::new();
    // The icon bar: the two sidebar toggles, then the standard tail.
    let sidebars = [Side::Left, Side::Right].map(|s| Action::new(s.command(), s.icon()));
    let fired = toolkit::titlebar::show_bar(
        ui,
        &title(app),
        Some(Icon::Folder),
        commands,
        app,
        strings,
        &sidebars,
    );
    events.extend(
        fired
            .into_iter()
            .map(|(id, origin)| UiEvent::Command(id, origin)),
    );
    bars::status(ui, |ui| {
        ui.small(app.info());
    });
    let total = ui.available_width();
    let places = app.core.sidebar(Sidebar::Places);
    // Preserve sidebar choices while lending their space to the file panes
    // in a narrow tile. Both sidebars return when the window grows.
    if places.open && total >= 640.0 {
        let shown = egui::Panel::left("places")
            .resizable(true)
            .default_size(total * places.width)
            .frame(bars::dock_frame(ui.ctx()))
            .show(ui, |ui| {
                toolkit::panel::section_label(ui, &Side::Left.title());
                places_panel(ui, app, &mut events);
            });
        sidebar_width(
            Sidebar::Places,
            shown.response.rect.width(),
            total,
            places.width,
            &mut events,
        );
    }
    let properties = app.core.sidebar(Sidebar::Properties);
    if properties.open && total >= 640.0 {
        let shown = egui::Panel::right("properties")
            .resizable(true)
            .default_size(total * properties.width)
            .frame(bars::dock_frame(ui.ctx()))
            .show(ui, |ui| {
                toolkit::panel::section_label(ui, &Side::Right.title());
                properties_panel(ui, app);
            });
        sidebar_width(
            Sidebar::Properties,
            shown.response.rect.width(),
            total,
            properties.width,
            &mut events,
        );
    }
    egui::CentralPanel::default()
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| panes(ui, app, commands, strings, &mut events));
    drag_preview(ui.ctx());
    dialogs(ui.ctx(), app, commands, strings, &mut events);
    if let Some(chosen) = toolkit::search::show(ui.ctx(), commands, app, strings, app.search_open) {
        events.push(UiEvent::CloseSearch);
        if let Some((id, origin)) = chosen { events.push(UiEvent::Command(id, origin)); }
    }
    if toolkit::macros::show_app(ui.ctx(), "dopus", app.macros_open) {
        events.push(UiEvent::CloseMacros);
    }
    toolkit::titlebar::edges(ui);
    events
}

fn sidebar_width(sidebar: Sidebar, width: f32, total: f32, known: f32, events: &mut Vec<UiEvent>) {
    let share = width / total.max(1.0);
    if (share - known).abs() > 0.005 {
        events.push(UiEvent::SidebarWidth(sidebar, share));
    }
}

/// Which sidebar.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
}

impl Side {
    fn command(self) -> &'static str {
        match self {
            Side::Left => "view.toggle-places",
            Side::Right => "view.toggle-properties",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Side::Left => Icon::PanelLeft,
            Side::Right => Icon::PanelRight,
        }
    }

    fn title(self) -> String {
        label(match self {
            Side::Left => "places",
            Side::Right => "properties",
        })
    }
}

pub(crate) fn places_panel(ui: &mut Ui, app: &App, events: &mut Vec<UiEvent>) {
    let active = &app.core.pane(app.core.active()).path;
    let font = TextStyle::Body.resolve(ui.style());
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        for (name, path) in app.core.places() {
            ui.horizontal(|ui| {
                let icon = crate::icons::place_icon(name);
                let colour = ui.visuals().weak_text_color();
                ui.add(icons::image_of(ui.ctx(), icon, ICON, colour));
                let width = (ui.available_width() - 2.0 * ui.spacing().button_padding.x).max(0.0);
                let text = crate::elide::fit(ui, name, &font, width);
                let row = toolkit::selectable(ui, active == path, text)
                    .on_hover_text(dopus_core::sanitise_display_path(path));
                if row.clicked() {
                    events.push(UiEvent::Go(path.clone()));
                }
            });
        }
    });
}

/// The tallest the Properties preview grows (logical px).
const PREVIEW_MAX_HEIGHT: f32 = 240.0;

/// The selection's thumbnail at the top of Properties: the panel's width
/// at most, never larger than its own pixels, centred.
fn preview_image(ui: &mut Ui, texture: &egui::TextureHandle) {
    let natural = texture.size_vec2() / ui.ctx().pixels_per_point();
    let scale = (ui.available_width() / natural.x)
        .min(PREVIEW_MAX_HEIGHT / natural.y)
        .min(1.0);
    ui.vertical_centered(|ui| {
        ui.add(egui::Image::new(texture).fit_to_exact_size(natural * scale));
    });
    ui.add_space(8.0);
}

fn properties_panel(ui: &mut Ui, app: &App) {
    let field = |ui: &mut Ui, key: &str, value: &str| {
        ui.label(RichText::new(label(key)).small().weak());
        ui.add(egui::Label::new(value).wrap());
        ui.add_space(4.0);
    };
    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        match app.core.properties(app.core.active()) {
            Properties::Folder { path, summary } => {
                field(ui, "prop-folder", &path);
                field(ui, "prop-contents", &summary);
            }
            Properties::Entry {
                entry,
                count_pending,
                metadata,
            } => {
                if let Some(texture) = app
                    .preview
                    .as_ref()
                    .filter(|p| p.key.1 == entry.path)
                    .and_then(|p| p.texture.as_ref())
                {
                    preview_image(ui, texture);
                }
                field(ui, "prop-name", &entry.name);
                match metadata {
                    None => field(ui, "prop-kind", "…"),
                    Some(Err(error)) => field(ui, "prop-error", &error),
                    Some(Ok(m)) => {
                        let time = |t: Option<std::time::SystemTime>| {
                            t.map_or_else(|| "—".to_owned(), dopus_core::format_modified_at)
                        };
                        field(ui, "prop-kind", &m.kind);
                        let size = if entry.is_dir {
                            if count_pending {
                                "…".to_owned()
                            } else {
                                dopus_core::format_child_count(entry.child_count)
                            }
                        } else {
                            dopus_core::format_size(m.size)
                        };
                        field(ui, "prop-size", &size);
                        field(ui, "prop-modified", &time(m.modified));
                        field(ui, "prop-created", &time(m.created));
                        field(ui, "prop-accessed", &time(m.accessed));
                        field(ui, "prop-permissions", &m.permissions);
                        field(ui, "prop-owner", &m.owner_group);
                        if let Some(target) = &m.symlink_target {
                            field(ui, "prop-link", target);
                        }
                    }
                }
                field(
                    ui,
                    "prop-path",
                    &dopus_core::sanitise_display_path(&entry.path),
                );
            }
        }
    });
}

fn panes(
    ui: &mut Ui,
    app: &App,
    commands: &Registry<App>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let area = ui.available_rect_before_wrap();
    let ratio = app
        .core
        .config_snapshot()
        .split_ratio
        .clamp(crate::config::SPLIT_MIN, crate::config::SPLIT_MAX);
    let left_w = ((area.width() - DIVIDER) * ratio).round();
    let left = Rect::from_min_size(area.min, vec2(left_w, area.height()));
    let divider = Rect::from_min_size(pos2(left.right(), area.top()), vec2(DIVIDER, area.height()));
    let right = Rect::from_min_max(pos2(divider.right(), area.top()), area.max);
    // Law 7: the divider sets the split ratio; a double-click restores half.
    let grip = ui.interact(
        divider,
        Id::unique("dopus-divider"),
        Sense::click_and_drag(),
    );
    if grip.hovered() || grip.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if grip.double_clicked() {
        events.push(UiEvent::Split(0.5));
    } else if grip.dragged()
        && let Some(pointer) = grip.interact_pointer_pos()
    {
        let share = (pointer.x - area.left()) / (area.width() - DIVIDER).max(1.0);
        events.push(UiEvent::Split(
            share.clamp(crate::config::SPLIT_MIN, crate::config::SPLIT_MAX),
        ));
    }
    let line = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter()
        .vline(divider.center().x, divider.y_range(), line);
    for (pane, rect) in [(PaneId::Left, left), (PaneId::Right, right)] {
        ui.scope_builder(
            UiBuilder::new().max_rect(rect).id_salt(pane.index()),
            |ui| {
                if ui.ctx().content_rect().width() < 640.0 {
                    ui.set_clip_rect(ui.clip_rect().intersect(rect));
                }
                pane_view(ui, app, commands, strings, pane, events);
            },
        );
    }
}

fn pane_view(
    ui: &mut Ui,
    app: &App,
    commands: &Registry<App>,
    strings: &Strings,
    pane: PaneId,
    events: &mut Vec<UiEvent>,
) {
    pane_body(ui, app, commands, strings, pane, false, events);
}

/// The file manager's pane, with destructive menus and drag/drop removed.
pub(crate) fn chooser_pane(
    ui: &mut Ui,
    app: &App,
    commands: &Registry<App>,
    events: &mut Vec<UiEvent>,
) {
    pane_body(
        ui,
        app,
        commands,
        &crate::strings(),
        PaneId::Left,
        true,
        events,
    );
}

fn pane_body(
    ui: &mut Ui,
    app: &App,
    commands: &Registry<App>,
    strings: &Strings,
    pane: PaneId,
    chooser: bool,
    events: &mut Vec<UiEvent>,
) {
    let active = app.core.active() == pane;
    let model = app.core.pane(pane);
    let accent = ui.visuals().selection.bg_fill;
    if active {
        ui.painter().rect_stroke(
            ui.max_rect().shrink(0.5),
            2.0,
            egui::Stroke::new(1.0, accent),
            egui::StrokeKind::Inside,
        );
    }
    ui.add_space(2.0);
    location_bar(ui, app, pane, active, events);
    let width = ui.available_width();
    let rows = app.core.visible_rows(pane);
    let widths = cached_column_widths(ui, model, &rows);
    let (show_size, show_modified) = (
        width >= NAME_MIN + widths.size,
        width >= NAME_MIN + widths.size + widths.modified,
    );
    // A short chooser needs browsing space before column headings or counts.
    let compact_chooser = chooser && ui.ctx().content_rect().height() < 320.0;
    if !compact_chooser {
        header(ui, app, pane, (widths, show_size, show_modified), events);
    }
    let row_h = (ui.text_style_height(&TextStyle::Body) + 6.0).round();
    let footer_h = if compact_chooser {
        0.0
    } else {
        ui.text_style_height(&TextStyle::Small) + 6.0
    };
    let list_h = (ui.available_height() - footer_h).max(row_h);
    // The pane's background, registered before the rows so they sit on top
    // of it: a click on the empty part activates the pane, a drop there
    // lands in its folder, and its context menu offers the pane's commands.
    let background = ui.interact(
        Rect::from_min_size(ui.cursor().min, vec2(width, list_h)),
        Id::unique((ui.ctx().viewport_id(), "pane-background", pane.index())),
        Sense::click(),
    );
    ui.allocate_ui(vec2(width, list_h), |ui| {
        ScrollArea::vertical()
            .id_salt(("rows", pane.index()))
            .auto_shrink(false)
            .show_rows(ui, row_h, rows.len(), |ui, range| {
                for index in range {
                    row(
                        ui,
                        app,
                        commands,
                        strings,
                        pane,
                        &rows[index],
                        chooser,
                        (row_h, widths, show_size, show_modified),
                        events,
                    );
                }
            });
    });
    if background.clicked() {
        events.push(UiEvent::Activate(pane));
    }
    if !chooser {
        drop_target(&background, pane, model.path.clone(), events);
        background.context_menu(|ui| {
            for id in [
                "file.new-folder",
                "view.refresh",
                "view.toggle-hidden",
                "view.toggle-previews",
            ] {
                context_item(ui, app, commands, strings, id, pane, events);
            }
        });
    }
    if !compact_chooser {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.small(if chooser {
                app.core.visible_summary(pane)
            } else {
                model.footer_summary()
            });
        });
    }
}

/// A pane's top row: the path in a text field (the sidebar toggles live in
/// the icon bar). Focusing the field starts an edit with the whole path
/// selected; Enter goes there, Esc or a click away puts it back.
fn location_bar(ui: &mut Ui, app: &App, pane: PaneId, active: bool, events: &mut Vec<UiEvent>) {
    let height = toolkit::button::icon_button_of(ui.ctx()).max(ui.spacing().interact_size.y + 4.0);
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.add_space(4.0);
            let width = (ui.available_width() - 4.0).max(40.0);
            let id = Id::unique((ui.ctx().viewport_id(), "location", pane.index()));
            match &app.editing {
                Some((editing, text)) if *editing == pane => {
                    let mut text = text.clone();
                    let response = ui
                        .add(
                            egui::TextEdit::singleline(&mut text)
                                .id(id)
                                .desired_width(width),
                        )
                        .accessible_name(label("prop-path"));
                    if app.focus_location {
                        // The whole path selected, so typing replaces it.
                        response.request_focus();
                        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                            let all = egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(text.chars().count()),
                            );
                            state.cursor.set_char_range(Some(all));
                            state.store(ui.ctx(), id);
                        }
                        events.push(UiEvent::LocationFocused);
                    }
                    if response.changed() {
                        events.push(UiEvent::LocationText(text));
                    }
                    if response.lost_focus() {
                        if ui.input(|i| i.key_pressed(Key::Enter)) {
                            events.push(UiEvent::SubmitLocation);
                        } else {
                            events.push(UiEvent::CancelLocation);
                        }
                    }
                }
                _ => {
                    // The path as a field at rest; taking the keyboard
                    // starts the edit (the same id, so focus carries over).
                    let mut path = dopus_core::sanitise_display_path(&app.core.pane(pane).path);
                    let colour = if active {
                        ui.visuals().text_color()
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut path)
                            .id(id)
                            .desired_width(width)
                            .text_color(colour),
                    );
                    let response = response.accessible_name(label("prop-path"));
                    if response.gained_focus() || response.clicked() {
                        events.push(UiEvent::EditLocation(pane));
                    }
                }
            }
        },
    );
}

fn header(
    ui: &mut Ui,
    app: &App,
    pane: PaneId,
    (widths, show_size, show_modified): (ColumnWidths, bool, bool),
    events: &mut Vec<UiEvent>,
) {
    let model = app.core.pane(pane);
    let arrow = |column| {
        if model.sort == column {
            if model.ascending { " ▲" } else { " ▼" }
        } else {
            ""
        }
    };
    let height = ui.text_style_height(&TextStyle::Small) + 8.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let columns = column_rects(rect, widths, show_size, show_modified);
    for (column, key, cell, align) in [
        (
            SortColumn::Name,
            "column-name",
            Some(columns.0),
            Align2::LEFT_CENTER,
        ),
        (
            SortColumn::Size,
            "column-size",
            columns.1,
            Align2::RIGHT_CENTER,
        ),
        (
            SortColumn::Modified,
            "column-modified",
            columns.2,
            Align2::RIGHT_CENTER,
        ),
    ] {
        let Some(cell) = cell else { continue };
        let response = ui
            .interact(
                cell,
                Id::unique((ui.ctx().viewport_id(), "header", pane.index(), key)),
                Sense::click(),
            )
            .on_hover_text(label_with("sort-by", &[("column", &label(key))]));
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label(key)));
        if response.hovered() {
            ui.painter()
                .rect_filled(cell, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        let x = if align == Align2::LEFT_CENTER {
            cell.left() + 6.0
        } else {
            cell.right() - 6.0
        };
        ui.painter().text(
            pos2(x, cell.center().y),
            align,
            format!("{}{}", label(key), arrow(column)),
            TextStyle::Small.resolve(ui.style()),
            ui.visuals().weak_text_color(),
        );
        if response.clicked() {
            events.push(UiEvent::Sort { pane, column });
        }
    }
    let line = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter().hline(rect.x_range(), rect.bottom(), line);
}

/// Name, Size and Modified cells in `rect`; the narrow ones hide first.
fn column_rects(
    rect: Rect,
    widths: ColumnWidths,
    show_size: bool,
    show_modified: bool,
) -> (Rect, Option<Rect>, Option<Rect>) {
    let mut right = rect.right();
    let modified = show_modified.then(|| {
        let cell = Rect::from_min_max(
            pos2(right - widths.modified, rect.top()),
            pos2(right, rect.bottom()),
        );
        right -= widths.modified;
        cell
    });
    let size = show_size.then(|| {
        let cell = Rect::from_min_max(
            pos2(right - widths.size, rect.top()),
            pos2(right, rect.bottom()),
        );
        right -= widths.size;
        cell
    });
    let name = Rect::from_min_max(rect.min, pos2(right, rect.bottom()));
    (name, size, modified)
}

#[expect(clippy::too_many_arguments, reason = "a row's inputs")]
fn row(
    ui: &mut Ui,
    app: &App,
    commands: &Registry<App>,
    strings: &Strings,
    pane: PaneId,
    visible: &VisibleRow,
    chooser: bool,
    (row_h, widths, show_size, show_modified): (f32, ColumnWidths, bool, bool),
    events: &mut Vec<UiEvent>,
) {
    let entry = &visible.entry;
    let model = app.core.pane(pane);
    let selected = model.selected_paths.contains(&entry.path);
    let focused = model.selected.as_ref() == Some(&entry.path);
    let expanded = entry.is_dir && model.expanded.contains(&entry.path);
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, selected, &entry.name)
    });
    let visuals = ui.visuals().clone();
    let fill = if selected {
        Some(visuals.selection.bg_fill)
    } else if response.hovered() {
        Some(visuals.widgets.hovered.weak_bg_fill)
    } else {
        None
    };
    // A folder of the other pane under a drag shows it will take the drop.
    let target = entry.is_dir
        && response
            .dnd_hover_payload::<Dragged>()
            .is_some_and(|d| d.pane != pane);
    let painter = ui.painter_at(rect);
    if let Some(fill) = fill {
        painter.rect_filled(rect, 2.0, fill);
    }
    if target {
        painter.rect_stroke(
            rect.shrink(1.0),
            2.0,
            egui::Stroke::new(1.5, visuals.selection.stroke.color),
            egui::StrokeKind::Inside,
        );
    }
    if focused && app.core.active() == pane {
        painter.rect_stroke(
            rect.shrink(0.5),
            2.0,
            egui::Stroke::new(1.0, visuals.selection.stroke.color.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
    }
    let text_colour = if selected {
        visuals.selection.stroke.color
    } else {
        visuals.text_color()
    };
    let (name_cell, size_cell, modified_cell) =
        column_rects(rect, widths, show_size, show_modified);
    let mut x = name_cell.left() + 4.0 + visible.depth as f32 * INDENT;
    let mid = rect.center().y;
    // The chevron opens a folder in place.
    if entry.is_dir {
        let chevron = Rect::from_center_size(pos2(x + ICON / 2.0, mid), vec2(ICON, ICON));
        let toggle = ui.interact(chevron, response.id.with("chevron"), Sense::click());
        let name = label_with(
            if expanded { "collapse" } else { "expand" },
            &[("name", &entry.name)],
        );
        toggle.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        let icon = if expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        icons::image_of(ui.ctx(), icon, ICON * 0.8, visuals.weak_text_color())
            .paint_at(ui, chevron.shrink(1.0));
        if toggle.clicked() {
            events.push(UiEvent::Toggle {
                pane,
                path: entry.path.clone(),
            });
        }
    }
    x += ICON + 2.0;
    let icon_rect = Rect::from_center_size(pos2(x + ICON / 2.0, mid), vec2(ICON, ICON));
    let file_icon = crate::icons::file_icon(&entry.path, entry.is_dir, expanded);
    let icon_colour = if selected {
        text_colour
    } else if entry.is_dir {
        visuals.hyperlink_color
    } else {
        visuals.weak_text_color()
    };
    icons::image_of(ui.ctx(), file_icon, ICON, icon_colour).paint_at(ui, icon_rect);
    x += ICON + 6.0;
    let font = TextStyle::Body.resolve(ui.style());
    let name = crate::elide::fit(
        ui,
        &entry.name,
        &font,
        (name_cell.right() - x - 4.0).max(0.0),
    );
    painter.text(
        pos2(x, mid),
        Align2::LEFT_CENTER,
        name,
        font.clone(),
        text_colour,
    );
    let small = FontId::new(font.size * 0.92, font.family.clone());
    let dim = if selected {
        text_colour
    } else {
        visuals.weak_text_color()
    };
    if let Some(cell) = size_cell {
        let size = if entry.is_dir {
            dopus_core::format_child_count(entry.child_count)
        } else {
            entry
                .size
                .map_or_else(|| "—".to_owned(), dopus_core::format_size)
        };
        painter.text(
            pos2(cell.right() - 6.0, mid),
            Align2::RIGHT_CENTER,
            size,
            small.clone(),
            dim,
        );
    }
    if let Some(cell) = modified_cell {
        let modified = entry
            .modified
            .map_or_else(|| "—".to_owned(), dopus_core::format_modified_at);
        painter.text(
            pos2(cell.right() - 6.0, mid),
            Align2::RIGHT_CENTER,
            modified,
            small,
            dim,
        );
    }
    // Interaction: click selects (Ctrl toggles, Shift extends); a
    // double-click opens a file, and opens or closes a folder in place.
    let modifiers = ui.input(|i| i.modifiers);
    if response.double_clicked() {
        let path = entry.path.clone();
        events.push(if entry.is_dir && chooser {
            UiEvent::Go(path)
        } else if entry.is_dir {
            UiEvent::Toggle { pane, path }
        } else {
            UiEvent::OpenFile { pane, path }
        });
    } else if response.clicked() {
        events.push(UiEvent::Click {
            pane,
            path: entry.path.clone(),
            ctrl: modifiers.command,
            shift: modifiers.shift,
        });
    }
    if response.secondary_clicked() {
        events.push(UiEvent::Point {
            pane,
            path: entry.path.clone(),
        });
    }
    // Dragging carries the selection when this row is in it, else the row.
    if !chooser && response.drag_started() {
        let paths = if selected {
            app.core.selected_paths(pane)
        } else {
            vec![entry.path.clone()]
        };
        let name = if paths.len() == 1 {
            entry.name.clone()
        } else {
            label_with("items", &[("count", &paths.len().to_string())])
        };
        response.dnd_set_drag_payload(Dragged { pane, paths, name });
    }
    if !chooser && entry.is_dir {
        drop_target(&response, pane, entry.path.clone(), events);
    }
    if chooser {
        return;
    }
    response.context_menu(|ui| {
        for id in [
            "file.open",
            "file.mail",
            "file.copy-other-pane",
            "file.move-other-pane",
            "file.rename",
            "file.delete",
            "file.new-folder",
            "view.refresh",
            "view.toggle-hidden",
        ] {
            context_item(ui, app, commands, strings, id, pane, events);
        }
    });
}

/// A drop of rows from the other pane on `response` lands in `destination`.
fn drop_target(
    response: &egui::Response,
    pane: PaneId,
    destination: PathBuf,
    events: &mut Vec<UiEvent>,
) {
    if let Some(dragged) = response.dnd_release_payload::<Dragged>()
        && dragged.pane != pane
    {
        events.push(UiEvent::Drop {
            source: dragged.pane,
            paths: dragged.paths.clone(),
            destination,
        });
    }
}

fn context_item(
    ui: &mut Ui,
    app: &App,
    commands: &Registry<App>,
    strings: &Strings,
    id: &'static str,
    pane: PaneId,
    events: &mut Vec<UiEvent>,
) {
    let Some(command) = commands.get(id) else {
        return;
    };
    let text = toolkit::action::label_text(strings, command.label);
    if ui
        .add_enabled((command.enabled)(app), egui::Button::new(text))
        .clicked()
    {
        events.push(UiEvent::CommandIn(id, pane));
        ui.close();
    }
}

/// The dragged item's name follows the pointer.
fn drag_preview(ctx: &egui::Context) {
    let Some(dragged) = egui::DragAndDrop::payload::<Dragged>(ctx) else {
        return;
    };
    let Some(pointer) = ctx.pointer_latest_pos() else {
        return;
    };
    egui::Area::new(Id::unique("dopus-drag-preview"))
        .order(egui::Order::Tooltip)
        .fixed_pos(pointer + vec2(12.0, 8.0))
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let colour = ui.visuals().text_color();
                    ui.add(icons::image_of(ui.ctx(), Icon::File, ICON, colour));
                    ui.label(&dragged.name);
                });
            });
        });
}

fn dialogs(
    ctx: &egui::Context,
    app: &App,
    commands: &Registry<App>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let Some(dialog) = &app.dialog else {
        return;
    };
    let id = Id::unique("dopus-dialog").with(match dialog {
        Dialog::Confirm { token, .. } | Dialog::Prompt { token, .. } => *token,
        Dialog::Drop { .. } => u64::MAX - 2,
        Dialog::Shortcuts => u64::MAX - 1,
        Dialog::About => u64::MAX,
    });
    let answered = |chosen: Option<usize>, dismissed: bool, answers: &[Answer]| {
        if dismissed {
            Some(Answer::Cancel)
        } else {
            chosen.and_then(|i| answers.get(i).cloned())
        }
    };
    let answer = match dialog {
        Dialog::Confirm { message, .. } => {
            let shown = dialog::Dialog::new(id, label("confirm"))
                .buttons(vec![
                    Choice::new(label("yes"), Role::Default),
                    Choice::new(label("no"), Role::Cancel),
                ])
                .show(ctx, |ui| ui.add(egui::Label::new(message.as_str()).wrap()));
            answered(
                shown.chosen,
                shown.dismissed,
                &[Answer::Accept, Answer::Cancel],
            )
        }
        Dialog::Prompt { kind, text, .. } => {
            let title = match kind {
                dopus_core::PromptKind::NewFolder => "new-folder",
                dopus_core::PromptKind::Rename => "rename",
            };
            let check = dopus_core::validate_filename(text);
            // Enter in the field submits a valid name: the field gives up
            // focus on Enter, so the dialog's own default-button Enter never
            // sees it.
            let mut submitted = false;
            let shown = dialog::Dialog::new(id, label(title))
                .buttons(vec![
                    Choice::new(label("ok"), Role::Default).enabled(check.is_ok()),
                    Choice::new(label("cancel"), Role::Cancel),
                ])
                .show(ctx, |ui| {
                    let mut value = text.clone();
                    let field = egui::TextEdit::singleline(&mut value)
                        .id(id.with("field"))
                        .desired_width(f32::INFINITY);
                    let response = ui.add(field).accessible_name(label("prop-name"));
                    // Before taking the keyboard back: losing it to Enter is the submit.
                    if response.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && dopus_core::validate_filename(&value).is_ok()
                    {
                        submitted = true;
                    }
                    if !response.has_focus() && ui.memory(|m| m.focused().is_none()) {
                        response.request_focus();
                    }
                    if response.changed() {
                        events.push(UiEvent::PromptText(value.clone()));
                    }
                    if let Err(problem) = &check {
                        ui.label(
                            RichText::new(problem)
                                .small()
                                .color(ui.visuals().warn_fg_color),
                        );
                    }
                });
            if submitted && !shown.dismissed {
                Some(Answer::Accept)
            } else {
                answered(
                    shown.chosen,
                    shown.dismissed,
                    &[Answer::Accept, Answer::Cancel],
                )
            }
        }
        Dialog::Drop {
            paths, destination, ..
        } => {
            let target = destination.file_name().map_or_else(
                || destination.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let shown = dialog::Dialog::new(id, target)
                .buttons(vec![
                    Choice::new(label("move-here"), Role::Default),
                    Choice::new(label("copy-here"), Role::Other),
                    Choice::new(label("cancel"), Role::Cancel),
                ])
                .show(ctx, |ui| {
                    ui.label(label_with(
                        "drop-body",
                        &[("count", &paths.len().to_string())],
                    ));
                });
            answered(
                shown.chosen,
                shown.dismissed,
                &[Answer::Accept, Answer::Other, Answer::Cancel],
            )
        }
        Dialog::Shortcuts => {
            let shown = dialog::Dialog::new(id, label("shortcuts"))
                .buttons(vec![Choice::new(label("done"), Role::Default)])
                .show(ctx, |ui| commands.shortcut_grid(ui, strings));
            answered(shown.chosen, shown.dismissed, &[Answer::Accept])
        }
        Dialog::About => {
            let info = buildinfo::build_info!();
            let shown = dialog::Dialog::new(id, label("about"))
                .buttons(vec![Choice::new(label("done"), Role::Default)])
                .show(ctx, |ui| {
                    ui.add(egui::Label::new(label("about-body")).wrap());
                    ui.label(RichText::new(format!("{} ({})", info.version, info.git_sha)).weak());
                });
            answered(shown.chosen, shown.dismissed, &[Answer::Accept])
        }
    };
    if let Some(answer) = answer {
        dialog::Dialog::reset(ctx, id);
        events.push(UiEvent::Answer(answer));
    }
}

/// Apply one frame's interactions: edits first, then commands, so a
/// command sees every edit made in the same frame.
pub fn apply(app: &mut App, commands: &Registry<App>, events: Vec<UiEvent>) {
    // Dismissal wins over any toggle delivered in this frame.
    let (closed, events): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|e| matches!(e, UiEvent::CloseMacros));
    let (fired, edits): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|e| matches!(e, UiEvent::Command(..) | UiEvent::CommandIn(..)));
    for event in edits.into_iter().chain(fired).chain(closed) {
        match event {
            UiEvent::CloseMacros => app.macros_open = false,
            UiEvent::CloseSearch => app.search_open = false,
            UiEvent::Command(id, origin) => {
                let _ = commands.execute(id, app, origin);
            }
            UiEvent::CommandIn(id, pane) => {
                app.core.set_active_pane(pane);
                let _ = commands.execute(id, app, Origin::Menu);
            }
            UiEvent::Activate(pane) => {
                if app.dialog.is_none() {
                    app.core.set_active_pane(pane);
                }
            }
            UiEvent::Click {
                pane,
                path,
                ctrl,
                shift,
            } => app.click_row(pane, path, ctrl, shift),
            UiEvent::Point { pane, path } => {
                if !app.core.selected_paths(pane).contains(&path) {
                    app.click_row(pane, path, false, false);
                } else {
                    app.core.set_active_pane(pane);
                }
            }
            UiEvent::Toggle { pane, path } => {
                if app.dialog.is_none() {
                    app.core.set_active_pane(pane);
                    app.core.toggle_expand(pane, &path);
                }
            }
            UiEvent::OpenFile { pane, path } => {
                app.click_row(pane, path, false, false);
                app.action_in("file.open", pane);
            }
            UiEvent::Sort { pane, column } => app.sort(pane, column),
            UiEvent::EditLocation(pane) => app.edit_location(pane),
            UiEvent::LocationText(text) => {
                if let Some((_, editing)) = &mut app.editing {
                    *editing = text;
                }
            }
            UiEvent::SubmitLocation => app.submit_location(),
            UiEvent::CancelLocation => app.cancel_location(),
            UiEvent::LocationFocused => app.focus_location = false,
            UiEvent::Go(path) => app.go(path),
            UiEvent::Split(ratio) => app.core.set_split_ratio(ratio),
            UiEvent::SidebarWidth(sidebar, width) => app.core.set_sidebar_width(sidebar, width),
            UiEvent::Drop {
                source,
                paths,
                destination,
            } => app.dropped(source, paths, destination),
            UiEvent::PromptText(text) => {
                if let Some(t) = app.prompt_text() {
                    *t = text;
                }
            }
            UiEvent::Answer(answer) => app.answer(answer),
        }
    }
}
