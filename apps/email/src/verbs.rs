// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The `email` Bus port, schema `email.v1`: every verb, its arguments, and
//! the one place a Bus command becomes state changes, work and replies.
//!
//! Success is rc 0; a refusal is rc 10 with `{error_code, message}`
//! ([`crate::state::code`]). `account` defaults to the selected account
//! everywhere; an id no account has is `UNKNOWN_ACCOUNT`, an email,
//! mailbox, event or contact id the account's server does not hold is
//! `NOT_FOUND`.
//!
//! - **Queries** that need the server (`email.list`, `email.get`,
//!   `email.events`, …) run on the account's client on the effects runtime
//!   and answer when it does, carrying the request id through
//!   [`Work::Query`]; the UI thread never waits.
//! - **Intents** on the window (`email.mailbox.select`, `email.mode`,
//!   `email.compose`, …) take the same state paths as the window does.
//! - **Data writes** (`email.mark`, `email.move`, `email.event.create`, …)
//!   run on the client ([`Work::Write`]), refresh the view they change, and
//!   answer with what the server made (a created id).
//! - **Accounts** (`email.account.set`, `.remove`, `.default`) are written
//!   to settingsd (fenced `settings.apply`) and answered once it accepts;
//!   a password goes only to the app's secrets file
//!   (`email.account.password.set`) and is never in any answer.
//!
//! `email.execute` runs any registry command; every executed command is
//! published on `email.command.executed`. The window also serves the
//! toolkit's drive verbs (`email.ui.*`, `email.window*`).

use jmap::{Client, Composed, ContactDraft, ContactPatch, EventDraft, EventPatch};
use serde::Serialize;
use serde_json::{Value, json};
use settings::email::AccountEntry;
use toolkit::command::{CommandError, Origin};
use toolkit::{Registry, Strings, drive};

use crate::state::{Mode, Phase, State, code};
use crate::writer::{AccountPatch, Change};
use crate::{APP, APP_ID};

pub const SCHEMA: &str = "email.v1";

mod list;

/// One verb as `HELP` and `app.describe` list it.
pub struct Verb {
    pub name: &'static str,
    pub read_only: bool,
    /// The arguments, as a JSON object of name to description.
    pub args: &'static str,
    pub description: &'static str,
}

const fn verb(
    name: &'static str,
    read_only: bool,
    args: &'static str,
    description: &'static str,
) -> Verb {
    Verb {
        name,
        read_only,
        args,
        description,
    }
}

/// Every verb the app answers itself, in manifest order. The window's drive
/// verbs follow them in [`describe`].
pub const VERBS: &[Verb] = &[
    verb("HELP", true, "{}", "List every verb"),
    verb(
        "app.describe",
        true,
        "{}",
        "Describe the app and every verb with its arguments",
    ),
    verb("email.ping", true, "{}", "Probe Email"),
    verb(
        "email.info",
        true,
        "{}",
        "The whole window state: accounts (no secret), selection, mode, lists shown (the message list's sort, page and selection under mail.table), compose, dialogs and UI shape",
    ),
    verb(
        "email.commands",
        true,
        "{}",
        "The app's commands with labels, shortcuts and enablement",
    ),
    verb(
        "email.accounts",
        true,
        "{}",
        "The accounts in tree order: settings, whether a password is held (has_password), connection state and error; never a password",
    ),
    verb(
        "email.mailboxes",
        true,
        r#"{"account":"optional account id"}"#,
        "The account's mailboxes from its server, with roles, parents and counts",
    ),
    verb(
        "email.list",
        true,
        r#"{"account":"optional account id","mailbox":"optional mailbox id (default the selected one)","sort":"optional received|subject|from (default received)","order":"optional asc|desc (default desc for received, asc otherwise)","page":"optional integer page, 0-based, of limit emails (or offset)","offset":"optional integer (or page)","limit":"optional integer 1..500 (default the page size)","query":"optional text to match"}"#,
        "One page of a mailbox in the order asked for (newest first by default), with the total count, the page and the sort",
    ),
    verb(
        "email.search",
        false,
        r#"{"account":"optional account id","text":"string","mailbox":"optional mailbox id","limit":"optional integer 1..500","show":"optional bool: show the result in the window's list (its search field and scope) rather than only answer"}"#,
        "Emails matching text, in one mailbox or all; with show, the window's list searches too",
    ),
    verb(
        "email.get",
        true,
        r#"{"account":"optional account id","id":"email id"}"#,
        "One email's headers and text body",
    ),
    verb(
        "email.calendars",
        true,
        r#"{"account":"optional account id"}"#,
        "The account's calendars",
    ),
    verb(
        "email.events",
        true,
        r#"{"account":"optional account id","calendar":"optional calendar id (default the selected one)","from":"optional RFC 3339 instant","to":"optional RFC 3339 instant"}"#,
        "A calendar's events in an optional window, newest start first",
    ),
    verb(
        "email.event.get",
        true,
        r#"{"account":"optional account id","id":"event id"}"#,
        "One event with its JSCalendar data",
    ),
    verb(
        "email.books",
        true,
        r#"{"account":"optional account id"}"#,
        "The account's address books",
    ),
    verb(
        "email.contacts",
        true,
        r#"{"account":"optional account id","book":"optional address book id (default the selected one)","filter":"optional text over name, email and company"}"#,
        "An address book's contacts",
    ),
    verb(
        "email.contact.get",
        true,
        r#"{"account":"optional account id","id":"contact id"}"#,
        "One contact with its JSContact data",
    ),
    verb("email.show", false, "{}", "Restore and focus the window"),
    verb("email.quit", false, "{}", "Close the window"),
    verb(
        "email.execute",
        false,
        r#"{"id":"command id from email.commands"}"#,
        "Run one app command by id, as its menu item or shortcut would",
    ),
    verb(
        "email.mode",
        false,
        r#"{"name":"mail, calendar or contacts"}"#,
        "Show a surface",
    ),
    verb(
        "email.refresh",
        false,
        r#"{"account":"optional account id"}"#,
        "Fetch the account's mailboxes, calendars and address books again (a failed account reconnects)",
    ),
    verb(
        "email.account.set",
        false,
        r#"{"id":"optional account id (absent: a new account)","name":"string","url":"https://… JMAP base","email":"login address","password":"optional string, kept only in the app's secrets file","ca_path":"optional absolute PEM path or null","enabled":"optional bool","signature":"optional plain text"}"#,
        "Create or update an account in settingsd; answers {id} once settingsd accepts",
    ),
    verb(
        "email.account.remove",
        false,
        r#"{"id":"account id"}"#,
        "Remove an account from settingsd (and its password); the default resets with it",
    ),
    verb(
        "email.account.password.set",
        false,
        r#"{"id":"account id (for a forget, any account id, one settingsd no longer holds included)","password":"string; empty forgets it"}"#,
        "Keep an account's password in the app's secrets file and reconnect; write-only. An empty password forgets it and answers {forgotten: bool}",
    ),
    verb(
        "email.account.connect",
        false,
        r#"{"id":"account id"}"#,
        "Connect an account again",
    ),
    verb(
        "email.account.select",
        false,
        r#"{"id":"account id"}"#,
        "Show an account",
    ),
    verb(
        "email.account.default",
        false,
        r#"{"id":"account id, or null for the first enabled one"}"#,
        "Make an account the compose default in settingsd",
    ),
    verb(
        "email.mailbox.select",
        false,
        r#"{"account":"optional account id","id":"mailbox id"}"#,
        "Show a mailbox",
    ),
    verb(
        "email.mailbox.create",
        false,
        r#"{"account":"optional account id","name":"string","parent":"optional mailbox id"}"#,
        "Create a mailbox; answers {id}",
    ),
    verb(
        "email.mailbox.rename",
        false,
        r#"{"account":"optional account id","id":"mailbox id","name":"string"}"#,
        "Rename a mailbox",
    ),
    verb(
        "email.mailbox.delete",
        false,
        r#"{"account":"optional account id","id":"mailbox id"}"#,
        "Delete an empty mailbox",
    ),
    verb(
        "email.open",
        false,
        r#"{"account":"optional account id","id":"email id"}"#,
        "Open an email in the reader; answers once it is loaded",
    ),
    verb(
        "email.mark",
        false,
        r#"{"account":"optional account id","ids":"email id or list of ids","read":"optional bool","flagged":"optional bool"}"#,
        "Set or clear read and flagged",
    ),
    verb(
        "email.move",
        false,
        r#"{"account":"optional account id","ids":"email id or list of ids","mailbox":"mailbox id"}"#,
        "Move emails to a mailbox",
    ),
    verb(
        "email.delete",
        false,
        r#"{"account":"optional account id","ids":"email id or list of ids","destroy":"optional bool (default false: to Trash)"}"#,
        "Move emails to Trash, or destroy them",
    ),
    verb(
        "email.send",
        false,
        r#"{"account":"optional account id","to":"addresses","cc":"optional addresses","bcc":"optional addresses","subject":"string","body":"string","in_reply_to":"optional Message-ID"}"#,
        "Send an email without the dialog",
    ),
    verb(
        "email.compose",
        false,
        r#"{"account":"optional account id","to":"optional addresses","cc":"optional addresses","subject":"optional string","body":"optional string"}"#,
        "Open the compose dialog, filled in",
    ),
    verb(
        "email.reply",
        false,
        r#"{"account":"optional account id","id":"email id","all":"optional bool"}"#,
        "Open the compose dialog replying to an email",
    ),
    verb(
        "email.forward",
        false,
        r#"{"account":"optional account id","id":"email id","to":"optional addresses"}"#,
        "Open the compose dialog forwarding an email",
    ),
    verb(
        "email.calendar.select",
        false,
        r#"{"account":"optional account id","id":"calendar id"}"#,
        "Show a calendar",
    ),
    verb(
        "email.event.create",
        false,
        r#"{"account":"optional account id","calendar":"optional calendar id (default the selected one)","title":"string","start":"RFC 3339 instant with an offset","end":"RFC 3339 instant with an offset (or duration)","duration":"ISO 8601 duration from start (or end)","time_zone":"optional IANA zone","all_day":"optional bool","location":"optional string","description":"optional string"}"#,
        "Create an event in a calendar (its id a UUID, as maild requires); the engine computes the duration; answers {id}",
    ),
    verb(
        "email.event.update",
        false,
        r#"{"account":"optional account id","id":"event id","title":"optional string","start":"optional RFC 3339 instant","end":"optional RFC 3339 instant","duration":"optional ISO 8601 duration (from start)","time_zone":"optional string","all_day":"optional bool","location":"optional string","description":"optional string"}"#,
        "Change an event's fields; maild keeps only title, start and end on an update and ignores the rest",
    ),
    verb(
        "email.event.delete",
        false,
        r#"{"account":"optional account id","id":"event id"}"#,
        "Delete an event",
    ),
    verb(
        "email.book.select",
        false,
        r#"{"account":"optional account id","id":"address book id"}"#,
        "Show an address book",
    ),
    verb(
        "email.contact.create",
        false,
        r#"{"account":"optional account id","book":"optional address book id (default the selected one)","name":"string","emails":"optional list of addresses","phones":"optional list of numbers","org":"optional string","notes":"optional string"}"#,
        "Create a contact; answers {id}",
    ),
    verb(
        "email.contact.update",
        false,
        r#"{"account":"optional account id","id":"contact id","name":"optional string","emails":"optional list","phones":"optional list","org":"optional string","notes":"optional string"}"#,
        "Change a contact's fields",
    ),
    verb(
        "email.contact.delete",
        false,
        r#"{"account":"optional account id","id":"contact id"}"#,
        "Delete a contact",
    ),
];

/// The whole Bus surface: the app's verbs and the window's drive verbs.
pub fn describe() -> Value {
    let mut verbs: Vec<Value> = VERBS
        .iter()
        .map(|v| {
            let args: Value = serde_json::from_str(v.args).unwrap_or_else(|_| json!({}));
            let mut entry =
                json!({"name": v.name, "description": v.description, "read_only": v.read_only});
            if args.as_object().is_some_and(|a| !a.is_empty()) {
                entry["args"] = args;
            }
            entry
        })
        .collect();
    // The list's and folders' verbs, then the toolkit's table set.
    verbs.extend(list::VERBS.iter().map(|v| {
        json!({"name": v.name, "description": v.description, "read_only": v.read_only,
            "args": serde_json::from_str::<Value>(v.args).unwrap_or_else(|_| json!({}))})
    }));
    verbs.extend(toolkit::datatable::verbs::describe(APP));
    verbs.extend(drive::describe(APP));
    json!({"schema": SCHEMA, "app_id": APP_ID, "version": env!("CARGO_PKG_VERSION"), "verbs": verbs})
}

/// What only the shell can do for a command.
#[derive(Clone, Debug, PartialEq)]
pub enum Forward {
    /// A drive verb (`email.ui.*`, `email.window*`), without the prefix.
    Drive { id: u64, verb: String, args: Value },
    /// Restore and focus the window, then answer.
    Show { id: u64 },
}

/// A query the account's server answers.
#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    Mailboxes,
    List {
        mailbox: Option<String>,
        text: Option<String>,
        offset: u64,
        limit: u64,
        sort: jmap::EmailSort,
    },
    Get {
        id: String,
    },
    Calendars,
    Events {
        calendar: String,
        from: Option<String>,
        to: Option<String>,
    },
    Event {
        id: String,
    },
    Books,
    Contacts {
        book: String,
        filter: String,
    },
    Contact {
        id: String,
    },
}

/// What happens with a query's answer besides the reply.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// Answer it.
    Answer,
    /// Open a compose replying to the email fetched (`true`: to all).
    Reply(bool),
    /// Open a compose forwarding the email fetched, to these addresses.
    Forward(String),
}

/// A change on the account's server.
#[derive(Clone, Debug, PartialEq)]
pub enum Write {
    Keywords {
        ids: Vec<String>,
        read: Option<bool>,
        flagged: Option<bool>,
    },
    Move {
        ids: Vec<String>,
        mailbox: String,
    },
    Delete {
        ids: Vec<String>,
        destroy: bool,
    },
    CreateMailbox {
        name: String,
        parent: Option<String>,
    },
    RenameMailbox {
        id: String,
        name: String,
    },
    DeleteMailbox {
        id: String,
    },
    /// Move a mailbox's emails to the Trash, then delete it.
    TrashMailbox {
        id: String,
    },
    /// Mark every unread email in a mailbox read.
    MarkMailboxRead {
        id: String,
    },
    /// Destroy every email in a mailbox (Trash, Junk).
    EmptyMailbox {
        id: String,
    },
    Send {
        draft: Composed,
    },
    CreateEvent {
        calendar: String,
        draft: EventDraft,
    },
    UpdateEvent {
        id: String,
        patch: EventPatch,
    },
    DeleteEvent {
        id: String,
    },
    CreateContact {
        book: String,
        draft: ContactDraft,
    },
    UpdateContact {
        id: String,
        patch: ContactPatch,
    },
    DeleteContact {
        id: String,
    },
}

impl Write {
    /// The surface a successful write changes, for the refresh after it.
    pub fn touches(&self) -> Mode {
        match self {
            Write::CreateEvent { .. } | Write::UpdateEvent { .. } | Write::DeleteEvent { .. } => {
                Mode::Calendar
            }
            Write::CreateContact { .. }
            | Write::UpdateContact { .. }
            | Write::DeleteContact { .. } => Mode::Contacts,
            _ => Mode::Mail,
        }
    }

    /// The emails a write takes out of the shown list.
    pub fn removes(&self) -> &[String] {
        match self {
            Write::Move { ids, .. } | Write::Delete { ids, .. } => ids,
            _ => &[],
        }
    }
}

fn to_value<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// Run `query` on `client`; the answer's JSON.
pub async fn run_query(client: &Client, query: Query) -> Result<Value, jmap::Error> {
    Ok(match query {
        Query::Mailboxes => json!({ "mailboxes": to_value(client.mailboxes().await?) }),
        Query::List {
            mailbox,
            text,
            offset,
            limit,
            sort,
        } => {
            let page = client
                .email_query_sorted(mailbox.as_deref(), text.as_deref(), sort, offset, limit)
                .await?;
            json!({
                "emails": to_value(page.emails),
                "total": page.total,
                "offset": offset,
                "page": offset / limit.max(1),
                "sort": sort.property.name(),
                "order": if sort.ascending { "asc" } else { "desc" },
            })
        }
        Query::Get { id } => {
            let email = client.email(&id).await?;
            let mut value = to_value(&email);
            value["text"] = json!(email.text_body_value());
            if email.text_body_value().is_none() {
                value["html"] = json!(email.html_body_value());
            }
            value
        }
        Query::Calendars => json!({ "calendars": to_value(client.calendars().await?) }),
        Query::Events { calendar, from, to } => {
            let events = client
                .calendar_events_between(&calendar, from.as_deref(), to.as_deref())
                .await?;
            json!({ "events": to_value(events) })
        }
        Query::Event { id } => to_value(client.calendar_event(&id).await?),
        Query::Books => json!({ "books": to_value(client.address_books().await?) }),
        Query::Contacts { book, filter } => {
            json!({ "contacts": to_value(client.contacts(&book, &filter).await?) })
        }
        Query::Contact { id } => to_value(client.contact(&id).await?),
    })
}

/// Run `write` on `client` (`sent`: the Sent mailbox, for a send); the
/// answer's JSON (a created id).
pub async fn run_write(
    client: &Client,
    write: Write,
    sent: Option<String>,
) -> Result<Value, jmap::Error> {
    Ok(match write {
        Write::Keywords { ids, read, flagged } => {
            client.set_keywords(&ids, read, flagged).await?;
            json!({ "ids": ids })
        }
        Write::Move { ids, mailbox } => {
            client.move_emails(&ids, &mailbox).await?;
            json!({ "ids": ids, "mailbox": mailbox })
        }
        Write::Delete { ids, destroy } => {
            client.delete_emails(&ids, destroy).await?;
            json!({ "ids": ids, "destroyed": destroy })
        }
        Write::CreateMailbox { name, parent } => {
            json!({ "id": client.create_mailbox(&name, parent.as_deref()).await? })
        }
        Write::RenameMailbox { id, name } => {
            client.rename_mailbox(&id, &name).await?;
            json!({ "id": id })
        }
        Write::DeleteMailbox { id } => {
            client.delete_mailbox(&id).await?;
            json!({ "id": id })
        }
        Write::TrashMailbox { id } => {
            json!({ "id": id, "moved": client.delete_mailbox_to_trash(&id).await? })
        }
        Write::MarkMailboxRead { id } => {
            json!({ "id": id, "marked": client.mark_mailbox_read(&id).await? })
        }
        Write::EmptyMailbox { id } => {
            json!({ "id": id, "destroyed": client.empty_mailbox(&id).await? })
        }
        Write::Send { draft } => {
            let sent =
                sent.ok_or_else(|| jmap::Error::Protocol(crate::label("no-sent-mailbox")))?;
            client.send(&draft, &sent).await?;
            json!({ "sent": true })
        }
        Write::CreateEvent { calendar, draft } => {
            json!({ "id": client.create_event(&calendar, &draft).await? })
        }
        Write::UpdateEvent { id, patch } => {
            client.update_event(&id, &patch).await?;
            json!({ "id": id })
        }
        Write::DeleteEvent { id } => {
            client.delete_event(&id).await?;
            json!({ "id": id })
        }
        Write::CreateContact { book, draft } => {
            json!({ "id": client.create_contact(&book, &draft).await? })
        }
        Write::UpdateContact { id, patch } => {
            client.update_contact(&id, &patch).await?;
            json!({ "id": id })
        }
        Write::DeleteContact { id } => {
            client.delete_contact(&id).await?;
            json!({ "id": id })
        }
    })
}

/// A server failure's refusal: an id the server does not hold is
/// `NOT_FOUND`, anything else `SERVER`. The one place an unknown id is told
/// from other failures: the engine has no typed not-found error yet, so a
/// `/set` refusal is matched by its JMAP type and an empty single-object
/// read by its message. A typed `jmap::Error::NotFound` is the follow-up.
pub fn server_refusal(error: &jmap::Error) -> (&'static str, String) {
    let message = error.to_string();
    let gone = match error {
        jmap::Error::Method { error_type, .. } => error_type == "notFound",
        // The engine's single-object reads say "… is gone".
        jmap::Error::Protocol(text) => text.ends_with("is gone"),
        _ => false,
    };
    (if gone { code::NOT_FOUND } else { code::SERVER }, message)
}

/// A refused argument.
struct Bad(&'static str, String);

type Checked<T> = Result<T, Bad>;

fn invalid(message: impl Into<String>) -> Bad {
    Bad(code::INVALID_ARGUMENT, message.into())
}

/// Optional string `key`; any other type is refused.
fn text(args: &Value, key: &str) -> Checked<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(invalid(format!("{key} must be a string"))),
    }
}

fn required(args: &Value, key: &str) -> Checked<String> {
    text(args, key)?
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| invalid(format!("{key} is required")))
}

fn flag(args: &Value, key: &str) -> Checked<Option<bool>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(invalid(format!("{key} must be true or false"))),
    }
}

/// `email.list`'s `sort` (`received`, `subject` or `from`; default
/// `received`) and `order` (`asc` or `desc`; default newest first for a
/// date, A to Z otherwise).
fn list_sort(args: &Value) -> Checked<jmap::EmailSort> {
    let property = match text(args, "sort")? {
        None => jmap::SortProperty::ReceivedAt,
        Some(name) => jmap::SortProperty::from_name(&name)
            .ok_or_else(|| invalid("sort must be received, subject or from"))?,
    };
    let ascending = match text(args, "order")?.as_deref() {
        None => property.ascending_first(),
        Some("asc") => true,
        Some("desc") => false,
        Some(_) => return Err(invalid("order must be asc or desc")),
    };
    Ok(jmap::EmailSort::new(property, ascending))
}

/// A whole number (Mix sends 10 or 10.0).
fn number(args: &Value, key: &str) -> Checked<Option<u64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| {
                n.as_f64()
                    .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                    .map(|f| f as u64)
            })
            .map(Some)
            .ok_or_else(|| invalid(format!("{key} must be a whole number"))),
        Some(_) => Err(invalid(format!("{key} must be a number"))),
    }
}

/// One string or a list of strings.
fn list(args: &Value, key: &str) -> Checked<Option<Vec<String>>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(vec![s.clone()])),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| invalid(format!("{key} must hold strings")))
            })
            .collect::<Checked<Vec<_>>>()
            .map(Some),
        Some(_) => Err(invalid(format!(
            "{key} must be a string or a list of strings"
        ))),
    }
}

fn ids(args: &Value) -> Checked<Vec<String>> {
    match list(args, "ids")?.or(list(args, "id")?) {
        Some(ids) if !ids.is_empty() => Ok(ids),
        _ => Err(invalid("ids is required")),
    }
}

/// Addresses as typed (`a@x, "B" <b@y>`) or as a list.
fn addresses(args: &Value, key: &str) -> Checked<Vec<jmap::EmailAddress>> {
    let joined = list(args, key)?.unwrap_or_default().join(", ");
    jmap::parse_addresses(&joined).map_err(|e| invalid(format!("{key}: {e}")))
}

/// The account named by `account` (or the selected one).
fn account(state: &State, args: &Value) -> Checked<String> {
    let id = text(args, "account")?.or_else(|| state.selected.clone());
    match id {
        Some(id) if state.account(&id).is_some() => Ok(id),
        Some(id) => Err(Bad(
            code::UNKNOWN_ACCOUNT,
            crate::label_with("unknown-account", &[("id", &id)]),
        )),
        None => Err(Bad(code::UNKNOWN_ACCOUNT, crate::label("no-account"))),
    }
}

/// The account named by `id`.
fn account_id(state: &State, args: &Value) -> Checked<String> {
    let id = required(args, "id")?;
    if state.account(&id).is_none() {
        return Err(Bad(
            code::UNKNOWN_ACCOUNT,
            crate::label_with("unknown-account", &[("id", &id)]),
        ));
    }
    Ok(id)
}

/// The account, which must be connected to reach its server.
fn connected(state: &State, args: &Value) -> Checked<String> {
    let id = account(state, args)?;
    match state.account(&id).map(|v| &v.phase) {
        Some(Phase::Ready) => Ok(id),
        _ => Err(Bad(code::NOT_CONNECTED, crate::label("not-connected"))),
    }
}

/// An end from a start and an ISO 8601 duration (`PT1H30M`, `P1D`, `P1W`).
pub fn end_after(start: &str, duration: &str) -> Result<String, String> {
    let begin = chrono::DateTime::parse_from_rfc3339(start)
        .map_err(|e| format!("start {start:?} is not an RFC 3339 instant: {e}"))?;
    let bad = || format!("duration {duration:?} is not an ISO 8601 duration");
    let rest = duration.strip_prefix('P').ok_or_else(bad)?;
    let (date, time) = rest.split_once('T').unwrap_or((rest, ""));
    let mut seconds: i64 = 0;
    let mut add = |part: &str, units: &[(char, i64)]| -> Result<(), String> {
        let mut digits = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() {
                digits.push(c);
                continue;
            }
            let scale = units.iter().find(|(u, _)| *u == c).ok_or_else(bad)?.1;
            let n: i64 = digits.parse().map_err(|_| bad())?;
            seconds = seconds
                .checked_add(n.checked_mul(scale).ok_or_else(bad)?)
                .ok_or_else(bad)?;
            digits.clear();
        }
        if digits.is_empty() {
            Ok(())
        } else {
            Err(bad())
        }
    };
    add(date, &[('W', 604_800), ('D', 86_400)])?;
    add(time, &[('H', 3_600), ('M', 60), ('S', 1)])?;
    if rest.is_empty() || (time.is_empty() && rest.contains('T')) {
        return Err(bad());
    }
    // Fallible all the way: a Bus caller's duration must never panic the
    // window, however large.
    let end = chrono::Duration::try_seconds(seconds)
        .and_then(|span| begin.checked_add_signed(span))
        .ok_or_else(|| format!("duration {duration:?} goes past the representable dates"))?;
    Ok(end.to_rfc3339())
}

/// Answer, change the state or queue the work for one Bus command; what
/// only the shell can do comes back.
pub fn command(
    state: &mut State,
    commands: &Registry<State>,
    strings: &Strings,
    id: u64,
    verb: &str,
    body: &str,
) -> Option<Forward> {
    let args = match serde_json::from_str::<Value>(body) {
        Ok(args) if args.is_object() => args,
        _ => {
            state.refuse(
                id,
                code::INVALID_ARGUMENT,
                "arguments must be a JSON object",
            );
            return None;
        }
    };
    if let Some(window) = verb
        .strip_prefix("email.")
        .filter(|v| drive::VERBS.contains(v))
    {
        return Some(Forward::Drive {
            id,
            verb: window.to_owned(),
            args,
        });
    }
    if verb == "email.show" {
        return Some(Forward::Show { id });
    }
    if let Err(Bad(code, message)) = dispatch(state, commands, strings, id, verb, &args) {
        state.refuse(id, code, &message);
    }
    None
}

fn dispatch(
    state: &mut State,
    commands: &Registry<State>,
    strings: &Strings,
    id: u64,
    verb: &str,
    args: &Value,
) -> Checked<()> {
    let query = |state: &mut State, account: String, query: Query| {
        state.query(account, id, query, Then::Answer);
    };
    match verb {
        "HELP" => state.reply(id, 0, describe()["verbs"].clone()),
        "app.describe" => state.reply(id, 0, describe()),
        "email.ping" => state.reply(
            id,
            0,
            json!({"pong": true, "schema": SCHEMA, "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id()}),
        ),
        "email.info" => state.reply(id, 0, state.info()),
        "email.commands" => {
            let body = json!({ "commands": commands.describe(state, strings) });
            state.reply(id, 0, body);
        }
        "email.accounts" => state.reply(id, 0, json!({ "accounts": state.accounts_view() })),
        "email.mailboxes" => {
            let account = connected(state, args)?;
            query(state, account, Query::Mailboxes);
        }
        "email.list" => {
            let account = connected(state, args)?;
            let mailbox = text(args, "mailbox")?.or_else(|| {
                state
                    .account(&account)
                    .and_then(|v| v.mail.selected.clone())
            });
            let limit = number(args, "limit")?
                .unwrap_or(u64::from(state.settings.mail.page_size))
                .clamp(1, 500);
            let offset = match (number(args, "offset")?, number(args, "page")?) {
                (Some(_), Some(_)) => return Err(invalid("give offset or page, not both")),
                (Some(offset), None) => offset,
                (None, Some(page)) => page.saturating_mul(limit),
                (None, None) => 0,
            };
            let list = Query::List {
                mailbox: Some(mailbox.ok_or_else(|| invalid("mailbox is required"))?),
                text: text(args, "query")?.filter(|t| !t.is_empty()),
                offset,
                limit,
                sort: list_sort(args)?,
            };
            query(state, account, list);
        }
        "email.search" if list::shows(args)? => {
            // The window's search: the list shows the result (no mailbox:
            // every folder; one: that folder).
            let account = connected(state, args)?;
            let scope = match text(args, "mailbox")? {
                Some(mailbox) => {
                    if !state.select_mailbox(&account, mailbox.clone()) {
                        return Err(Bad(
                            code::NOT_FOUND,
                            crate::label_with("no-such", &[("id", &mailbox)]),
                        ));
                    }
                    crate::state::Scope::Folder
                }
                None => crate::state::Scope::All,
            };
            state.set_search(&account, Some(required(args, "text")?), Some(scope));
            state.reply(id, 0, state.table_state(&account));
        }
        "email.search" => {
            let account = connected(state, args)?;
            let search = Query::List {
                mailbox: text(args, "mailbox")?,
                text: Some(required(args, "text")?),
                offset: 0,
                limit: number(args, "limit")?
                    .unwrap_or(u64::from(state.settings.mail.page_size))
                    .clamp(1, 500),
                sort: jmap::EmailSort::default(),
            };
            query(state, account, search);
        }
        "email.get" => {
            let account = connected(state, args)?;
            query(state, account, Query::Get {
                id: required(args, "id")?,
            });
        }
        "email.calendars" => {
            let account = connected(state, args)?;
            query(state, account, Query::Calendars);
        }
        "email.events" => {
            let account = connected(state, args)?;
            let calendar = text(args, "calendar")?
                .or_else(|| state.account(&account).and_then(|v| v.calendars.selected.clone()))
                .ok_or_else(|| invalid("calendar is required"))?;
            query(state, account, Query::Events {
                calendar,
                from: text(args, "from")?,
                to: text(args, "to")?,
            });
        }
        "email.event.get" => {
            let account = connected(state, args)?;
            query(state, account, Query::Event {
                id: required(args, "id")?,
            });
        }
        "email.books" => {
            let account = connected(state, args)?;
            query(state, account, Query::Books);
        }
        "email.contacts" => {
            let account = connected(state, args)?;
            let book = text(args, "book")?
                .or_else(|| state.account(&account).and_then(|v| v.contacts.selected.clone()))
                .ok_or_else(|| invalid("book is required"))?;
            query(state, account, Query::Contacts {
                book,
                filter: text(args, "filter")?.unwrap_or_default(),
            });
        }
        "email.contact.get" => {
            let account = connected(state, args)?;
            query(state, account, Query::Contact {
                id: required(args, "id")?,
            });
        }
        "email.quit" => {
            state.reply(id, 0, json!({ "quitting": true }));
            state.quit();
        }
        "email.execute" => {
            let command = required(args, "id")?;
            match commands.execute(&command, state, Origin::Bus) {
                Ok(()) => state.reply(id, 0, json!({ "executed": command })),
                Err(CommandError::Unknown(_)) => {
                    return Err(Bad(code::UNKNOWN_COMMAND, crate::label("unknown-command")));
                }
                Err(CommandError::Disabled(_)) => {
                    return Err(Bad(code::DISABLED, crate::label("command-disabled")));
                }
            }
        }
        "email.mode" => {
            let name = required(args, "name")?;
            let mode = Mode::from_name(&name)
                .ok_or_else(|| invalid("name must be mail, calendar or contacts"))?;
            state.set_mode(mode);
            state.reply(id, 0, json!({ "mode": mode }));
        }
        "email.refresh" => {
            let account = account(state, args)?;
            state.refresh_account(&account);
            state.reply(id, 0, json!({ "account": account }));
        }
        "email.account.set" => account_set(state, id, args)?,
        "email.account.remove" => {
            let account = account_id(state, args)?;
            // Removing it mid-migration would forget its password while the
            // schema-1 file still holds it: refused until the migration ends.
            if state.migration_locks(&account) {
                return Err(Bad(code::MIGRATION_PENDING, crate::label("migration-pending")));
            }
            state.change_accounts(Change::Remove(account), Some(id), None);
        }
        "email.account.password.set" => {
            let password = text(args, "password")?.ok_or_else(|| invalid("password is required"))?;
            if let Some(account) = text(args, "id")?
                && state.migration_locks(&account)
            {
                return Err(Bad(code::MIGRATION_PENDING, crate::label("migration-pending")));
            }
            if password.is_empty() {
                // A forget works for any well-formed id, an orphan whose
                // account settingsd no longer holds included (Prefs cleans
                // up after a removal this way).
                let account = required(args, "id")?;
                if !settings::email::valid_id(&account) {
                    return Err(invalid("id is not an account id"));
                }
                state.forget_password(&account, id);
            } else {
                let account = account_id(state, args)?;
                state.set_password(&account, &password, Some(id));
            }
        }
        "email.account.connect" => {
            let account = account_id(state, args)?;
            state.reconnect(&account);
            state.reply(id, 0, json!({ "id": account }));
        }
        "email.account.select" => {
            let account = account_id(state, args)?;
            state.select_account(&account);
            state.reply(id, 0, json!({ "id": account }));
        }
        "email.account.default" => {
            let account = match args.get("id") {
                None | Some(Value::Null) => None,
                Some(_) => Some(account_id(state, args)?),
            };
            state.change_accounts(Change::Default(account), Some(id), None);
        }
        "email.mailbox.select" => {
            let account = account(state, args)?;
            let mailbox = required(args, "id")?;
            if !state.select_mailbox(&account, mailbox.clone()) {
                return Err(Bad(code::NOT_FOUND, crate::label_with("no-such", &[("id", &mailbox)])));
            }
            state.reply(id, 0, json!({ "account": account, "id": mailbox }));
        }
        "email.mailbox.create" => {
            let account = connected(state, args)?;
            let write = Write::CreateMailbox {
                name: required(args, "name")?,
                parent: text(args, "parent")?,
            };
            state.write_server(account, Some(id), write);
        }
        "email.mailbox.rename" => {
            let account = connected(state, args)?;
            let write = Write::RenameMailbox {
                id: required(args, "id")?,
                name: required(args, "name")?,
            };
            state.write_server(account, Some(id), write);
        }
        "email.mailbox.delete" => {
            let account = connected(state, args)?;
            let write = Write::DeleteMailbox {
                id: required(args, "id")?,
            };
            state.write_server(account, Some(id), write);
        }
        "email.open" => {
            let account = connected(state, args)?;
            let email = required(args, "id")?;
            state.open_by_bus(&account, email, id);
        }
        "email.mark" => {
            let account = connected(state, args)?;
            let (read, flagged) = (flag(args, "read")?, flag(args, "flagged")?);
            if read.is_none() && flagged.is_none() {
                return Err(invalid("read or flagged is required"));
            }
            let write = Write::Keywords {
                ids: ids(args)?,
                read,
                flagged,
            };
            state.write_server(account, Some(id), write);
        }
        "email.move" => {
            let account = connected(state, args)?;
            // The path a drop onto a folder takes too.
            state.move_emails(&account, ids(args)?, required(args, "mailbox")?, Some(id));
        }
        "email.delete" => {
            let account = connected(state, args)?;
            let write = Write::Delete {
                ids: ids(args)?,
                destroy: flag(args, "destroy")?.unwrap_or(false),
            };
            state.write_server(account, Some(id), write);
        }
        "email.send" => {
            let account = connected(state, args)?;
            let to = addresses(args, "to")?;
            if to.is_empty() {
                return Err(invalid("to is required"));
            }
            let from = state.account(&account).map(|v| v.entry.email.clone()).unwrap_or_default();
            let draft = Composed {
                from,
                to,
                cc: addresses(args, "cc")?,
                bcc: addresses(args, "bcc")?,
                subject: required(args, "subject")?,
                body: text(args, "body")?.unwrap_or_default(),
                in_reply_to: text(args, "in_reply_to")?,
            };
            state.write_server(account, Some(id), Write::Send { draft });
        }
        "email.compose" => {
            let account = account(state, args)?;
            if state.compose.is_some() {
                return Err(Bad(code::BUSY, crate::label("compose-open")));
            }
            let draft = Composed {
                to: addresses(args, "to")?,
                cc: addresses(args, "cc")?,
                subject: text(args, "subject")?.unwrap_or_default(),
                body: text(args, "body")?.unwrap_or_default(),
                ..Composed::default()
            };
            state.open_compose(Some(&account), draft);
            state.reply(id, 0, json!({ "composing": account }));
        }
        "email.reply" | "email.forward" => {
            let account = connected(state, args)?;
            if state.compose.is_some() {
                return Err(Bad(code::BUSY, crate::label("compose-open")));
            }
            let email = required(args, "id")?;
            let then = if verb == "email.reply" {
                Then::Reply(flag(args, "all")?.unwrap_or(false))
            } else {
                Then::Forward(list(args, "to")?.unwrap_or_default().join(", "))
            };
            state.query(account, id, Query::Get { id: email }, then);
        }
        "email.calendar.select" => {
            let account = account(state, args)?;
            let calendar = required(args, "id")?;
            if !state.select_calendar(&account, calendar.clone()) {
                return Err(Bad(code::NOT_FOUND, crate::label_with("no-such", &[("id", &calendar)])));
            }
            state.reply(id, 0, json!({ "account": account, "id": calendar }));
        }
        "email.event.create" => {
            let account = connected(state, args)?;
            let calendar = text(args, "calendar")?
                .or_else(|| state.account(&account).and_then(|v| v.calendars.selected.clone()))
                .ok_or_else(|| invalid("calendar is required"))?;
            let start = required(args, "start")?;
            let end = match (text(args, "end")?, text(args, "duration")?) {
                (Some(end), _) => end,
                (None, Some(duration)) => end_after(&start, &duration).map_err(invalid)?,
                (None, None) => return Err(invalid("end or duration is required")),
            };
            let draft = EventDraft {
                title: required(args, "title")?,
                start,
                end,
                time_zone: text(args, "time_zone")?,
                show_without_time: flag(args, "all_day")?.unwrap_or(false),
                location: text(args, "location")?,
                description: text(args, "description")?,
            };
            state.write_server(account, Some(id), Write::CreateEvent { calendar, draft });
        }
        "email.event.update" => {
            let account = connected(state, args)?;
            let start = text(args, "start")?;
            let end = match (text(args, "end")?, text(args, "duration")?, &start) {
                (Some(end), _, _) => Some(end),
                (None, Some(duration), Some(start)) => Some(end_after(start, &duration).map_err(invalid)?),
                (None, Some(_), None) => return Err(invalid("duration needs start")),
                (None, None, _) => None,
            };
            let patch = EventPatch {
                title: text(args, "title")?,
                start,
                end,
                time_zone: text(args, "time_zone")?,
                show_without_time: flag(args, "all_day")?,
                location: text(args, "location")?,
                description: text(args, "description")?,
            };
            if patch == EventPatch::default() {
                return Err(invalid("nothing to change"));
            }
            let event = required(args, "id")?;
            state.write_server(account, Some(id), Write::UpdateEvent { id: event, patch });
        }
        "email.event.delete" => {
            let account = connected(state, args)?;
            let event = required(args, "id")?;
            state.write_server(account, Some(id), Write::DeleteEvent { id: event });
        }
        "email.book.select" => {
            let account = account(state, args)?;
            let book = required(args, "id")?;
            if !state.select_book(&account, book.clone()) {
                return Err(Bad(code::NOT_FOUND, crate::label_with("no-such", &[("id", &book)])));
            }
            state.reply(id, 0, json!({ "account": account, "id": book }));
        }
        "email.contact.create" => {
            let account = connected(state, args)?;
            let book = text(args, "book")?
                .or_else(|| state.account(&account).and_then(|v| v.contacts.selected.clone()))
                .ok_or_else(|| invalid("book is required"))?;
            let draft = ContactDraft {
                name: required(args, "name")?,
                emails: list(args, "emails")?.unwrap_or_default(),
                phones: list(args, "phones")?.unwrap_or_default(),
                organization: text(args, "org")?,
                notes: text(args, "notes")?,
            };
            state.write_server(account, Some(id), Write::CreateContact { book, draft });
        }
        "email.contact.update" => {
            let account = connected(state, args)?;
            let patch = ContactPatch {
                name: text(args, "name")?,
                emails: list(args, "emails")?,
                phones: list(args, "phones")?,
                organization: text(args, "org")?,
                notes: text(args, "notes")?,
            };
            if patch == ContactPatch::default() {
                return Err(invalid("nothing to change"));
            }
            let contact = required(args, "id")?;
            state.write_server(account, Some(id), Write::UpdateContact { id: contact, patch });
        }
        "email.contact.delete" => {
            let account = connected(state, args)?;
            let contact = required(args, "id")?;
            state.write_server(account, Some(id), Write::DeleteContact { id: contact });
        }
        other => {
            return list::dispatch(state, id, other, args)
                .unwrap_or_else(|| Err(Bad(code::UNKNOWN_VERB, crate::label("unknown-verb"))));
        }
    }
    Ok(())
}

/// `email.account.set`: a new account (no `id`, or none yet) or a change to
/// one, written to settingsd; the password, if any, to the secrets file
/// once settingsd has the account.
fn account_set(state: &mut State, id: u64, args: &Value) -> Checked<()> {
    let ca_path = match args.get("ca_path") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(path)) if path.trim().is_empty() => Some(None),
        Some(Value::String(path)) => Some(Some(path.trim().to_owned())),
        Some(_) => return Err(invalid("ca_path must be a string or null")),
    };
    let trimmed = |key: &str| -> Checked<Option<String>> {
        Ok(text(args, key)?.map(|v| v.trim().to_owned()))
    };
    let patch = AccountPatch {
        name: trimmed("name")?,
        url: trimmed("url")?.map(|u| u.trim_end_matches('/').to_owned()),
        email: trimmed("email")?,
        ca_path,
        enabled: flag(args, "enabled")?,
        signature: text(args, "signature")?,
    };
    let password = text(args, "password")?.filter(|p| !p.is_empty());
    if password.is_some()
        && let Some(account) = text(args, "id")?
        && state.migration_locks(&account)
    {
        return Err(Bad(
            code::MIGRATION_PENDING,
            crate::label("migration-pending"),
        ));
    }
    let change = match text(args, "id")? {
        // An update names only what it changes: a retry after a conflict
        // reapplies just those fields to the fresh read.
        Some(account) => {
            if state.settings.account(&account).is_none() {
                return Err(Bad(
                    code::UNKNOWN_ACCOUNT,
                    crate::label_with("unknown-account", &[("id", &account)]),
                ));
            }
            Change::Update(account, patch)
        }
        None => {
            let required = |value: &Option<String>, key: &str| -> Checked<String> {
                value
                    .clone()
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| invalid(format!("{key} is required")))
            };
            let mut entry = AccountEntry {
                id: settings::email::mint_account_id(&state.settings.accounts),
                name: required(&patch.name, "name")?,
                url: required(&patch.url, "url")?,
                email: required(&patch.email, "email")?,
                ca_path: None,
                enabled: true,
                signature: String::new(),
            };
            patch.apply(&mut entry);
            Change::Upsert(entry)
        }
    };
    state.change_accounts(change, Some(id), password);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::connected_many;
    use crate::work::Work;

    fn run(state: &mut State, verb: &str, args: Value) -> Option<Forward> {
        command(
            state,
            &crate::commands::registry(),
            &crate::strings(),
            1,
            verb,
            &args.to_string(),
        )
    }

    #[test]
    fn every_verb_is_described_once_with_parseable_args() {
        let mut seen = std::collections::BTreeSet::new();
        for verb in VERBS {
            assert!(seen.insert(verb.name), "{} twice", verb.name);
            let args: Value = serde_json::from_str(verb.args).expect(verb.name);
            assert!(args.is_object(), "{}", verb.name);
        }
        let described = describe();
        let names: Vec<&str> = described["verbs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names.len(),
            VERBS.len()
                + list::VERBS.len()
                + toolkit::datatable::verbs::VERBS.len()
                + drive::VERBS.len()
        );
        let unique: std::collections::BTreeSet<&&str> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "a name twice");
    }

    #[test]
    fn the_list_and_folder_verbs_are_handled() {
        let names = list::VERBS
            .iter()
            .map(|v| v.name.to_owned())
            .chain(
                toolkit::datatable::verbs::VERBS
                    .iter()
                    .map(|v| format!("email.{v}")),
            );
        for name in names {
            let mut state = connected_many(&["a"]);
            run(&mut state, &name, json!({}));
            let replies = state.take_replies();
            assert!(
                !replies
                    .iter()
                    .any(|r| r.body["error_code"] == code::UNKNOWN_VERB),
                "{name}: {replies:?}"
            );
        }
    }

    #[test]
    fn every_described_verb_is_handled() {
        for verb in VERBS {
            let mut state = connected_many(&["a"]);
            let forward = run(&mut state, verb.name, json!({}));
            let replies = state.take_replies();
            assert!(
                !replies
                    .iter()
                    .any(|r| r.body["error_code"] == code::UNKNOWN_VERB),
                "{}: {replies:?}",
                verb.name
            );
            if verb.name == "email.show" {
                assert_eq!(forward, Some(Forward::Show { id: 1 }));
            }
        }
        for verb in drive::VERBS {
            let mut state = connected_many(&["a"]);
            let forward = run(&mut state, &format!("email.{verb}"), json!({}));
            assert!(matches!(forward, Some(Forward::Drive { verb: v, .. }) if v == *verb));
        }
        let mut state = connected_many(&["a"]);
        run(&mut state, "email.nope", json!({}));
        assert_eq!(
            state.take_replies()[0].body["error_code"],
            code::UNKNOWN_VERB
        );
    }

    #[test]
    fn unknown_accounts_and_bad_arguments_are_distinct_refusals() {
        let mut state = connected_many(&["a"]);
        run(&mut state, "email.list", json!({"account": "zz"}));
        assert_eq!(
            state.take_replies()[0].body["error_code"],
            code::UNKNOWN_ACCOUNT
        );
        run(
            &mut state,
            "email.mailbox.select",
            json!({"account": "a", "id": "nope"}),
        );
        assert_eq!(state.take_replies()[0].body["error_code"], code::NOT_FOUND);
        run(&mut state, "email.mark", json!({"ids": ["E1"]}));
        assert_eq!(
            state.take_replies()[0].body["error_code"],
            code::INVALID_ARGUMENT
        );
        run(&mut state, "email.mode", json!({"name": 3}));
        let reply = &state.take_replies()[0];
        assert_eq!(
            (reply.rc, reply.body["error_code"].as_str()),
            (10, Some(code::INVALID_ARGUMENT))
        );
    }

    #[test]
    fn a_query_carries_its_request_id_to_the_account_client() {
        let mut state = connected_many(&["a", "b"]);
        run(
            &mut state,
            "email.list",
            json!({"account": "b", "mailbox": "m", "limit": 20.0}),
        );
        match state.take_work().as_slice() {
            [
                Work::Query {
                    account,
                    reply: 1,
                    query:
                        Query::List {
                            mailbox, limit: 20, ..
                        },
                    ..
                },
            ] => {
                assert_eq!(account, "b");
                assert_eq!(mailbox.as_deref(), Some("m"));
            }
            other => panic!("{other:?}"),
        }
        assert!(
            state.take_replies().is_empty(),
            "answered once the server does"
        );
    }

    #[test]
    fn a_list_asks_the_server_for_its_sort_and_page() {
        let mut state = connected_many(&["a"]);
        run(
            &mut state,
            "email.list",
            json!({"mailbox": "m", "sort": "subject", "page": 2, "limit": 20}),
        );
        match state.take_work().as_slice() {
            [
                Work::Query {
                    query:
                        Query::List {
                            offset: 40,
                            limit: 20,
                            sort,
                            ..
                        },
                    ..
                },
            ] => assert_eq!(
                *sort,
                jmap::EmailSort::new(jmap::SortProperty::Subject, true)
            ),
            other => panic!("{other:?}"),
        }
        // No sort is the newest first; an order flips it.
        run(
            &mut state,
            "email.list",
            json!({"mailbox": "m", "order": "asc"}),
        );
        assert!(matches!(
            state.take_work().as_slice(),
            [Work::Query {
                query: Query::List { offset: 0, sort, .. },
                ..
            }] if *sort == jmap::EmailSort::new(jmap::SortProperty::ReceivedAt, true)
        ));
        for bad in [
            json!({"mailbox": "m", "sort": "size"}),
            json!({"mailbox": "m", "order": "up"}),
            json!({"mailbox": "m", "page": 1, "offset": 5}),
            json!({"mailbox": "m", "page": -1}),
        ] {
            run(&mut state, "email.list", bad.clone());
            assert_eq!(
                state.take_replies()[0].body["error_code"],
                code::INVALID_ARGUMENT,
                "{bad}"
            );
        }
        assert!(state.take_work().is_empty());
    }

    #[test]
    fn the_accounts_never_carry_a_password() {
        let mut state = connected_many(&["a"]);
        run(&mut state, "email.accounts", json!({}));
        let body = state.take_replies()[0].body.to_string();
        assert!(
            body.contains("\"has_password\":true") && !body.contains("secret"),
            "{body}"
        );
        run(&mut state, "email.info", json!({}));
        let body = state.take_replies()[0].body.to_string();
        assert!(!body.contains("secret"), "{body}");
    }

    #[test]
    fn account_set_writes_settingsd_and_keeps_the_password_out_of_it() {
        let mut state = connected_many(&["a"]);
        run(
            &mut state,
            "email.account.set",
            json!({"name": "Work", "url": "https://w.example.invalid/", "email": "me@w.example.invalid", "password": "pw-9"}),
        );
        let work = state.take_work();
        let changes = work
            .iter()
            .find_map(|w| match w {
                Work::Settings { changes, .. } => Some(changes.clone()),
                _ => None,
            })
            .expect("a settings write");
        let accounts = changes["email.accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[1]["url"], "https://w.example.invalid");
        assert!(accounts[1]["id"].as_str().unwrap().starts_with("acct-"));
        assert!(!changes.to_string().contains("pw-9"));
        // An update keeps what it does not name.
        run(
            &mut state,
            "email.account.set",
            json!({"id": "a", "name": "Renamed"}),
        );
        let changes = state
            .take_work()
            .into_iter()
            .find_map(|w| match w {
                Work::Settings { changes, .. } => Some(changes),
                _ => None,
            })
            .unwrap();
        assert_eq!(changes["email.accounts"][0]["name"], "Renamed");
        assert_eq!(
            changes["email.accounts"][0]["email"],
            "ann@a.example.invalid"
        );
        run(
            &mut state,
            "email.account.set",
            json!({"id": "zz", "name": "x"}),
        );
        assert_eq!(
            state.take_replies()[0].body["error_code"],
            code::UNKNOWN_ACCOUNT
        );
    }

    /// A forget (empty password) works for any well-formed id, an orphan
    /// settingsd no longer holds included, and answers whether one was
    /// held; a real password for an unknown id is still refused.
    #[test]
    fn a_forget_works_for_an_orphaned_id() {
        // b's account is gone from settingsd, its secret is not.
        let mut orphaned = connected_many(&["a", "b"]);
        orphaned.set_settings(crate::state::tests::settings(&["a"]));
        orphaned.take_work();
        assert!(orphaned.account("b").is_none() && orphaned.has_password("b"));
        run(
            &mut orphaned,
            "email.account.password.set",
            json!({"id": "b", "password": ""}),
        );
        let work = orphaned.take_work();
        assert!(!orphaned.has_password("b"));
        assert!(orphaned.take_replies().is_empty(), "answered once saved");
        for w in &work {
            if let Work::SaveSecrets { ticket } = w {
                let version = orphaned.secrets_snapshot().0;
                orphaned.secrets_saved(*ticket, version, Ok(true));
            }
        }
        let reply = &orphaned.take_replies()[0];
        assert_eq!(reply.rc, 0);
        assert_eq!(reply.body["forgotten"], true);
        // Nothing held: still rc 0, forgotten false.
        run(
            &mut orphaned,
            "email.account.password.set",
            json!({"id": "zz", "password": ""}),
        );
        let reply = &orphaned.take_replies()[0];
        assert_eq!(
            (reply.rc, reply.body["forgotten"].clone()),
            (0, json!(false))
        );
        // A malformed id, or a real password for an unknown one, is refused.
        run(
            &mut orphaned,
            "email.account.password.set",
            json!({"id": "NO GOOD", "password": ""}),
        );
        assert_eq!(
            orphaned.take_replies()[0].body["error_code"],
            code::INVALID_ARGUMENT
        );
        run(
            &mut orphaned,
            "email.account.password.set",
            json!({"id": "zz", "password": "pw"}),
        );
        assert_eq!(
            orphaned.take_replies()[0].body["error_code"],
            code::UNKNOWN_ACCOUNT
        );
    }

    /// Review #15: while the schema-1 account is still migrating, its
    /// password can neither be set nor forgotten (a restart would bring the
    /// old one back): a recoverable MIGRATION_PENDING.
    #[test]
    fn review_15_the_migrating_account_refuses_password_changes() {
        let mut state = State::new(
            crate::state::UiState::default(),
            Mode::Mail,
            None,
            settings::email::Email::default(),
            crate::secrets::Secrets::default(),
        );
        let legacy = crate::state::tests::entry("legacy");
        state.begin_migration(legacy, "pw-old");
        state.take_work();
        for args in [
            json!({"id": "legacy", "password": "pw-new"}),
            json!({"id": "legacy", "password": ""}),
        ] {
            run(&mut state, "email.account.password.set", args);
            let reply = &state.take_replies()[0];
            assert_eq!(reply.body["error_code"], code::MIGRATION_PENDING);
        }
        run(
            &mut state,
            "email.account.set",
            json!({"id": "legacy", "password": "pw-new"}),
        );
        assert_eq!(
            state.take_replies()[0].body["error_code"],
            code::MIGRATION_PENDING
        );
        assert!(state.has_password("legacy"));
    }

    /// Round 4, #15: removing the account still migrating is refused too.
    #[test]
    fn round4_15_removing_the_migrating_account_is_refused() {
        let mut state = State::new(
            crate::state::UiState::default(),
            Mode::Mail,
            None,
            settings::email::Email::default(),
            crate::secrets::Secrets::default(),
        );
        state.begin_migration(crate::state::tests::entry("legacy"), "pw-old");
        state.take_work();
        run(&mut state, "email.account.remove", json!({"id": "legacy"}));
        let replies = state.take_replies();
        assert_eq!(
            replies.first().map(|r| r.body["error_code"].clone()),
            Some(json!(code::MIGRATION_PENDING))
        );
        assert!(state.has_password("legacy"));
    }

    #[test]
    fn execute_runs_registry_commands_with_the_bus_origin() {
        let commands = crate::commands::registry();
        let mut state = connected_many(&["a"]);
        command(
            &mut state,
            &commands,
            &crate::strings(),
            1,
            "email.execute",
            r#"{"id":"view.calendar"}"#,
        );
        assert_eq!(state.mode, Mode::Calendar);
        let runs = commands.take_executed().0;
        assert_eq!(
            (runs[0].id.as_str(), runs[0].origin),
            ("view.calendar", Origin::Bus)
        );
        command(
            &mut state,
            &commands,
            &crate::strings(),
            2,
            "email.execute",
            r#"{"id":"nope"}"#,
        );
        let replies = state.take_replies();
        assert_eq!(replies[1].body["error_code"], code::UNKNOWN_COMMAND);
    }

    #[test]
    fn every_command_is_reachable_by_execute() {
        let commands = crate::commands::registry();
        for command_ in commands.iter() {
            let mut state = connected_many(&["a"]);
            let body = json!({"id": command_.id}).to_string();
            command(
                &mut state,
                &commands,
                &crate::strings(),
                1,
                "email.execute",
                &body,
            );
            assert!(
                state.take_replies().iter().any(|r| r.id == 1),
                "{}",
                command_.id
            );
        }
    }

    #[test]
    fn a_duration_gives_the_end() {
        assert_eq!(
            end_after("2026-10-14T08:00:00+10:00", "PT1H30M").unwrap(),
            "2026-10-14T09:30:00+10:00"
        );
        assert_eq!(
            end_after("2026-10-14T08:00:00Z", "P1DT1H").unwrap(),
            "2026-10-15T09:00:00+00:00"
        );
        assert!(end_after("2026-10-14T08:00:00Z", "1H").is_err());
        assert!(end_after("2026-10-14T08:00:00Z", "PT").is_err());
        assert!(end_after("tomorrow", "PT1H").is_err());
    }

    /// Review finding 6: a huge duration is refused, never a panic, and the
    /// verb answers INVALID_ARGUMENT.
    #[test]
    fn an_overflowing_duration_is_refused_not_a_panic() {
        assert!(end_after("2026-10-14T08:00:00Z", "PT9223372036854775807S").is_err());
        assert!(end_after("2026-10-14T08:00:00Z", "P9000000000W").is_err());
        let mut state = connected_many(&["a"]);
        run(
            &mut state,
            "email.event.create",
            json!({"calendar": "C1", "title": "x", "start": "2026-10-14T08:00:00Z", "duration": "PT9223372036854775807S"}),
        );
        let reply = &state.take_replies()[0];
        assert_eq!(reply.body["error_code"], code::INVALID_ARGUMENT);
    }

    #[test]
    fn server_failures_name_a_missing_id() {
        let gone = jmap::Error::Protocol("email \"E9\" is gone".into());
        assert_eq!(server_refusal(&gone).0, code::NOT_FOUND);
        let missing = jmap::Error::method("notFound", None);
        assert_eq!(server_refusal(&missing).0, code::NOT_FOUND);
        let other = jmap::Error::method("serverFail", Some("down"));
        assert_eq!(server_refusal(&other).0, code::SERVER);
    }
}
