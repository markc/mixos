// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! email's commands. The menu bar, the shortcuts and `email.execute` come
//! from this registry; every run is published on `email.command.executed`.

use egui::{Key, KeyboardShortcut, Modifiers};
use toolkit::datatable::{Order, PageMove};
use toolkit::{Command, Icon, Registry};

use crate::state::{COLUMN_DATE, COLUMN_FROM, COLUMN_SUBJECT, Mode, Scope, State};

fn idle(state: &State) -> bool {
    state.compose.is_none() && state.form.is_none() && state.ui.dialog.is_none()
}

/// The current account is connected (its server can be written).
fn ready(state: &State) -> bool {
    idle(state)
        && state
            .current()
            .is_some_and(|v| v.phase == crate::state::Phase::Ready)
}

/// An event can be made: a calendar to put it in.
fn calendared(state: &State) -> bool {
    ready(state)
        && state
            .current()
            .is_some_and(|v| !v.calendars.calendars.is_empty())
}

/// A contact can be made: an address book to put it in.
fn booked(state: &State) -> bool {
    ready(state)
        && state
            .current()
            .is_some_and(|v| !v.contacts.books.is_empty())
}

fn undialogued(state: &State) -> bool {
    state.ui.dialog.is_none()
}

/// A new mail can be written: idle, with an account to send from.
fn composable(state: &State) -> bool {
    idle(state) && state.compose_account().is_some()
}

/// The current account has an email open, in the mail surface.
fn reading(state: &State) -> bool {
    ready(state)
        && state.mode == Mode::Mail
        && state.current().is_some_and(|v| v.mail.open.is_some())
}

/// The current account shows a mailbox's list (it can sort and page).
fn listing(state: &State) -> bool {
    undialogued(state)
        && state.mode == Mode::Mail
        && state
            .current()
            .is_some_and(|v| v.phase == crate::state::Phase::Ready && v.mail.selected.is_some())
}

/// More than one account to step through.
fn several(state: &State) -> bool {
    undialogued(state) && state.accounts.len() > 1
}

/// One View › Sort By command: id, label, column, the Alt chord's key, run.
type SortCommand = (
    &'static str,
    &'static str,
    &'static str,
    Key,
    fn(&mut State),
);

const fn key(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

/// The registry: every command, its menu, its shortcut and its guard.
pub fn registry() -> Registry<State> {
    let mut r = Registry::new();
    r.add(Command {
        id: "file.refresh",
        submenu: None,
        group: 0,
        label: "refresh",
        menu: Some("file"),
        shortcut: key(Modifiers::COMMAND, Key::R),
        icon: Some(Icon::RefreshCw),
        enabled: undialogued,
        run: State::refresh,
    })
    .add(Command {
        id: "file.accounts",
        submenu: None,
        group: 1,
        label: "accounts",
        menu: Some("file"),
        shortcut: None,
        icon: Some(Icon::Settings),
        enabled: undialogued,
        run: State::open_prefs,
    })
    .add(Command {
        id: "file.quit",
        submenu: None,
        group: 2,
        label: "quit",
        menu: Some("file"),
        shortcut: key(Modifiers::COMMAND, Key::Q),
        icon: None,
        enabled: undialogued,
        run: State::quit,
    })
    .add(Command {
        id: "mail.compose",
        submenu: None,
        group: 0,
        label: "compose",
        menu: Some("mail"),
        shortcut: key(Modifiers::COMMAND, Key::N),
        icon: Some(Icon::SquarePen),
        enabled: composable,
        run: State::compose,
    })
    .add(Command {
        id: "mail.reply",
        submenu: None,
        group: 1,
        label: "reply",
        menu: Some("mail"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::R),
        icon: Some(Icon::Reply),
        enabled: reading,
        run: State::reply_open,
    })
    .add(Command {
        id: "mail.reply_all",
        submenu: None,
        group: 1,
        label: "reply-all",
        menu: Some("mail"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::A),
        icon: None,
        enabled: reading,
        run: State::reply_all_open,
    })
    .add(Command {
        id: "mail.forward",
        submenu: None,
        group: 1,
        label: "forward",
        menu: Some("mail"),
        shortcut: key(Modifiers::COMMAND, Key::L),
        icon: None,
        enabled: reading,
        run: State::forward_open,
    })
    .add(Command {
        id: "mail.mark_unread",
        submenu: None,
        group: 2,
        label: "mark-unread",
        menu: Some("mail"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::U),
        icon: None,
        enabled: reading,
        run: State::mark_open_unread,
    })
    .add(Command {
        id: "mail.flag",
        submenu: None,
        group: 2,
        label: "flag",
        menu: Some("mail"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S),
        icon: None,
        enabled: reading,
        run: State::flag_open,
    })
    .add(Command {
        id: "mail.move",
        submenu: None,
        group: 3,
        label: "move",
        menu: Some("mail"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::M),
        icon: Some(Icon::FolderInput),
        enabled: reading,
        run: State::move_open,
    })
    .add(Command {
        id: "mail.delete",
        submenu: None,
        group: 3,
        label: "delete",
        menu: Some("mail"),
        shortcut: key(Modifiers::NONE, Key::Delete),
        icon: Some(Icon::Trash),
        enabled: reading,
        run: State::delete_open,
    })
    .add(Command {
        id: "calendar.new_event",
        submenu: None,
        group: 0,
        label: "new-event",
        menu: Some("calendar"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::E),
        icon: Some(Icon::Calendar),
        enabled: calendared,
        run: State::new_event,
    })
    .add(Command {
        id: "contacts.new_contact",
        submenu: None,
        group: 0,
        label: "new-contact",
        menu: Some("contacts"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::K),
        icon: Some(Icon::Contact),
        enabled: booked,
        run: State::new_contact,
    });
    // View › Theme: any scheme, style, mode and framing, in this window only.
    r.theme_menu("view", choice, set_choice);
    r.add(Command {
        id: "view.mail",
        submenu: None,
        group: 0,
        label: "view-mail",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::Num1),
        icon: Some(Icon::Mail),
        enabled: undialogued,
        run: |state| state.set_mode(Mode::Mail),
    })
    .add(Command {
        id: "view.calendar",
        submenu: None,
        group: 0,
        label: "view-calendar",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::Num2),
        icon: Some(Icon::Calendar),
        enabled: undialogued,
        run: |state| state.set_mode(Mode::Calendar),
    })
    .add(Command {
        id: "view.contacts",
        submenu: None,
        group: 0,
        label: "view-contacts",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::Num3),
        icon: Some(Icon::Contact),
        enabled: undialogued,
        run: |state| state.set_mode(Mode::Contacts),
    })
    .add(Command {
        id: "view.next_account",
        submenu: None,
        group: 1,
        label: "next-account",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::PageDown),
        icon: None,
        enabled: several,
        run: |state| state.step_account(true),
    })
    .add(Command {
        id: "view.previous_account",
        submenu: None,
        group: 1,
        label: "previous-account",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::PageUp),
        icon: None,
        enabled: several,
        run: |state| state.step_account(false),
    });
    // View › Sort By: the list's server-side sort, ticked on the column in
    // force; choosing it again reverses it, as its header does.
    let sorts: [SortCommand; 3] = [
        (
            "view.sort_from",
            "column-from",
            COLUMN_FROM,
            Key::Num1,
            |s| s.sort_mail(COLUMN_FROM),
        ),
        (
            "view.sort_subject",
            "column-subject",
            COLUMN_SUBJECT,
            Key::Num2,
            |s| s.sort_mail(COLUMN_SUBJECT),
        ),
        (
            "view.sort_date",
            "column-date",
            COLUMN_DATE,
            Key::Num3,
            |s| s.sort_mail(COLUMN_DATE),
        ),
    ];
    for (id, label, column, chord, run) in sorts {
        r.add_toggle(
            Command {
                id,
                submenu: Some("sort-by"),
                group: 2,
                label,
                menu: Some("view"),
                shortcut: key(Modifiers::ALT, chord),
                icon: None,
                enabled: listing,
                run,
            },
            move |s: &State| s.mail_sorted_by(column),
        );
    }
    // …then the direction, under a rule.
    let orders: [(&'static str, &'static str, Order, fn(&mut State)); 2] = [
        ("view.sort_ascending", "sort-ascending", Order::Ascending, |s| {
            s.sort_order(Order::Ascending)
        }),
        ("view.sort_descending", "sort-descending", Order::Descending, |s| {
            s.sort_order(Order::Descending)
        }),
    ];
    for (id, label, order, run) in orders {
        r.add_toggle(
            Command {
                id,
                submenu: Some("sort-by"),
                group: 3,
                label,
                menu: Some("view"),
                shortcut: None,
                icon: None,
                enabled: listing,
                run,
            },
            move |s: &State| s.mail_sorted_in(order),
        );
    }
    // The list header's search: focus, clear, and where it looks.
    r.add(Command {
        id: "view.search",
        submenu: None,
        group: 4,
        label: "search-messages",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::F),
        icon: Some(Icon::Search),
        enabled: listing,
        run: State::focus_search,
    })
    .add(Command {
        id: "view.search_clear",
        submenu: None,
        group: 4,
        label: "search-clear",
        menu: Some("view"),
        shortcut: None,
        icon: None,
        enabled: |s| listing(s) && s.current().is_some_and(|v| !v.mail.search.text.is_empty()),
        run: State::clear_search,
    });
    for (id, label, scope, run) in [
        (
            "view.search_folder",
            "search-this-folder",
            Scope::Folder,
            (|s| s.search_scope(Scope::Folder)) as fn(&mut State),
        ),
        (
            "view.search_all",
            "search-all-folders",
            Scope::All,
            |s| s.search_scope(Scope::All),
        ),
    ] {
        r.add_toggle(
            Command {
                id,
                submenu: Some("search-in"),
                group: 4,
                label,
                menu: Some("view"),
                shortcut: None,
                icon: None,
                enabled: listing,
                run,
            },
            move |s: &State| s.searching_in(scope),
        );
    }
    r.add(Command {
        id: "view.previous_page",
        submenu: None,
        group: 3,
        label: "previous-page",
        menu: Some("view"),
        shortcut: key(Modifiers::ALT, Key::PageUp),
        icon: Some(Icon::ChevronLeft),
        enabled: |s| listing(s) && s.can_page_mail(PageMove::Previous),
        run: |s| s.page_mail(PageMove::Previous),
    })
    .add(Command {
        id: "view.next_page",
        submenu: None,
        group: 3,
        label: "next-page",
        menu: Some("view"),
        shortcut: key(Modifiers::ALT, Key::PageDown),
        icon: Some(Icon::ChevronRight),
        enabled: |s| listing(s) && s.can_page_mail(PageMove::Next),
        run: |s| s.page_mail(PageMove::Next),
    })
    // Folder: the tree node's own actions, also its context menu.
    .add(Command {
        id: "folder.new",
        submenu: None,
        group: 0,
        label: "folder-new",
        menu: Some("folder"),
        shortcut: None,
        icon: Some(Icon::FolderPlus),
        enabled: State::folder_targeted,
        run: State::folder_new,
    })
    .add(Command {
        id: "folder.rename",
        submenu: None,
        group: 0,
        label: "folder-rename",
        menu: Some("folder"),
        shortcut: None,
        icon: Some(Icon::Pencil),
        enabled: State::folder_editable,
        run: State::folder_rename,
    })
    .add(Command {
        id: "folder.delete",
        submenu: None,
        group: 0,
        label: "folder-delete",
        menu: Some("folder"),
        shortcut: None,
        icon: Some(Icon::Trash),
        enabled: State::folder_editable,
        run: State::folder_delete,
    })
    .add(Command {
        id: "folder.mark_read",
        submenu: None,
        group: 1,
        label: "folder-mark-read",
        menu: Some("folder"),
        shortcut: None,
        icon: Some(Icon::Check),
        enabled: State::folder_unread,
        run: State::folder_mark_read,
    })
    .add(Command {
        id: "folder.empty",
        submenu: None,
        group: 1,
        label: "folder-empty",
        menu: Some("folder"),
        shortcut: None,
        icon: None,
        enabled: State::folder_emptiable,
        run: State::folder_empty,
    })
    .add(Command {
        id: "folder.refresh",
        submenu: None,
        group: 2,
        label: "folder-refresh",
        menu: Some("folder"),
        shortcut: None,
        icon: Some(Icon::RefreshCw),
        enabled: State::account_targeted,
        run: State::folder_refresh,
    })
    .add(Command {
        id: "account.compose",
        submenu: None,
        group: 3,
        label: "account-compose",
        menu: Some("folder"),
        shortcut: None,
        icon: Some(Icon::SquarePen),
        enabled: State::account_targeted,
        run: State::account_compose,
    })
    .add(Command {
        id: "account.reconnect",
        submenu: None,
        group: 3,
        label: "account-reconnect",
        menu: Some("folder"),
        shortcut: None,
        icon: Some(Icon::Plug),
        enabled: State::account_targeted,
        run: State::account_reconnect,
    })
    .add(Command {
        id: "account.settings",
        submenu: None,
        group: 3,
        label: "account-settings",
        menu: Some("folder"),
        shortcut: None,
        icon: Some(Icon::Settings),
        enabled: State::account_targeted,
        run: State::account_settings,
    })
    .add(Command {
        id: "help.shortcuts",
        submenu: None,
        group: 0,
        label: "shortcuts",
        menu: Some("help"),
        shortcut: None,
        icon: None,
        enabled: undialogued,
        run: |state| state.open_dialog(crate::state::Dialog::Shortcuts),
    })
    .add(Command {
        id: "help.about",
        submenu: None,
        group: 1,
        label: "about",
        menu: Some("help"),
        shortcut: None,
        icon: None,
        enabled: undialogued,
        run: |state| state.open_dialog(crate::state::Dialog::About),
    })
    // The title bar's right-hand controls (no menu of their own).
    .add(Command {
        id: "view.mode",
        submenu: None,
        group: 0,
        label: "toggle-mode",
        menu: None,
        shortcut: None,
        icon: None,
        enabled: toolkit::command::always,
        run: State::toggle_mode,
    });
    r
}

/// The window's theme choice, as the Theme menu reads it.
pub fn choice(state: &State) -> toolkit::theme_menu::Choice {
    toolkit::theme_menu::Choice {
        scheme: state.ui.theme.scheme,
        style: state.ui.theme.style,
        mode: state.ui.theme.mode,
        decorations: state.ui.theme.decorations,
        captions: state.ui.theme.captions,
    }
}

/// Store the Theme menu's choice as the window's.
fn set_choice(state: &mut State, choice: toolkit::theme_menu::Choice) {
    state.ui.theme.scheme = choice.scheme;
    state.ui.theme.style = choice.style;
    state.ui.theme.mode = choice.mode;
    state.ui.theme.decorations = choice.decorations;
    state.ui.theme.captions = choice.captions;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every label is in the catalogue.
    #[test]
    fn every_label_is_in_the_catalogue() {
        let strings = crate::strings();
        let known = |key: &str| toolkit::command::label_text(&strings, key) != key;
        for command in registry().iter() {
            assert!(
                known(command.label),
                "{} label {:?}",
                command.id,
                command.label
            );
        }
    }

    /// No two commands share a chord.
    #[test]
    fn no_shortcut_is_taken_twice() {
        let r = registry();
        let mut seen = std::collections::BTreeMap::new();
        for command in r.iter() {
            for chord in r.shortcuts_of(command.id) {
                let name = format!("{chord:?}");
                if let Some(other) = seen.insert(name.clone(), command.id) {
                    panic!("{name} is both {other} and {}", command.id);
                }
            }
        }
    }
}
