// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The `email` Bus port, schema `email.v1`: every command, its arguments, and
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
//! published on `email.action.executed`. The window also serves the
//! toolkit's drive commands (`email.ui.*`, `email.window*`).

use jmap::{Client, Composed, ContactDraft, ContactPatch, EventDraft, EventPatch};
use serde::Serialize;
use serde_json::{Value, json};
use settings::email::AccountEntry;
use toolkit::action::{CommandError, Origin};
use toolkit::{Registry, Strings, drive};

use crate::state::{Mode, Phase, State, code};
use crate::writer::{AccountPatch, Change};
use crate::{APP, APP_ID};

pub const SCHEMA: &str = "email.v1";

/// One command as `HELP` and `app.describe` list it.
pub struct Command {
    pub name: &'static str,
    pub read_only: bool,
    /// The arguments, as a JSON object of name to description.
    pub args: &'static str,
    pub description: &'static str,
}

const fn command_spec(
    name: &'static str,
    read_only: bool,
    args: &'static str,
    description: &'static str,
) -> Command {
    Command {
        name,
        read_only,
        args,
        description,
    }
}

/// Every command the app answers itself, in manifest order. The window's drive
/// commands follow them in [`describe`].
pub const COMMANDS: &[Command] = &[
    command_spec("HELP", true, "{}", "List every command"),
    command_spec(
        "app.describe",
        true,
        "{}",
        "Describe the app and every command with its arguments",
    ),
    command_spec("email.ping", true, "{}", "Probe Email"),
    command_spec(
        "email.info",
        true,
        "{}",
        "The whole window state: accounts (no secret), selection, mode, lists shown, compose, dialogs and UI shape",
    ),
    command_spec(
        "email.actions",
        true,
        "{}",
        "The app's commands with labels, shortcuts and enablement",
    ),
    command_spec(
        "email.accounts",
        true,
        "{}",
        "The accounts in tree order: settings, whether a password is held (has_password), connection state and error; never a password",
    ),
    command_spec(
        "email.mailboxes",
        true,
        r#"{"account":"optional account id"}"#,
        "The account's mailboxes from its server, with roles, parents and counts",
    ),
    command_spec(
        "email.list",
        true,
        r#"{"account":"optional account id","mailbox":"optional mailbox id (default the selected one)","offset":"optional integer","limit":"optional integer 1..500 (default the page size)","query":"optional text to match"}"#,
        "One page of a mailbox, newest first, with the total count",
    ),
    command_spec(
        "email.search",
        true,
        r#"{"account":"optional account id","text":"string","mailbox":"optional mailbox id","limit":"optional integer 1..500"}"#,
        "Emails matching text, in one mailbox or all",
    ),
    command_spec(
        "email.get",
        true,
        r#"{"account":"optional account id","id":"email id"}"#,
        "One email's headers and text body",
    ),
    command_spec(
        "email.calendars",
        true,
        r#"{"account":"optional account id"}"#,
        "The account's calendars",
    ),
    command_spec(
        "email.events",
        true,
        r#"{"account":"optional account id","calendar":"optional calendar id (default the selected one)","from":"optional RFC 3339 instant","to":"optional RFC 3339 instant"}"#,
        "A calendar's events in an optional window, newest start first",
    ),
    command_spec(
        "email.event.get",
        true,
        r#"{"account":"optional account id","id":"event id"}"#,
        "One event with its JSCalendar data",
    ),
    command_spec(
        "email.books",
        true,
        r#"{"account":"optional account id"}"#,
        "The account's address books",
    ),
    command_spec(
        "email.contacts",
        true,
        r#"{"account":"optional account id","book":"optional address book id (default the selected one)","filter":"optional text over name, email and company"}"#,
        "An address book's contacts",
    ),
    command_spec(
        "email.contact.get",
        true,
        r#"{"account":"optional account id","id":"contact id"}"#,
        "One contact with its JSContact data",
    ),
    command_spec("email.show", false, "{}", "Restore and focus the window"),
    command_spec("email.quit", false, "{}", "Close the window"),
    command_spec(
        "email.execute",
        false,
        r#"{"id":"command id from email.actions"}"#,
        "Run one app command by id, as its menu item or shortcut would",
    ),
    command_spec(
        "email.mode",
        false,
        r#"{"name":"mail, calendar or contacts"}"#,
        "Show a surface",
    ),
    command_spec(
        "email.refresh",
        false,
        r#"{"account":"optional account id"}"#,
        "Fetch the account's mailboxes, calendars and address books again (a failed account reconnects)",
    ),
    command_spec(
        "email.account.set",
        false,
        r#"{"id":"optional account id (absent: a new account)","name":"string","url":"https://… JMAP base","email":"login address","password":"optional string, kept only in the app's secrets file","ca_path":"optional absolute PEM path or null","enabled":"optional bool","signature":"optional plain text","identities":"optional list of {id, name, address, signature, reply_to}; replaces the account's identities, [] for none"}"#,
        "Create or update an account in settingsd; answers {id} once settingsd accepts",
    ),
    command_spec(
        "email.account.remove",
        false,
        r#"{"id":"account id"}"#,
        "Remove an account from settingsd (and its password); the default resets with it",
    ),
    command_spec(
        "email.account.password.set",
        false,
        r#"{"id":"account id (for a forget, any account id, one settingsd no longer holds included)","password":"string; empty forgets it"}"#,
        "Keep an account's password in the app's secrets file and reconnect; write-only. An empty password forgets it and answers {forgotten: bool}",
    ),
    command_spec(
        "email.account.connect",
        false,
        r#"{"id":"account id"}"#,
        "Connect an account again",
    ),
    command_spec(
        "email.account.select",
        false,
        r#"{"id":"account id"}"#,
        "Show an account",
    ),
    command_spec(
        "email.account.default",
        false,
        r#"{"id":"account id, or null for the first enabled one"}"#,
        "Make an account the compose default in settingsd",
    ),
    command_spec(
        "email.mailbox.select",
        false,
        r#"{"account":"optional account id","id":"mailbox id"}"#,
        "Show a mailbox",
    ),
    command_spec(
        "email.mailbox.create",
        false,
        r#"{"account":"optional account id","name":"string","parent":"optional mailbox id"}"#,
        "Create a mailbox; answers {id}",
    ),
    command_spec(
        "email.mailbox.rename",
        false,
        r#"{"account":"optional account id","id":"mailbox id","name":"string"}"#,
        "Rename a mailbox",
    ),
    command_spec(
        "email.mailbox.delete",
        false,
        r#"{"account":"optional account id","id":"mailbox id"}"#,
        "Delete an empty mailbox",
    ),
    command_spec(
        "email.open",
        false,
        r#"{"account":"optional account id","id":"email id"}"#,
        "Open an email in the reader; answers once it is loaded",
    ),
    command_spec(
        "email.mark",
        false,
        r#"{"account":"optional account id","ids":"email id or list of ids","read":"optional bool","flagged":"optional bool"}"#,
        "Set or clear read and flagged, one Email/set per 500 ids; answers {ids, done, failed: [{id, reason}], partial} (partial is true when some ids changed and some failed) and is refused only when every id failed",
    ),
    command_spec(
        "email.move",
        false,
        r#"{"account":"optional account id","ids":"email id or list of ids","mailbox":"mailbox id"}"#,
        "Move emails to a mailbox, one Email/set per 500 ids; answers {ids, mailbox, done, failed: [{id, reason}], partial} (partial is true when some ids changed and some failed) and is refused only when every id failed",
    ),
    command_spec(
        "email.junk",
        false,
        r#"{"account":"optional account id","ids":"email id or list of ids","junk":"bool: true moves to the Junk mailbox, false back to the inbox"}"#,
        "Mark emails as junk or not junk by moving them into or out of the Junk mailbox; maild trains its spam filter (spam or ham) on the move",
    ),
    command_spec(
        "email.delete",
        false,
        r#"{"account":"optional account id","ids":"email id or list of ids","destroy":"optional bool (default false: to Trash)"}"#,
        "Move emails to Trash, or destroy them, one Email/set per 500 ids; answers {ids, destroyed, done, failed: [{id, reason}], partial} (partial is true when some ids changed and some failed) and is refused only when every id failed",
    ),
    command_spec(
        "email.send",
        false,
        r#"{"account":"optional account id","to":"addresses","cc":"optional addresses","bcc":"optional addresses","subject":"string","body":"string","in_reply_to":"optional Message-ID","attachments":"optional list of absolute file paths"}"#,
        "Send an email without the dialog",
    ),
    command_spec(
        "email.compose",
        false,
        r#"{"account":"optional account id","to":"optional addresses","cc":"optional addresses","bcc":"optional addresses","reply_to":"optional address (absent: the identity's)","subject":"optional string","body":"optional string","attachments":"optional list of absolute file paths"}"#,
        "Open the compose dialog, filled in",
    ),
    command_spec(
        "email.compose.attachment.remove",
        false,
        r#"{"path":"attachment path as listed by email.info"}"#,
        "Remove every occurrence of a path from the open compose; answers with compose state",
    ),
    command_spec(
        "email.reply",
        false,
        r#"{"account":"optional account id","id":"email id","all":"optional bool","quote":"optional bool (absent: email.compose.quote_on_reply)"}"#,
        "Open the compose dialog replying to an email",
    ),
    command_spec(
        "email.forward",
        false,
        r#"{"account":"optional account id","id":"email id","to":"optional addresses"}"#,
        "Open the compose dialog forwarding an email",
    ),
    command_spec(
        "email.attachments",
        true,
        r#"{"account":"optional account id","id":"email id"}"#,
        "An email's attachments (part, name, type, size, undecodable, save_only), without their content",
    ),
    command_spec(
        "email.attachment.save",
        false,
        r#"{"account":"optional account id","id":"email id","part":"attachment part id","dir":"optional absolute existing directory (default: the downloads directory)"}"#,
        "Save one attachment, never replacing a file (a taken name is numbered); answers {path}",
    ),
    command_spec(
        "email.contacts.suggest",
        true,
        r#"{"account":"optional account id","text":"what is typed (at least 2 characters)","limit":"optional count, default 8"}"#,
        "Address completions for text: the account's contacts first, then addresses it has seen, ranked and deduplicated",
    ),
    command_spec(
        "email.print",
        false,
        r#"{"account":"optional account id","id":"optional email id (default: the open email)"}"#,
        "Write the email as a self-contained print page and open it in the browser, whose print dialog prints or saves a PDF; answers {path, opened}",
    ),
    command_spec(
        "email.calendar.select",
        false,
        r#"{"account":"optional account id","id":"calendar id"}"#,
        "Show a calendar",
    ),
    command_spec(
        "email.event.create",
        false,
        r#"{"account":"optional account id","calendar":"optional calendar id (default the selected one)","title":"string","start":"RFC 3339 instant with an offset","end":"RFC 3339 instant with an offset (or duration)","duration":"ISO 8601 duration from start (or end)","time_zone":"optional IANA zone","all_day":"optional bool","location":"optional string","description":"optional string"}"#,
        "Create an event in a calendar (its id a UUID, as maild requires); the engine computes the duration; answers {id}",
    ),
    command_spec(
        "email.event.update",
        false,
        r#"{"account":"optional account id","id":"event id","title":"optional string","start":"optional RFC 3339 instant","end":"optional RFC 3339 instant","duration":"optional ISO 8601 duration (from start)","time_zone":"optional string","all_day":"optional bool","location":"optional string","description":"optional string"}"#,
        "Change an event's fields; maild keeps only title, start and end on an update and ignores the rest",
    ),
    command_spec(
        "email.event.delete",
        false,
        r#"{"account":"optional account id","id":"event id"}"#,
        "Delete an event",
    ),
    command_spec(
        "email.book.select",
        false,
        r#"{"account":"optional account id","id":"address book id"}"#,
        "Show an address book",
    ),
    command_spec(
        "email.contact.create",
        false,
        r#"{"account":"optional account id","book":"optional address book id (default the selected one)","name":"string","emails":"optional list of addresses","phones":"optional list of numbers","org":"optional string","notes":"optional string"}"#,
        "Create a contact; answers {id}",
    ),
    command_spec(
        "email.contact.update",
        false,
        r#"{"account":"optional account id","id":"contact id","name":"optional string","emails":"optional list","phones":"optional list","org":"optional string","notes":"optional string"}"#,
        "Change a contact's fields",
    ),
    command_spec(
        "email.contact.delete",
        false,
        r#"{"account":"optional account id","id":"contact id"}"#,
        "Delete a contact",
    ),
];

/// The whole Bus surface: the app's commands and the window's drive commands.
pub fn describe() -> Value {
    let mut command_names: Vec<Value> = COMMANDS
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
    command_names.extend(drive::describe(APP));
    command_names.extend(citizen::macros::describe(APP));
    json!({"schema": SCHEMA, "app_id": APP_ID, "version": env!("CARGO_PKG_VERSION"), "commands": command_names})
}

/// What only the shell can do for a command.
#[derive(Clone, Debug, PartialEq)]
pub enum Forward {
    /// A drive command (`email.ui.*`, `email.window*`), without the prefix.
    Drive {
        id: u64,
        command: String,
        args: Value,
    },
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
    /// Contacts in any address book matching `text` (address completion).
    Suggest {
        text: String,
    },
}

/// How many matching contacts a completion fetches: more than it shows,
/// since one card can hold several addresses and some are deduplicated.
pub const SUGGEST_FETCH: u32 = 20;

/// What happens with a query's answer besides the reply.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// Answer it.
    Answer,
    /// Open a compose replying to the email fetched (`true`: to all),
    /// quoting it or not (`None`: as the settings say).
    Reply(bool, Option<bool>),
    /// Open a compose forwarding the email fetched, to these addresses.
    Forward(String),
    /// Print the email fetched (`email.print`).
    Print,
    /// Answer with the ranked completions for `text`, at most `limit`
    /// (`email.contacts.suggest`).
    Suggest { text: String, limit: usize },
    /// Answer with the email fetched's attachments (`email.attachments`).
    Attachments,
    /// Save the email fetched's attachment `part`, in `dir` or the
    /// downloads directory (`email.attachment.save`).
    SaveAttachment { part: String, dir: Option<String> },
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

    /// The emails a message write names (none for any other write).
    pub fn ids(&self) -> &[String] {
        match self {
            Write::Keywords { ids, .. } | Write::Move { ids, .. } | Write::Delete { ids, .. } => {
                ids
            }
            _ => &[],
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
        } => {
            let page = client
                .email_query(mailbox.as_deref(), text.as_deref(), offset, limit)
                .await?;
            json!({ "emails": to_value(page.emails), "total": page.total, "offset": offset })
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
        Query::Suggest { text } => {
            json!({ "contacts": to_value(client.contacts_search(&text, SUGGEST_FETCH).await?) })
        }
    })
}

/// Run `write` on `client` (`sent`: the Drafts and Sent mailboxes, for a
/// send); the answer's JSON (a created id).
pub async fn run_write(
    client: &Client,
    write: Write,
    sent: Option<jmap::SendBoxes>,
) -> Result<Value, jmap::Error> {
    Ok(match write {
        Write::Keywords { .. } | Write::Move { .. } | Write::Delete { .. } => {
            let trash = match write {
                Write::Delete { destroy: false, .. } => {
                    let mailboxes = client.mailboxes().await?;
                    mailboxes
                        .into_iter()
                        .find(|m| m.role.as_deref() == Some("trash"))
                        .map(|m| m.id)
                }
                _ => None,
            };
            let (ids, change) = batch_change(&write, trash.as_deref())?;
            let outcome = client.change_emails(ids, &change).await;
            batch_answer(&write, outcome)?
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
        Write::Send { draft } => {
            let boxes =
                sent.ok_or_else(|| jmap::Error::Protocol(crate::label("no-sent-mailbox")))?;
            // A Bus send is one try: created in Drafts, filed in Sent only
            // once submitted. Its caller hears whether it went.
            let (_, result) = client
                .send(&draft, &boxes, jmap::SendAttempt::default())
                .await;
            let report = result?;
            json!({ "sent": true, "filed": report.unfiled.is_none() })
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

/// The batched change a message write makes (`email.mark`, `email.move`,
/// `email.junk`, `email.delete`) and the ids it names: the same Email/set
/// path as the window's actions, one request per
/// [`jmap::MAX_IDS_PER_REQUEST`] ids. `trash` is the account's Trash
/// mailbox, which a delete that does not destroy needs.
pub fn batch_change<'a>(
    write: &'a Write,
    trash: Option<&str>,
) -> Result<(&'a [String], jmap::EmailChange), jmap::Error> {
    let change = match write {
        Write::Keywords { read, flagged, .. } => jmap::EmailChange::Keywords {
            read: *read,
            flagged: *flagged,
        },
        Write::Move { mailbox, .. } => jmap::EmailChange::Move {
            mailbox: mailbox.clone(),
        },
        Write::Delete { destroy: true, .. } => jmap::EmailChange::Destroy,
        Write::Delete { destroy: false, .. } => {
            let mailbox = trash
                .ok_or_else(|| jmap::Error::Protocol("no mailbox has the trash role".into()))?;
            jmap::EmailChange::Move {
                mailbox: mailbox.to_owned(),
            }
        }
        _ => return Err(jmap::Error::Protocol("not a message write".into())),
    };
    Ok((write.ids(), change))
}

/// A batched message write's answer: the fields the command always answered
/// with (`ids` as asked, and `mailbox` or `destroyed`), plus `done` (the
/// ids changed) and `failed` (`[{id, reason}]`). One bad id never fails
/// the call; only when every id failed is it refused, as a single-id call
/// always was (`NOT_FOUND` when every id is unknown to the server).
pub fn batch_answer(write: &Write, outcome: jmap::SetOutcome) -> Result<Value, jmap::Error> {
    if outcome.done.is_empty() && !outcome.failed.is_empty() {
        let reasons = outcome
            .failed
            .iter()
            .map(|(id, why)| format!("{id} ({why})"))
            .collect::<Vec<_>>()
            .join(", ");
        let all_gone = outcome
            .failed
            .iter()
            .all(|(_, why)| why.starts_with("notFound"));
        return Err(if all_gone {
            jmap::Error::Method {
                error_type: "notFound".into(),
                description: reasons,
            }
        } else {
            jmap::Error::Protocol(reasons)
        });
    }
    let failed: Vec<Value> = outcome
        .failed
        .iter()
        .map(|(id, reason)| json!({ "id": id, "reason": reason }))
        .collect();
    let mut answer = match write {
        Write::Keywords { ids, .. } => json!({ "ids": ids }),
        Write::Move { ids, mailbox } => json!({ "ids": ids, "mailbox": mailbox }),
        Write::Delete { ids, destroy } => json!({ "ids": ids, "destroyed": destroy }),
        _ => json!({}),
    };
    // A partial result: some ids changed and some did not. JMAP cannot roll
    // back, so the reply says so instead of refusing the call.
    answer["partial"] = json!(!outcome.done.is_empty() && !outcome.failed.is_empty());
    answer["done"] = json!(outcome.done);
    answer["failed"] = Value::Array(failed);
    Ok(answer)
}

/// A server failure's refusal: an id the server does not hold is
/// `NOT_FOUND`, anything else `SERVER`. The one place an unknown id is told
/// from other failures: the engine has no typed not-found error yet, so a
/// `/set` refusal is matched by its JMAP type and an empty single-object
/// read by its message. A typed `jmap::Error::NotFound` is the follow-up.
pub fn server_refusal(error: &jmap::Error) -> (&'static str, String) {
    let message = error.to_string();
    let attachment_code = match error {
        jmap::Error::AttachmentRelative(_) => Some(code::ATTACHMENT_RELATIVE),
        jmap::Error::AttachmentNotFile(_) => Some(code::ATTACHMENT_NOT_FILE),
        jmap::Error::AttachmentUnreadable { .. } => Some(code::ATTACHMENT_UNREADABLE),
        jmap::Error::MessageTooLarge { .. } => Some(code::MESSAGE_TOO_LARGE),
        _ => None,
    };
    if let Some(code) = attachment_code {
        return (code, message);
    }
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

/// A list of sender identities (`{id, name, address, signature, reply_to}`
/// objects), or None when absent. The settings validation checks the values.
fn identities(args: &Value, key: &str) -> Checked<Option<Vec<settings::email::Identity>>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|e| invalid(format!("{key}: {e}"))),
    }
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

fn attachment_paths(args: &Value) -> Checked<Vec<std::path::PathBuf>> {
    match args.get("attachments") {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(std::path::PathBuf::from)
                    .ok_or_else(|| invalid("attachments must hold absolute file paths"))
            })
            .collect(),
        _ => Err(invalid("attachments must be a list of absolute file paths")),
    }
}

fn validate_message(state: &State, account: &str, draft: &Composed) -> Checked<()> {
    let limit = state
        .account(account)
        .and_then(|view| view.session.as_ref())
        .and_then(|session| session.max_size_upload);
    jmap::validate_attachments(draft, limit).map_err(|error| {
        let (code, message) = server_refusal(&error);
        Bad(code, message)
    })
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
    command_name: &str,
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
    if let Some(window) = command_name
        .strip_prefix("email.")
        .filter(|v| drive::COMMANDS.contains(v))
    {
        return Some(Forward::Drive {
            id,
            command: window.to_owned(),
            args,
        });
    }
    if command_name == "email.show" {
        return Some(Forward::Show { id });
    }
    if let Err(Bad(code, message)) = dispatch(state, commands, strings, id, command_name, &args) {
        state.refuse(id, code, &message);
    }
    None
}

fn dispatch(
    state: &mut State,
    commands: &Registry<State>,
    strings: &Strings,
    id: u64,
    command_name: &str,
    args: &Value,
) -> Checked<()> {
    let query = |state: &mut State, account: String, query: Query| {
        state.query(account, id, query, Then::Answer);
    };
    match command_name {
        "HELP" => state.reply(id, 0, describe()["commands"].clone()),
        "app.describe" => state.reply(id, 0, describe()),
        "email.ping" => state.reply(
            id,
            0,
            json!({"pong": true, "schema": SCHEMA, "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id()}),
        ),
        "email.info" => state.reply(id, 0, state.info()),
        "email.actions" => {
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
            let limit = number(args, "limit")?.unwrap_or(u64::from(state.settings.mail.page_size));
            let list = Query::List {
                mailbox: Some(mailbox.ok_or_else(|| invalid("mailbox is required"))?),
                text: text(args, "query")?.filter(|t| !t.is_empty()),
                offset: number(args, "offset")?.unwrap_or(0),
                limit: limit.clamp(1, 500),
            };
            query(state, account, list);
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
            // A draft open with something typed is saved and the person is
            // asked first: the caller hears that it waits on them.
            state.quit();
            let waiting = state.confirm.is_some();
            state.reply(
                id,
                0,
                json!({ "quitting": !waiting, "confirming": waiting }),
            );
        }
        "email.execute" => {
            let command = required(args, "id")?;
            match commands.execute(&command, state, Origin::Bus) {
                Ok(()) => {
                    if command == "mail.send"
                        && !state
                            .compose
                            .as_ref()
                            .is_some_and(|c| c.busy && c.sending.is_some())
                    {
                        let message = state
                            .compose
                            .as_ref()
                            .and_then(|c| c.attachment_error.as_ref().or(c.error.as_ref()))
                            .cloned()
                            .unwrap_or_else(|| crate::label("command-disabled"));
                        return Err(Bad(code::DISABLED, message));
                    }
                    state.reply(id, 0, json!({ "executed": command }));
                }
                Err(CommandError::Unknown(_)) => {
                    return Err(Bad(code::UNKNOWN_ACTION, crate::label("unknown-action")));
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
            let write = Write::Move {
                ids: ids(args)?,
                mailbox: required(args, "mailbox")?,
            };
            state.write_server(account, Some(id), write);
        }
        "email.junk" => {
            let account = connected(state, args)?;
            let junk = flag(args, "junk")?.ok_or_else(|| invalid("junk must be true or false"))?;
            let mailbox = state
                .account(&account)
                .and_then(|v| crate::state::junk_target(v, junk))
                .ok_or_else(|| {
                    let key = if junk { "no-junk-mailbox" } else { "no-inbox" };
                    Bad(code::NOT_FOUND, crate::label(key))
                })?;
            let write = Write::Move {
                ids: ids(args)?,
                mailbox,
            };
            state.write_server(account, Some(id), write);
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
                attachments: attachment_paths(args)?,
                in_reply_to: text(args, "in_reply_to")?,
                from_name: String::new(),
                reply_to: None,
            };
            validate_message(state, &account, &draft)?;
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
                bcc: addresses(args, "bcc")?,
                subject: text(args, "subject")?.unwrap_or_default(),
                body: text(args, "body")?.unwrap_or_default(),
                attachments: attachment_paths(args)?,
                ..Composed::default()
            };
            // The identity sets Reply-To; an explicit one replaces it.
            let reply_to = text(args, "reply_to")?.filter(|r| !r.trim().is_empty());
            // The editor's body cap is separate from the upload limit. Check
            // it first so an oversized body remains INVALID_ARGUMENT.
            state.open_compose(Some(&account), draft);
            if let Some(error) = state.close_failed_compose() {
                return Err(invalid(&error));
            }
            if let (Some(reply_to), Some(compose)) = (reply_to, state.compose.as_mut()) {
                compose.draft.reply_to = Some(reply_to);
            }
            state.check_compose_attachment_size();
            if let Some(compose) = &state.compose {
                let draft = Composed {
                    body: compose.body.contents(),
                    ..compose.draft.clone()
                };
                if let Err(error) = validate_message(state, &account, &draft) {
                    state.compose = None;
                    return Err(error);
                }
            }
            state.reply(
                id,
                0,
                json!({ "composing": account, "compose": state.info()["compose"] }),
            );
        }
        "email.compose.attachment.remove" => {
            let path = std::path::PathBuf::from(required(args, "path")?);
            let compose = state
                .compose
                .as_ref()
                .ok_or_else(|| Bad(code::NOT_FOUND, crate::label("compose-not-open")))?;
            if compose.busy {
                return Err(Bad(code::BUSY, crate::label("sending")));
            }
            if !state.remove_compose_attachment(&path) {
                return Err(Bad(code::NOT_FOUND, crate::label("no-such-attachment")));
            }
            state.reply(id, 0, json!({ "compose": state.info()["compose"] }));
        }
        "email.reply" | "email.forward" => {
            let account = connected(state, args)?;
            if state.compose.is_some() {
                return Err(Bad(code::BUSY, crate::label("compose-open")));
            }
            let email = required(args, "id")?;
            let then = if command_name == "email.reply" {
                Then::Reply(flag(args, "all")?.unwrap_or(false), flag(args, "quote")?)
            } else {
                Then::Forward(list(args, "to")?.unwrap_or_default().join(", "))
            };
            state.query(account, id, Query::Get { id: email }, then);
        }
        "email.contacts.suggest" => {
            let account = connected(state, args)?;
            let text = required(args, "text")?;
            let limit = number(args, "limit")?.map_or(crate::suggest::LIMIT, |n| n.clamp(1, 50) as usize);
            if text.trim().chars().count() < crate::suggest::MIN_CHARS {
                return Err(invalid("text needs at least 2 characters"));
            }
            let query = Query::Suggest {
                text: text.trim().to_owned(),
            };
            state.query(account, id, query, Then::Suggest { text, limit });
        }
        "email.attachments" | "email.attachment.save" => {
            let account = connected(state, args)?;
            let wanted = required(args, "id")?;
            let open = state
                .account(&account)
                .and_then(|v| v.mail.open.clone())
                .filter(|open| open.id == wanted);
            if command_name == "email.attachments" {
                match open {
                    Some(open) => {
                        let transfers = state.account(&account).map(|v| v.mail.transfers.clone());
                        let list = crate::state::attachments_view(&open, transfers.as_ref());
                        state.reply(id, 0, json!({ "id": open.id, "attachments": list }));
                    }
                    None => state.query(account, id, Query::Get { id: wanted }, Then::Attachments),
                }
            } else {
                let part = required(args, "part")?;
                let dir = text(args, "dir")?;
                match open {
                    Some(open) => {
                        let work = crate::state::attachment_work(&account, &open, &part, false, dir, Some(id))
                            .map_err(|(code, message)| Bad(code, message))?;
                        state.queue_work(work);
                    }
                    None => state.query(
                        account,
                        id,
                        Query::Get { id: wanted },
                        Then::SaveAttachment { part, dir },
                    ),
                }
            }
        }
        "email.print" => {
            let account = connected(state, args)?;
            let open = state.account(&account).and_then(|v| v.mail.open.clone());
            match (text(args, "id")?, open) {
                (Some(wanted), Some(open)) if wanted == open.id => {
                    state.print_shown(&account, &open, Some(id));
                }
                (Some(wanted), _) => state.query(account, id, Query::Get { id: wanted }, Then::Print),
                (None, Some(open)) => state.print_shown(&account, &open, Some(id)),
                (None, None) => return Err(Bad(code::NOT_FOUND, crate::label("no-open-email"))),
            }
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
        _ => return Err(Bad(code::UNKNOWN_COMMAND, crate::label("unknown-command"))),
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
        identities: identities(args, "identities")?,
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
                identities: Vec::new(),
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

    #[test]
    fn compose_reply_and_info_expose_the_chips_and_removal_saves_the_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, b"hello").unwrap();
        let mut state = connected_many(&["a"]);
        run(
            &mut state,
            "email.compose",
            json!({"to": "ann@example.invalid", "subject": "Files", "attachments": [path]}),
        );
        let reply = state.take_replies().remove(0);
        assert_eq!(reply.rc, 0);
        let expected = json!({ "path": path, "name": "note.txt", "label": "note.txt", "size": 5, "state": "ready" });
        assert_eq!(reply.body["compose"]["attachments"], json!([expected]));
        run(&mut state, "email.info", json!({}));
        assert_eq!(
            state.take_replies()[0].body["compose"],
            reply.body["compose"]
        );
        let revision = state.compose.as_ref().unwrap().revision;
        run(
            &mut state,
            "email.compose.attachment.remove",
            json!({"path": path}),
        );
        let reply = state.take_replies().remove(0);
        assert_eq!(reply.rc, 0);
        assert_eq!(reply.body["compose"]["attachments"], json!([]));
        assert_eq!(reply.body["compose"]["attachment_size"], 0);
        assert_eq!(reply.body["compose"]["sendable"], true);
        assert_eq!(state.compose.as_ref().unwrap().revision, revision + 1);
        state.save_draft();
        assert!(state.take_draft_ops().iter().any(
            |op| matches!(op, crate::drafts::DraftOp::Save(draft) if draft.attachments.is_empty())
        ));
        run(
            &mut state,
            "email.compose.attachment.remove",
            json!({"path": path}),
        );
        assert_eq!(state.take_replies()[0].body["error_code"], code::NOT_FOUND);
    }

    #[test]
    fn attachment_removal_checks_arguments_open_compose_and_busy_state() {
        let mut state = connected_many(&["a"]);
        for args in [json!({}), json!({"path": 7})] {
            run(&mut state, "email.compose.attachment.remove", args);
            assert_eq!(
                state.take_replies()[0].body["error_code"],
                code::INVALID_ARGUMENT
            );
        }
        run(
            &mut state,
            "email.compose.attachment.remove",
            json!({"path": "/home/user/file"}),
        );
        assert_eq!(state.take_replies()[0].body["error_code"], code::NOT_FOUND);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, b"hello").unwrap();
        run(
            &mut state,
            "email.compose",
            json!({"to": "ann@example.invalid", "subject": "Files", "attachments": [path]}),
        );
        state.take_replies();
        state.apply_ui(crate::view::UiEvent::SendCompose);
        run(
            &mut state,
            "email.compose.attachment.remove",
            json!({"path": path}),
        );
        assert_eq!(state.take_replies()[0].body["error_code"], code::BUSY);
        assert_eq!(state.compose.as_ref().unwrap().draft.attachments, [path]);
    }

    #[test]
    fn both_outgoing_commands_refuse_bad_attachment_paths_and_size_distinctly() {
        let dir = tempfile::tempdir().unwrap();
        let loop_path = dir.path().join("unreadable");
        std::os::unix::fs::symlink(&loop_path, &loop_path).unwrap();
        let oversized = dir.path().join("large.bin");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(jmap::DEFAULT_UPLOAD_LIMIT)
            .unwrap();
        for command_name in ["email.send", "email.compose"] {
            for (path, expected) in [
                (
                    std::path::PathBuf::from("relative.txt"),
                    code::ATTACHMENT_RELATIVE,
                ),
                (dir.path().to_owned(), code::ATTACHMENT_NOT_FILE),
                (dir.path().join("missing"), code::ATTACHMENT_NOT_FILE),
                (loop_path.clone(), code::ATTACHMENT_UNREADABLE),
                (oversized.clone(), code::MESSAGE_TOO_LARGE),
            ] {
                let mut state = connected_many(&["a"]);
                run(
                    &mut state,
                    command_name,
                    json!({"to": "ann@example.invalid", "subject": "Files", "attachments": [path]}),
                );
                let reply = &state.take_replies()[0];
                assert_eq!(
                    (reply.rc, reply.body["error_code"].as_str()),
                    (10, Some(expected)),
                    "{command_name}: {reply:?}"
                );
                assert!(!reply.body["message"].as_str().unwrap().is_empty());
                assert!(state.compose.is_none());
                assert!(state.take_work().is_empty());
            }
            let mut state = connected_many(&["a"]);
            run(
                &mut state,
                command_name,
                json!({"to": "ann@example.invalid", "subject": "Files", "attachments": "relative.txt"}),
            );
            assert_eq!(
                state.take_replies()[0].body["error_code"],
                code::INVALID_ARGUMENT
            );
        }
    }

    #[test]
    fn compose_and_direct_send_keep_symlink_paths_for_the_send() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        let link = dir.path().join("link.txt");
        std::fs::write(&path, b"contents").unwrap();
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let args = json!({"to": "ann@example.invalid", "subject": "Files", "attachments": [link]});
        let mut state = connected_many(&["a"]);
        run(&mut state, "email.compose", args.clone());
        assert_eq!(state.take_replies()[0].rc, 0);
        assert_eq!(
            state.compose.as_ref().unwrap().draft.attachments,
            std::slice::from_ref(&link)
        );
        state.apply_ui(crate::view::UiEvent::SendCompose);
        assert!(state.take_work().iter().any(
            |work| matches!(work, Work::Send { draft, .. } if draft.attachments == [link.clone()])
        ));
        let mut state = connected_many(&["a"]);
        run(&mut state, "email.send", args);
        assert!(
            matches!(queued_write(&mut state), Write::Send { draft } if draft.attachments == [link])
        );
    }

    #[test]
    fn outgoing_commands_use_the_selected_accounts_advertised_limit() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file.txt");
        std::fs::write(&file, b"contents").unwrap();
        for command_name in ["email.send", "email.compose"] {
            let mut state = connected_many(&["a"]);
            state
                .accounts
                .iter_mut()
                .find(|view| view.id() == "a")
                .unwrap()
                .session
                .as_mut()
                .unwrap()
                .max_size_upload = Some(10);
            run(
                &mut state,
                command_name,
                json!({"to": "ann@example.invalid", "subject": "Files", "attachments": [file]}),
            );
            assert_eq!(
                state.take_replies()[0].body["error_code"],
                code::MESSAGE_TOO_LARGE
            );
        }
    }

    fn run(state: &mut State, command_name: &str, args: Value) -> Option<Forward> {
        command(
            state,
            &crate::actions::registry(),
            &crate::strings(),
            1,
            command_name,
            &args.to_string(),
        )
    }

    #[test]
    fn every_command_is_described_once_with_parseable_args() {
        let mut seen = std::collections::BTreeSet::new();
        for command_name in COMMANDS {
            assert!(
                seen.insert(command_name.name),
                "{} twice",
                command_name.name
            );
            let args: Value = serde_json::from_str(command_name.args).expect(command_name.name);
            assert!(args.is_object(), "{}", command_name.name);
        }
        let described = describe();
        let names: Vec<&str> = described["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names.len(),
            COMMANDS.len() + drive::COMMANDS.len() + citizen::macros::describe(APP).len()
        );
    }

    #[test]
    fn every_described_command_is_handled() {
        for command_name in COMMANDS {
            let mut state = connected_many(&["a"]);
            let forward = run(&mut state, command_name.name, json!({}));
            let replies = state.take_replies();
            assert!(
                !replies
                    .iter()
                    .any(|r| r.body["error_code"] == code::UNKNOWN_COMMAND),
                "{}: {replies:?}",
                command_name.name
            );
            if command_name.name == "email.show" {
                assert_eq!(forward, Some(Forward::Show { id: 1 }));
            }
        }
        for command_name in drive::COMMANDS {
            let mut state = connected_many(&["a"]);
            let forward = run(&mut state, &format!("email.{command_name}"), json!({}));
            assert!(
                matches!(forward, Some(Forward::Drive { command: v, .. }) if v == *command_name)
            );
        }
        let mut state = connected_many(&["a"]);
        run(&mut state, "email.nope", json!({}));
        assert_eq!(
            state.take_replies()[0].body["error_code"],
            code::UNKNOWN_COMMAND
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
        let commands = crate::actions::registry();
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
        assert_eq!(replies[1].body["error_code"], code::UNKNOWN_ACTION);
    }

    #[test]
    fn execute_send_uses_the_attachment_gate_and_captures_its_caller() {
        use crate::state::SendOrigin;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, b"hello").unwrap();
        let mut state = connected_many(&["a"]);
        run(
            &mut state,
            "email.compose",
            json!({"to":"ann@example.invalid", "subject":"Files", "attachments":[path]}),
        );
        state.take_replies();
        let origin = SendOrigin {
            caller_key: "mesh:sender@peer".into(),
            drive_request_id: None,
        };
        state.with_send_origin(Some(origin.clone()), |state| {
            run(state, "email.execute", json!({"id":"mail.send"}));
        });
        assert_eq!(state.take_replies()[0].rc, 0);
        assert_eq!(
            state.compose.as_ref().unwrap().send_origin.as_ref(),
            Some(&origin)
        );
        let work = state.take_work();
        let ticket = work
            .iter()
            .find_map(|w| match w {
                Work::Send {
                    origin: captured,
                    ticket,
                    ..
                } if captured == &origin => Some(*ticket),
                _ => None,
            })
            .expect("the accepted send must retain the Bus caller");
        // A later person's retry must not inherit the earlier Bus caller.
        let tag = crate::state::Tag {
            account: "a".into(),
            generation: state.account("a").unwrap().generation(),
        };
        state.apply(crate::shell::Event::Account(
            tag,
            crate::shell::Done::Sent(
                ticket,
                jmap::SendAttempt::default(),
                Err(jmap::Error::Protocol("refused".into())),
            ),
        ));
        state.apply_ui(crate::view::UiEvent::SendCompose);
        assert_eq!(
            state.compose.as_ref().unwrap().send_origin,
            Some(SendOrigin::default())
        );

        for grow in [false, true] {
            let mut state = connected_many(&["a"]);
            state.open_compose(
                Some("a"),
                Composed {
                    to: jmap::parse_addresses("ann@example.invalid").unwrap(),
                    subject: "Files".into(),
                    attachments: vec![path.clone()],
                    ..Composed::default()
                },
            );
            state.take_work();
            // Change the file after the cached action guard said Send was enabled.
            if grow {
                std::fs::File::create(&path)
                    .unwrap()
                    .set_len(jmap::DEFAULT_UPLOAD_LIMIT)
                    .unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            run(&mut state, "email.execute", json!({"id":"mail.send"}));
            let reply = state.take_replies().remove(0);
            assert_ne!(reply.rc, 0, "an unqueued send must be a refusal");
            assert_eq!(reply.body["error_code"], code::DISABLED);
            assert_eq!(
                reply.body["message"],
                state
                    .compose
                    .as_ref()
                    .unwrap()
                    .attachment_error
                    .as_ref()
                    .unwrap()
                    .as_str()
            );
            assert!(
                !state
                    .take_work()
                    .iter()
                    .any(|w| matches!(w, Work::Send { .. }))
            );
            assert!(!state.compose_sendable());
            assert!(state.compose.as_ref().unwrap().attachment_error.is_some());
            std::fs::write(&path, b"hello").unwrap();
        }
    }

    #[test]
    fn every_command_is_reachable_by_execute() {
        let commands = crate::actions::registry();
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
    fn execute_send_refuses_stale_senders_and_unparsable_recipients() {
        for stale in [false, true] {
            let mut state = connected_many(&["a"]);
            state.open_compose(
                Some("a"),
                Composed {
                    to: jmap::parse_addresses("ann@example.invalid").unwrap(),
                    subject: "Files".into(),
                    ..Composed::default()
                },
            );
            let generation = state.account("a").unwrap().generation();
            let compose = state.compose.as_mut().unwrap();
            if stale {
                compose.stale = Some(crate::state::StaleFrom {
                    account: "a".into(),
                    generation,
                    address: "new@example.invalid".into(),
                });
            } else {
                compose.to_text = "invalid-address".into();
            }
            state.take_work();
            run(&mut state, "email.execute", json!({"id":"mail.send"}));
            let reply = state.take_replies().remove(0);
            assert_ne!(reply.rc, 0);
            assert_eq!(reply.body["error_code"], code::DISABLED);
            assert_eq!(
                reply.body["message"],
                state.compose.as_ref().unwrap().error.as_ref().unwrap().as_str()
            );
            assert!(
                !state.take_work().iter().any(|w| matches!(w, Work::Send { .. }))
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
    /// command answers INVALID_ARGUMENT.
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

    /// The write a command queued.
    fn queued_write(state: &mut State) -> Write {
        match state.take_work().as_slice() {
            [
                Work::Write {
                    write,
                    reply: Some(1),
                    ..
                },
            ] => write.clone(),
            other => panic!("{other:?}"),
        }
    }

    fn many(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("E{i}")).collect()
    }

    #[test]
    fn message_commands_send_one_email_set_per_500_ids() {
        let ids = many(1_201);
        let cases = [
            ("email.mark", json!({"ids": ids, "flagged": true}), "update"),
            ("email.move", json!({"ids": ids, "mailbox": "M2"}), "update"),
            ("email.delete", json!({"ids": ids}), "update"),
            (
                "email.delete",
                json!({"ids": ids, "destroy": true}),
                "destroy",
            ),
        ];
        for (command_name, args, key) in cases {
            let mut state = connected_many(&["a"]);
            run(&mut state, command_name, args);
            let write = queued_write(&mut state);
            let (named, change) = batch_change(&write, Some("M-trash")).unwrap();
            assert_eq!(named, ids.as_slice(), "{command_name}");
            let calls = jmap::email_set_calls("A1", named, &change);
            let sizes: Vec<usize> = calls
                .iter()
                .map(|c| match &c.1[key] {
                    Value::Object(map) => map.len(),
                    Value::Array(list) => list.len(),
                    other => panic!("{command_name}: {other}"),
                })
                .collect();
            assert_eq!(sizes, [500, 500, 201], "{command_name} {key}");
            assert!(calls.iter().all(|c| c.0 == "Email/set"));
        }
        // A delete to Trash needs the Trash mailbox; a destroy does not.
        let delete = Write::Delete {
            ids: many(1),
            destroy: false,
        };
        assert!(batch_change(&delete, None).is_err());
        let (_, change) = batch_change(&delete, Some("M-trash")).unwrap();
        assert_eq!(
            change,
            jmap::EmailChange::Move {
                mailbox: "M-trash".into()
            }
        );
    }

    #[test]
    fn account_set_reads_identities_as_a_list_and_refuses_anything_else() {
        let args = json!({"identities": [
            {"id": "work", "name": "Ann", "address": "ann@work.example.invalid", "reply_to": null}
        ]});
        let Ok(Some(read)) = identities(&args, "identities") else {
            panic!("a list of identities reads");
        };
        assert_eq!(read[0].id, "work");
        assert_eq!(read[0].reply_to, None);
        assert!(matches!(identities(&json!({}), "identities"), Ok(None)));
        let null = json!({"identities": null});
        assert!(matches!(identities(&null, "identities"), Ok(None)));
        let text = json!({"identities": "work"});
        assert!(identities(&text, "identities").is_err());
        let no_address = json!({"identities": [{"id": "work"}]});
        assert!(identities(&no_address, "identities").is_err());
    }

    #[test]
    fn a_partial_failure_answers_each_id_and_keeps_the_old_fields() {
        let outcome = jmap::SetOutcome {
            done: vec!["E0".into(), "E2".into()],
            failed: vec![("E1".into(), "notFound: no such email".into())],
        };
        let write = Write::Move {
            ids: many(3),
            mailbox: "M2".into(),
        };
        let answer = batch_answer(&write, outcome).expect("one bad id never fails the call");
        assert_eq!(answer["ids"], json!(["E0", "E1", "E2"]));
        assert_eq!(answer["mailbox"], "M2");
        assert_eq!(answer["done"], json!(["E0", "E2"]));
        assert_eq!(
            answer["failed"],
            json!([{"id": "E1", "reason": "notFound: no such email"}])
        );
        assert_eq!(answer["partial"], true);
        let delete = Write::Delete {
            ids: many(1),
            destroy: true,
        };
        let ok = jmap::SetOutcome {
            done: many(1),
            failed: vec![],
        };
        let answer = batch_answer(&delete, ok).unwrap();
        assert_eq!(answer["destroyed"], true);
        assert_eq!(answer["failed"], json!([]));
        assert_eq!(answer["partial"], false);
    }

    #[test]
    fn only_a_call_where_every_id_failed_is_refused() {
        let mark = Write::Keywords {
            ids: many(1),
            read: Some(true),
            flagged: None,
        };
        let gone = jmap::SetOutcome {
            done: vec![],
            failed: vec![("E0".into(), "notFound".into())],
        };
        let error = batch_answer(&mark, gone).unwrap_err();
        assert_eq!(
            server_refusal(&error).0,
            code::NOT_FOUND,
            "as a single id was"
        );
        let down = jmap::SetOutcome {
            done: vec![],
            failed: vec![
                ("E0".into(), "notFound".into()),
                ("E1".into(), "HTTP 502".into()),
            ],
        };
        let error = batch_answer(&mark, down).unwrap_err();
        assert_eq!(server_refusal(&error).0, code::SERVER);
        assert!(error.to_string().contains("E1 (HTTP 502)"));
    }
}
