// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The message list's and the folder tree's verbs: the toolkit table set
//! mounted as `email.table.*`, the search, the folder-wide writes and the
//! open form's answers. Each takes the path the window's own control does.

use serde_json::{Value, json};
use toolkit::datatable::verbs::{self as table_verbs, Refusal};

use super::{Bad, Checked, Verb, Write, connected, flag, invalid, required, text, verb};
use crate::state::{FormField, Scope, State, code};
use crate::view::UiEvent;

/// The verbs here, in manifest order; the table set follows them in
/// [`super::describe`].
pub const VERBS: &[Verb] = &[
    verb(
        "email.search.set",
        false,
        r#"{"account":"optional account id","text":"string","scope":"optional folder|all (default the list's)"}"#,
        "Search the list as its search field does: the text, matched on the server, in the selected folder or all folders",
    ),
    verb(
        "email.search.clear",
        false,
        r#"{"account":"optional account id"}"#,
        "Clear the list's search: the folder shows again",
    ),
    verb(
        "email.mailbox.mark_read",
        false,
        r#"{"account":"optional account id","id":"mailbox id"}"#,
        "Mark every unread email in a mailbox read, on the server, 500 at a time",
    ),
    verb(
        "email.mailbox.empty",
        false,
        r#"{"account":"optional account id","id":"mailbox id (a Trash or Junk)"}"#,
        "Destroy every email in a Trash or Junk mailbox, 500 at a time",
    ),
    verb(
        "email.form.field",
        false,
        r#"{"name":"title|start|end|location|name|email|phone|organization|folder_name","value":"string"}"#,
        "Set a field of the open form (a folder dialog's name is folder_name)",
    ),
    verb(
        "email.form.submit",
        false,
        r#"{"choice":"optional mailbox id (a move)"}"#,
        "Answer the open form as its confirming button does (Create, Rename, Delete, Empty, Save…)",
    ),
    verb(
        "email.form.cancel",
        false,
        "{}",
        "Dismiss the open form, as Cancel does",
    ),
];

/// Run one of this module's verbs (or a table verb); `None` when `verb` is
/// not one of them.
pub fn dispatch(state: &mut State, id: u64, verb: &str, args: &Value) -> Option<Checked<()>> {
    if let Some(table) = verb
        .strip_prefix("email.")
        .filter(|v| table_verbs::VERBS.contains(v))
    {
        return Some(table_verb(state, id, table, args));
    }
    Some(match verb {
        "email.search.set" => search_set(state, id, args),
        "email.search.clear" => search_clear(state, id, args),
        "email.mailbox.mark_read" => mailbox_write(state, id, args, false),
        "email.mailbox.empty" => mailbox_write(state, id, args, true),
        "email.form.field" => form_field(state, id, args),
        "email.form.submit" => form_submit(state, id, args),
        "email.form.cancel" => {
            let open = state.form.is_some();
            state.apply_ui(UiEvent::FormCancel);
            state.reply(id, 0, json!({ "cancelled": open }));
            Ok(())
        }
        _ => return None,
    })
}

fn search_set(state: &mut State, id: u64, args: &Value) -> Checked<()> {
    let account = connected(state, args)?;
    let text = required(args, "text")?;
    let scope = scope(args)?;
    state.set_search(&account, Some(text), scope);
    state.reply(id, 0, state.table_state(&account));
    Ok(())
}

fn search_clear(state: &mut State, id: u64, args: &Value) -> Checked<()> {
    let account = connected(state, args)?;
    state.set_search(&account, Some(String::new()), None);
    state.reply(id, 0, state.table_state(&account));
    Ok(())
}

/// `email.mailbox.mark_read`, or `.empty` (`empty`: only a Trash or Junk).
fn mailbox_write(state: &mut State, id: u64, args: &Value, empty: bool) -> Checked<()> {
    let account = connected(state, args)?;
    let mailbox = required(args, "id")?;
    let found = state
        .account(&account)
        .and_then(|v| v.mail.mailboxes.iter().find(|m| m.id == mailbox).cloned())
        .ok_or_else(|| Bad(code::NOT_FOUND, format!("no mailbox {mailbox}")))?;
    let write = if empty {
        if !matches!(found.role.as_deref(), Some("trash" | "junk" | "spam")) {
            return Err(invalid("only a Trash or Junk mailbox is emptied"));
        }
        Write::EmptyMailbox { id: mailbox }
    } else {
        Write::MarkMailboxRead { id: mailbox }
    };
    state.mailbox_write(&account, Some(id), write);
    Ok(())
}

fn form_field(state: &mut State, id: u64, args: &Value) -> Checked<()> {
    if state.form.is_none() {
        return Err(Bad(code::NOT_FOUND, "no form is open".into()));
    }
    let field = match required(args, "name")?.as_str() {
        "title" => FormField::Title,
        "start" => FormField::Start,
        "end" => FormField::End,
        "location" => FormField::Location,
        "name" => FormField::Name,
        "email" => FormField::Email,
        "phone" => FormField::Phone,
        "organization" => FormField::Organization,
        "folder_name" => FormField::FolderName,
        _ => return Err(invalid("no such form field")),
    };
    let value = text(args, "value")?.unwrap_or_default();
    state.apply_ui(UiEvent::FormField(field, value));
    state.reply(id, 0, json!({ "form": state.info()["form"].clone() }));
    Ok(())
}

fn form_submit(state: &mut State, id: u64, args: &Value) -> Checked<()> {
    if state.form.is_none() {
        return Err(Bad(code::NOT_FOUND, "no form is open".into()));
    }
    state.apply_ui(UiEvent::FormSubmit(text(args, "choice")?));
    state.reply(id, 0, json!({ "form": state.info()["form"].clone() }));
    Ok(())
}

/// `email.table.*` on the account's message list.
fn table_verb(state: &mut State, id: u64, verb: &str, args: &Value) -> Checked<()> {
    let account = connected(state, args)?;
    if verb == "table.page_size" {
        return Err(Bad(
            code::REFUSED,
            "the page size is the email.mail.page_size setting (settings.apply)".into(),
        ));
    }
    // The verb's own arguments, without the account.
    let mut own = args.clone();
    if let Some(map) = own.as_object_mut() {
        map.remove("account");
    }
    match state.table_verb(&account, verb, &own) {
        Ok(answer) => {
            state.reply(id, 0, answer);
            Ok(())
        }
        Err(Refusal::Invalid(message)) => Err(invalid(message)),
        Err(Refusal::NotFound(message)) => Err(Bad(code::NOT_FOUND, message)),
    }
}

/// `scope`: `folder` or `all`, or absent.
fn scope(args: &Value) -> Checked<Option<Scope>> {
    match text(args, "scope")? {
        None => Ok(None),
        Some(name) => Scope::from_name(&name)
            .map(Some)
            .ok_or_else(|| invalid("scope must be folder or all")),
    }
}

/// `email.search`'s `show`: drive the list's search rather than only
/// answer a query.
pub fn shows(args: &Value) -> Checked<bool> {
    Ok(flag(args, "show")?.unwrap_or(false))
}
