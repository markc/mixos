// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! email's commands. The menu bar, the shortcuts and `email.execute` come
//! from this registry; every run is published on `email.action.executed`.

use egui::{Key, KeyboardShortcut, Modifiers};
use toolkit::action::Scope;
use toolkit::{Action, Icon, Registry};

use crate::state::{Mode, State};

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

/// The current account has an email open, in the mail surface, and no
/// other one is loading in its place: a row just clicked (or right-clicked
/// for its menu) is not answered by the email that was open before it.
fn reading(state: &State) -> bool {
    ready(state)
        && state.mode == Mode::Mail
        && state
            .current()
            .is_some_and(|v| v.mail.open.is_some() && !v.opening_another())
}

/// The message list has a target for the message actions (the selection,
/// else the keyboard row), and no "Select all N" fetch is loading.
fn targeted(state: &State) -> bool {
    ready(state)
        && state.mode == Mode::Mail
        && state
            .current()
            .is_some_and(|v| v.mail.fetching.is_none() && v.mail.target().is_some())
}

/// The open email has a text body the reader can show as Markdown or as
/// its source.
fn readable(state: &State) -> bool {
    reading(state)
        && state
            .current()
            .and_then(|v| v.mail.open.as_ref())
            .is_some_and(crate::reader::has_text)
}

/// The target can go to Junk: the mailbox is not Junk, and the account
/// has a Junk mailbox.
fn junkable(state: &State) -> bool {
    targeted(state)
        && state.current().is_some_and(|v| {
            !crate::state::in_junk(v) && crate::state::junk_target(v, true).is_some()
        })
}

/// The mailbox is Junk and the target can go back to the inbox.
fn unjunkable(state: &State) -> bool {
    targeted(state)
        && state.current().is_some_and(|v| {
            crate::state::in_junk(v) && crate::state::junk_target(v, false).is_some()
        })
}

/// The displayed email has attachments, even while another email loads.
fn attached(state: &State) -> bool {
    idle(state)
        && state.mode == Mode::Mail
        && state
            .current()
            .and_then(|v| v.mail.open.as_ref())
            .and_then(|e| e.attachments.as_ref())
            .is_some_and(|list| !list.is_empty())
}

/// More than one account to step through.
fn several(state: &State) -> bool {
    undialogued(state) && state.accounts.len() > 1
}

const fn key(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

fn compose_attachable(state: &State) -> bool {
    state.compose.as_ref().is_some_and(|compose| !compose.busy)
}

fn compose_attached(state: &State) -> bool {
    compose_attachable(state)
        && state
            .compose
            .as_ref()
            .is_some_and(|c| !c.draft.attachments.is_empty())
}

/// The registry: every command, its menu, its shortcut and its guard.
pub fn registry() -> Registry<State> {
    let mut r = Registry::new();
    r.dialog_menu("compose-attachments-menu");
    r.add(Action {
        id: "compose.attach",
        label: "compose-attach",
        menu: Some("compose-attachments-menu"),
        submenu: None,
        group: 0,
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::A),
        icon: Some(Icon::Paperclip),
        enabled: compose_attachable,
        run: State::attach_files,
    })
    .add(Action {
        id: "compose.attachments.clear",
        label: "compose-attachments-clear",
        menu: Some("compose-attachments-menu"),
        submenu: None,
        group: 1,
        shortcut: None,
        icon: None,
        enabled: compose_attached,
        run: State::clear_compose_attachments,
    });
    r.add(toolkit::macros::command(
        |s: &State| s.ui.macros_open || undialogued(s),
        |s: &mut State| {
            s.ui.macros_open = !s.ui.macros_open;
        },
    ));
    r.add(Action {
        id: "file.refresh",
        submenu: None,
        group: 0,
        label: "refresh",
        menu: Some("file"),
        // Ctrl+T, Thunderbird's Get Messages: Ctrl+R is Reply, and
        // function keys are the person's to bind, never a default.
        shortcut: key(Modifiers::COMMAND, Key::T),
        icon: Some(Icon::RefreshCw),
        enabled: undialogued,
        run: State::refresh,
    })
    .add(Action {
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
    .add(Action {
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
    .add(Action {
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
    .add(Action {
        id: "mail.send",
        submenu: None,
        group: 0,
        label: "send",
        menu: None,
        shortcut: None,
        icon: None,
        enabled: State::compose_sendable,
        run: State::send_compose,
    })
    // Message: what can be done to the open email, Thunderbird's keys.
    .add(Action {
        id: "mail.reply",
        submenu: None,
        group: 0,
        label: "reply",
        menu: Some("message"),
        shortcut: key(Modifiers::COMMAND, Key::R),
        icon: Some(Icon::Reply),
        enabled: reading,
        run: State::reply_open,
    })
    .add(Action {
        id: "mail.reply_all",
        submenu: None,
        group: 0,
        label: "reply-all",
        menu: Some("message"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::R),
        icon: Some(Icon::ReplyAll),
        enabled: reading,
        run: State::reply_all_open,
    })
    .add(Action {
        id: "mail.forward",
        submenu: None,
        group: 0,
        label: "forward",
        menu: Some("message"),
        shortcut: key(Modifiers::COMMAND, Key::L),
        icon: Some(Icon::Forward),
        enabled: reading,
        run: State::forward_open,
    })
    // Thunderbird's Edit as New Message, and its key.
    .add(Action {
        id: "mail.send_as_new",
        submenu: None,
        group: 0,
        label: "send-as-new",
        menu: Some("message"),
        shortcut: key(Modifiers::COMMAND, Key::E),
        icon: Some(Icon::Redo2),
        enabled: reading,
        run: State::send_as_new_open,
    })
    .add(Action {
        id: "mail.reply_quoted",
        submenu: None,
        group: 1,
        label: "reply-quoted",
        menu: Some("message"),
        shortcut: None,
        icon: None,
        enabled: reading,
        run: State::reply_open_quoted,
    })
    .add(Action {
        id: "mail.reply_unquoted",
        submenu: None,
        group: 1,
        label: "reply-unquoted",
        menu: Some("message"),
        shortcut: None,
        icon: None,
        enabled: reading,
        run: State::reply_open_unquoted,
    })
    .add(Action {
        id: "mail.reply_all_quoted",
        submenu: None,
        group: 1,
        label: "reply-all-quoted",
        menu: Some("message"),
        shortcut: None,
        icon: None,
        enabled: reading,
        run: State::reply_all_open_quoted,
    })
    .add(Action {
        id: "mail.reply_all_unquoted",
        submenu: None,
        group: 1,
        label: "reply-all-unquoted",
        menu: Some("message"),
        shortcut: None,
        icon: None,
        enabled: reading,
        run: State::reply_all_open_unquoted,
    })
    .add(Action {
        id: "mail.flag",
        submenu: None,
        group: 2,
        label: "flag",
        menu: Some("message"),
        shortcut: key(Modifiers::NONE, Key::S),
        icon: Some(Icon::Star),
        enabled: targeted,
        run: State::flag_target,
    })
    .add(Action {
        id: "mail.mark_unread",
        submenu: None,
        group: 2,
        label: "mark-unread",
        menu: Some("message"),
        shortcut: key(Modifiers::NONE, Key::M),
        icon: Some(Icon::MailOpen),
        enabled: targeted,
        run: State::toggle_target_read,
    })
    .add(Action {
        id: "mail.junk",
        submenu: None,
        group: 3,
        label: "junk",
        menu: Some("message"),
        shortcut: key(Modifiers::NONE, Key::J),
        icon: Some(Icon::ShieldAlert),
        enabled: junkable,
        run: State::mark_junk,
    })
    .add(Action {
        id: "mail.not_junk",
        submenu: None,
        group: 3,
        label: "not-junk",
        menu: Some("message"),
        shortcut: key(Modifiers::SHIFT, Key::J),
        icon: None,
        enabled: unjunkable,
        run: State::mark_not_junk,
    })
    .add(Action {
        id: "mail.move",
        submenu: None,
        group: 4,
        label: "move",
        menu: Some("message"),
        shortcut: key(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::M),
        icon: Some(Icon::FolderInput),
        enabled: targeted,
        run: State::move_target,
    })
    .add(Action {
        id: "mail.delete",
        submenu: None,
        group: 4,
        label: "delete",
        menu: Some("message"),
        shortcut: key(Modifiers::NONE, Key::Delete),
        icon: Some(Icon::Trash),
        enabled: targeted,
        run: State::delete_target,
    })
    .add(Action {
        id: "mail.print",
        submenu: None,
        group: 5,
        label: "print",
        menu: Some("message"),
        shortcut: key(Modifiers::COMMAND, Key::P),
        icon: Some(Icon::Printer),
        enabled: reading,
        run: State::print_open,
    })
    .add_toggle(
        Action {
            id: "mail.attachments",
            submenu: None,
            group: 5,
            label: "attachments",
            menu: Some("message"),
            shortcut: None,
            icon: Some(Icon::Paperclip),
            enabled: attached,
            run: State::toggle_attachments,
        },
        |state| state.ui.attachments_expanded,
    )
    .add(Action {
        id: "mail.save_attachments",
        submenu: None,
        group: 5,
        label: "save-attachments",
        menu: Some("message"),
        shortcut: None,
        icon: Some(Icon::Download),
        enabled: |state| reading(state) && attached(state),
        run: State::save_all_attachments,
    })
    .add(Action {
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
    .add(Action {
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
    r.add(Action {
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
    .add(Action {
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
    .add(Action {
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
    .add(Action {
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
    .add(Action {
        id: "view.previous_account",
        submenu: None,
        group: 1,
        label: "previous-account",
        menu: Some("view"),
        shortcut: key(Modifiers::COMMAND, Key::PageUp),
        icon: None,
        enabled: several,
        run: |state| state.step_account(false),
    })
    .add_toggle(
        Action {
            id: "view.reader.markdown",
            submenu: Some("message"),
            group: 2,
            label: "reader-markdown",
            menu: Some("view"),
            // No chord: Ctrl+U is the text fields' delete-to-line-start.
            shortcut: None,
            icon: Some(Icon::FileCode),
            enabled: readable,
            run: State::toggle_reader,
        },
        markdown_shown,
    )
    .add_toggle(
        Action {
            id: "view.attachments_inline",
            submenu: None,
            group: 2,
            label: "attachments-inline",
            menu: Some("view"),
            shortcut: None,
            icon: None,
            enabled: undialogued,
            run: State::toggle_attachments_inline,
        },
        |state| state.ui.attachments_inline,
    )
    .add(Action {
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
    .add(Action {
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
    .add(Action {
        id: "view.mode",
        submenu: None,
        group: 0,
        label: "toggle-mode",
        menu: None,
        shortcut: None,
        icon: None,
        enabled: toolkit::action::always,
        run: State::toggle_mode,
    });
    // Bare keys and Delete belong to a text field while one is typed in.
    for id in [
        "mail.flag",
        "mail.mark_unread",
        "mail.junk",
        "mail.not_junk",
        "mail.delete",
    ] {
        r.scope(id, Scope::Browsing);
    }
    // Labels that say what the command will do to the target.
    r.relabel("mail.flag", |state| {
        if target_all(state, jmap::Email::is_flagged) {
            "unflag"
        } else {
            "flag"
        }
    })
    .relabel("mail.junk", |state| {
        // Disabled for want of a Junk mailbox: the label says so.
        if state
            .current()
            .is_some_and(|v| crate::state::junk_target(v, true).is_none())
        {
            "junk-none"
        } else {
            "junk"
        }
    })
    .relabel("mail.mark_unread", |state| {
        let some_unread = state
            .current()
            .is_some_and(|v| v.mail.target_rows().iter().any(|e| !e.is_read()));
        if some_unread {
            "mark-read"
        } else {
            "mark-unread"
        }
    });
    r
}

/// Registry chords wait while the composer body has the keyboard.
pub fn shortcuts(
    ctx: &egui::Context,
    state: &State,
    registry: &Registry<State>,
) -> Vec<&'static str> {
    if state.attachment_chooser.is_some() {
        Vec::new()
    } else if state.compose.is_some() && crate::view::compose::body_focused(ctx) {
        if compose_attachable(state)
            && registry
                .shortcuts_of("compose.attach")
                .iter()
                .any(|shortcut| ctx.input_mut(|input| input.consume_shortcut(shortcut)))
        {
            vec!["compose.attach"]
        } else {
            Vec::new()
        }
    } else {
        registry.shortcuts(ctx, state)
    }
}

/// Whether every shown row of the current account's target passes `test`
/// (false with none).
fn target_all(state: &State, test: fn(&jmap::Email) -> bool) -> bool {
    state.current().is_some_and(|v| {
        let rows = v.mail.target_rows();
        !rows.is_empty() && rows.into_iter().all(test)
    })
}

/// View › Message › Render Markdown's tick: the open email is shown as
/// Markdown.
fn markdown_shown(state: &State) -> bool {
    state
        .current()
        .and_then(|v| Some((v, v.mail.open.as_ref()?)))
        .is_some_and(|(v, email)| {
            let reader = &v.mail.reader;
            let format = if reader.is_for(email) {
                reader.format()
            } else {
                crate::reader::detect(email)
            };
            format == crate::reader::Format::Markdown
        })
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

    #[test]
    fn attachments_are_dialog_only_and_guards_follow_the_compose() {
        let mut state = crate::state::tests::connected_many(&["a"]);
        let registry = registry();
        let attach = registry.get("compose.attach").unwrap();
        let clear = registry.get("compose.attachments.clear").unwrap();
        assert!(!(attach.enabled)(&state));
        assert!(!(clear.enabled)(&state));
        state.compose();
        assert!((attach.enabled)(&state));
        assert!(!(clear.enabled)(&state));
        state
            .compose
            .as_mut()
            .unwrap()
            .draft
            .attachments
            .push("/home/user/file".into());
        assert!((clear.enabled)(&state));
        state.compose.as_mut().unwrap().busy = true;
        assert!(!(attach.enabled)(&state));
        assert!(!(clear.enabled)(&state));
        let model = registry.model(&egui::Context::default(), &state, &crate::strings());
        assert!(
            !model
                .iter()
                .any(|menu| menu.title == crate::label("compose-attachments-menu"))
        );
    }

    /// Every label is in the catalogue.
    #[test]
    fn every_label_is_in_the_catalogue() {
        let strings = crate::strings();
        let known = |key: &str| toolkit::action::label_text(&strings, key) != key;
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
