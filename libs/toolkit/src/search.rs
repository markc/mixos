// SPDX-License-Identifier: MIT OR Apache-2.0
//! Command search for apps without a document find bar or filter.
use crate::{Action, Registry, Strings, action::Origin};

/// Register through `Registry::search_action`; the app owns open/close state.
pub fn command<S>(enabled: fn(&S) -> bool, open: fn(&mut S)) -> Action<S> {
    Action {
        id: "view.search",
        label: "search-menus",
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: Some(crate::Icon::Search),
        enabled,
        run: open,
    }
}

/// `None` stays open; `Some(None)` closes; `Some(Some(id))` runs a result.
pub fn show<S>(ctx: &egui::Context, registry: &Registry<S>, state: &S, strings: &Strings, open: bool)
    -> Option<Option<(&'static str, Origin)>>
{
    let key = crate::window_id(ctx.viewport_id(), "toolkit.command-search");
    if !open {
        ctx.data_mut(|d| d.remove::<String>(key));
        return None;
    }
    let previous = ctx.data(|d| d.get_temp::<String>(key));
    let first = previous.is_none();
    let mut query = previous.unwrap_or_default();
    let mut chosen = None;
    let response = crate::dialog::Dialog::new(key, crate::strings::own("search-menus"))
        .show(ctx, |ui| {
            let field = crate::field::search(ui, &mut query, &crate::strings::own("search-menus"), ui.available_width());
            if first { field.request_focus(); }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for action in registry.search(&query, state, strings) {
                    if action.id != "view.search" && ui.button(crate::action::label_text(strings, action.label)).clicked() {
                        chosen = Some((action.id, Origin::Palette));
                    }
                }
            });
        });
    ctx.data_mut(|d| d.insert_temp(key, query));
    if chosen.is_some() || response.dismissed { Some(chosen) } else { None }
}
