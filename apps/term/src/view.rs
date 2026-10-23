// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The window's drawing: the toolkit title bar with the menus, the tab
//! strip, a notice line when a paste was refused, and the panes, each a
//! whole-pixel border (the focus ring when the tab is split) around its
//! GPU grid. Every colour is a design token; the grid's own colours are
//! the program's.

use crate::app::App;
use crate::label;
use crate::layout;
use egui::{Color32, FontId, Ui};
use settings::terminal::FocusRing;
use toolkit::chrome::Chrome;
use toolkit::command::Origin;
use toolkit::dialog::{self, Choice, Role};
use toolkit::tabs::DocTab;
use toolkit::{Icon, Registry, Strings, titlebar};

/// One interaction from the chrome this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    Command(&'static str, Origin),
    SelectTab(u64),
    CloseTab(u64),
}

/// The title bar and its menus.
pub fn title_bar(
    ui: &mut Ui,
    app: &App,
    commands: &Registry<App>,
    strings: &Strings,
) -> Vec<UiEvent> {
    // The icon bar: the standard tail only.
    titlebar::show_bar(
        ui,
        &label("title"),
        Some(Icon::FileTerminal),
        commands,
        app,
        strings,
        &[],
    )
    .into_iter()
    .map(|(id, origin)| UiEvent::Command(id, origin))
    .collect()
}

/// The tab strip: one tab per terminal tab, titled by its program (OSC
/// titles), with a close box.
pub fn tab_strip(ui: &mut Ui, app: &App) -> Vec<UiEvent> {
    let tabs = &app.shape.tabs;
    if tabs.is_empty() {
        return Vec::new();
    }
    let shown: Vec<DocTab> = tabs
        .iter()
        .map(|tab| {
            // A program's spinner glyph (Claude Code's status) is drawn as the
            // shared half-disc icon, not as text the UI font may lack.
            let (marker, rest) = toolkit::tabs::split_marker(&tab.title);
            DocTab {
                title: rest.to_owned(),
                name: rest.to_owned(),
                marker,
                ..DocTab::default()
            }
        })
        .collect();
    let selected = tabs.iter().position(|tab| tab.active).unwrap_or(0);
    let picked = toolkit::tabs::documents(ui, egui::Id::new("term.tabs"), &shown, selected, None);
    let mut events = Vec::new();
    if let Some(tab) = picked.selected.and_then(|i| tabs.get(i)) {
        events.push(UiEvent::SelectTab(tab.id));
    }
    if let Some(tab) = picked.closed.and_then(|i| tabs.get(i)) {
        events.push(UiEvent::CloseTab(tab.id));
    }
    events
}

/// Refused pastes and unsaved zooms stay visible until resolved.
pub fn notice(ui: &mut Ui, app: &App, font_unsaved: bool) {
    let font_notice = font_unsaved.then(|| label("font-not-saved"));
    for notice in app.paste_notice.iter().chain(font_notice.iter()) {
        let chrome = Chrome::of(ui.ctx());
        egui::Frame::new()
            .fill(chrome.palette.card)
            .inner_margin(egui::Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.colored_label(chrome.palette.warning, notice);
            });
    }
}

/// Every visible pane: its border (the focus ring when there is a choice),
/// its background, and its grid.
pub fn panes(ui: &Ui, app: &App) {
    let chrome = Chrome::of(ui.ctx());
    let painter = ui.painter();
    let scale = app.scale();
    let border = layout::border(scale, app.edges().border);
    let inset = app.inset();
    for (id, geometry) in app.placed() {
        let outer = layout::rect(geometry);
        let ring = if app.shape.focus_ring(id) {
            focus_ring(&chrome, app.focus_ring)
        } else {
            chrome.palette.card_border
        };
        // Whole-pixel border: the ring's colour changes, never its width,
        // so focus never resizes a PTY.
        painter.rect_filled(outer, 0.0, ring);
        painter.rect_filled(outer.shrink(border), 0.0, chrome.palette.canvas);
        if let (Some(pane), Some(&(width, height))) = (app.panes.get(&id), app.targets.get(&id)) {
            let rect = egui::Rect::from_min_size(
                outer.min + egui::vec2(inset, inset),
                egui::vec2(width as f32 / scale, height as f32 / scale),
            );
            // A pane smaller than its smallest grid (two columns, one
            // row) clips the grid at its own border, never drawing over a
            // neighbour; the grid keeps its origin.
            let interior = outer.shrink(border).intersect(painter.clip_rect());
            painter
                .with_clip_rect(interior)
                .add(pane.callback(rect, app.raster.clone()));
        }
    }
    preedit(ui, app, &chrome);
}

/// How far the neutral ring moves from the card border toward the text: a
/// quiet grey step, no hue.
const NEUTRAL_RING: f32 = 0.25;

/// The focused pane's ring (`terminal.focus`): neutral is the card border
/// stepped toward the text colour; accent is the card border taking
/// `strength` of the current theme's accent (a quarter by default).
fn focus_ring(chrome: &Chrome, (ring, strength): (FocusRing, f32)) -> Color32 {
    let palette = &chrome.palette;
    match ring {
        FocusRing::Neutral => palette
            .card_border
            .lerp_to_gamma(palette.text, NEUTRAL_RING),
        FocusRing::Accent => palette
            .card_border
            .lerp_to_gamma(palette.accent, strength.clamp(0.0, 1.0)),
    }
}

/// The input method's composition, over the caret until it commits.
fn preedit(ui: &Ui, app: &App, chrome: &Chrome) {
    let (Some(text), Some(caret)) = (&app.ime_preedit, app.caret()) else {
        return;
    };
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(
        text.clone(),
        FontId::monospace(caret.height() * 0.8),
        chrome.palette.text,
    );
    let rect = egui::Rect::from_min_size(
        caret.min,
        egui::vec2(galley.size().x.max(caret.width()), caret.height()),
    );
    painter.rect_filled(rect, 0.0, chrome.palette.field);
    painter.galley(
        egui::pos2(rect.min.x, rect.center().y - galley.size().y / 2.0),
        galley,
        Color32::PLACEHOLDER,
    );
    painter.hline(
        rect.x_range(),
        rect.bottom() - 1.0,
        egui::Stroke::new(1.0, chrome.palette.accent),
    );
}

/// The Keyboard Shortcuts dialog; true when it was closed.
pub fn keys_dialog(ctx: &egui::Context, commands: &Registry<App>, strings: &Strings) -> bool {
    let response = dialog::Dialog::new(egui::Id::new("term.keys"), label("keys-title"))
        .buttons(vec![Choice::new(label("close"), Role::Default)])
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    egui::Grid::new("term.keys.grid")
                        .striped(true)
                        .show(ui, |ui| {
                            for command in commands.iter() {
                                if let Some(shortcut) = command.shortcut {
                                    ui.label(toolkit::command::label_text(strings, command.label));
                                    ui.monospace(ui.ctx().format_shortcut(&shortcut));
                                    ui.end_row();
                                }
                            }
                        });
                });
        });
    response.chosen.is_some() || response.dismissed
}

/// Build provenance and the live settings source, shared by the dialog and tests.
pub fn about_text(settings_connected: bool) -> String {
    let info = buildinfo::build_info!();
    let source = label(if settings_connected {
        "about-settings-connected"
    } else {
        "about-settings-disconnected"
    });
    format!(
        "{}\n{}\n{}\nMIT OR Apache-2.0\n{}",
        label("title"),
        info.line(),
        label("about-description"),
        source
    )
}

/// The toolkit's standard About dialog; true when it was closed.
pub fn about_dialog(ctx: &egui::Context, settings_connected: bool) -> bool {
    let response = dialog::Dialog::new(egui::Id::new("term.about"), label("about-title"))
        .buttons(vec![Choice::new(label("close"), Role::Default)])
        .show(ctx, |ui| {
            ui.add(egui::Label::new(about_text(settings_connected)).wrap());
        });
    response.chosen.is_some() || response.dismissed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_contains_build_provenance_and_settings_source() {
        let info = buildinfo::build_info!();
        for connected in [false, true] {
            let text = about_text(connected);
            for expected in [
                "Terminal",
                info.version,
                info.git_sha,
                info.build_time,
                "MIT OR Apache-2.0",
            ] {
                assert!(text.contains(expected), "{text}");
            }
            assert!(text.contains(&label(if connected {
                "about-settings-connected"
            } else {
                "about-settings-disconnected"
            })));
        }
    }
}
