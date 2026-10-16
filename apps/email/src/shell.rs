// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The running window: feeds the state its events and carries out its
//! work. JMAP calls run on a small tokio runtime, one client per account;
//! each completion is sent back as an [`Event`] with a repaint request, so
//! the window wakes only when something happened (no polling). The Bus
//! connection carries the session-profile topic (theme-follow and the
//! `email.*` accounts), the settingsd writes, and single-instance
//! activation.
//!
//! This is the app's one architectural exception: the effects runtime
//! speaks HTTPS itself, through the `jmap` engine, because a pure JMAP
//! client's server is whatever account the person pointed it at.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::task::Poll;

use futures::StreamExt;
use jmap::{Client, EmailPage};
use serde_json::{Value, json};
use toolkit::command::Origin;
use toolkit::{Registry, Strings, Theme, drive};

use crate::attachments::{self, Target};
use crate::commands;
use crate::secrets::{Secrets, SecretsFile};
use crate::state::{Mode, SessionInfo, Settled, State, Tag, UiState, code};
use crate::verbs::{Forward, Then, Write};
use crate::view::{UiEvent, view};
use crate::work::{Login, Work};
use crate::{APP, APP_ID, label, label_with};

/// One account's answer, addressed by [`Tag`] in [`Event::Account`]. The
/// tickets are the requests' (see [`crate::work::Work`]).
pub enum Done {
    Mailboxes(Result<Vec<jmap::Mailbox>, jmap::Error>),
    Page(u64, Result<EmailPage, jmap::Error>),
    /// One email with its bodies, and the Bus request that asked for it;
    /// boxed — a full [`jmap::Email`] is large.
    Opened(u64, Option<u64>, Box<Result<jmap::Email, jmap::Error>>),
    /// A Bus query's answer, for request `reply`.
    Answered(u64, Then, Result<Value, jmap::Error>),
    /// A server write finished (`reply`: the Bus request, if any).
    Wrote(
        Option<u64>,
        Option<u64>,
        Box<Write>,
        Result<Value, jmap::Error>,
    ),
    Marked(Result<(), jmap::Error>),
    Sent(u64, Result<(), jmap::Error>),
    Calendars(Result<Vec<jmap::Calendar>, jmap::Error>),
    Events(u64, Result<Vec<jmap::CalendarEvent>, jmap::Error>),
    Books(Result<Vec<jmap::AddressBook>, jmap::Error>),
    Contacts(u64, Result<Vec<jmap::Contact>, jmap::Error>),
}

/// What finished off the UI thread. Completions carry their result; the
/// shell resolves the Look pair itself (theme-follow, below).
pub enum Event {
    /// A Bus delivery.
    Delivery(citizen::Delivery),
    /// An account's connect finished. The client stays in the shell; the
    /// state gets the summary.
    Connected(Tag, Result<SessionInfo, jmap::Error>),
    /// An account's server answered.
    Account(Tag, Done),
    /// settingsd answered write `ticket`.
    Settled(u64, Settled),
    /// The secrets file was written (or refused) for save `ticket`.
    SecretsSaved(u64, u64, Result<bool, String>),
    /// Settings write `op`'s deadline passed.
    WriteDeadline(u64),
    /// A read of the session profile (settingsd), for the shell.
    Look(u64, Result<citizen::Reply, citizen::CallError>),
    /// A retry of the session look after a transient fault (its generation).
    LookRetry(u64),
    /// `email.show` (request `id`) finished.
    Shown(u64, Result<Value, String>),
    /// Open Email Settings finished; an error is for the status line.
    PrefsOpened(Result<(), String>),
    /// A print page was written and handed to the browser (its path), for
    /// Bus request `reply` if one asked.
    Printed(Option<u64>, Result<String, String>),
    /// Attachment download `ticket` finished (`true`: it was to open),
    /// with the file's path, for Bus request `reply` if one asked.
    Attached(u64, Option<u64>, bool, Result<String, String>),
}

pub struct Shell {
    state: State,
    commands: Registry<State>,
    strings: Strings,
    theme: Theme,
    bus: citizen::Handle,
    runtime: tokio::runtime::Runtime,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    ctx: egui::Context,
    /// Each connected account's client, with its connection generation.
    clients: HashMap<String, (u64, Client)>,
    /// The newest connect asked for, per account: only its client is kept.
    latest: HashMap<String, u64>,
    /// Where a finished connect parks its client for the UI thread to
    /// adopt; the event channel carries only the summary.
    adopted: Arc<Mutex<Vec<(String, u64, Client)>>>,
    exiting: bool,
    /// The theme choice installed last.
    installed: toolkit::theme_menu::Choice,
    /// The theme file's session theme: the fallback while settingsd is
    /// absent, or holds a look this build cannot draw.
    file_theme: Theme,
    /// The session look settingsd holds, once read.
    look: Option<settings::Appearance>,
    /// The profile's bindings for email, as last applied.
    keymap: toolkit::keymap::Overrides,
    follower: settings::follow::Follower,
    /// The latest look retry; an older one firing is void.
    look_retry: u64,
    config: Option<crate::config::ConfigFile>,
    /// The secrets saver: a serial worker thread that writes each snapshot
    /// in order, off the UI thread (a stalled disk never freezes the window).
    saver: Option<mpsc::Sender<SaveJob>>,
    /// The saver's thread, joined (bounded) on close so a queued save lands.
    saver_thread: Option<std::thread::JoinHandle<()>>,
    /// Settings writes sent, for unique operation ids.
    applies: u64,
    /// Bus commands waiting behind drive input accepted before them.
    held: std::collections::VecDeque<(u64, String, String)>,
    /// The compositor's Bus service, for `email.show`.
    comp: String,
    /// `email.show` requests forwarded to the compositor, not yet answered.
    shows: std::collections::BTreeSet<u64>,
    /// Attachment downloads running, by ticket: aborted when the window
    /// closes.
    downloads: HashMap<u64, tokio::task::AbortHandle>,
}

/// This host's name: the settingsd instance to try first (settingsd runs
/// as `--instance %H`; the follower adopts the right one from its answer).
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}

/// Prefs' app id, as `apps.launch` takes it.
const PREFS_APP_ID: &str = "dev.mixos.prefs";

/// How long one settings read or write may take.
const LOOK_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

type Deliveries = futures::channel::mpsc::Receiver<citizen::Delivery>;

fn locked(inbox: &Mutex<Deliveries>) -> std::sync::MutexGuard<'_, Deliveries> {
    inbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What a settingsd apply answered.
fn settled(result: Result<citizen::Reply, citizen::CallError>) -> Settled {
    let reply = match result {
        Ok(reply) => reply,
        Err(error) if error.outcome_unknown => return Settled::Unknown(error.message),
        Err(error) => return Settled::Unavailable(error.message),
    };
    if reply.body.trim().is_empty() {
        return Settled::Unavailable(
            reply
                .error
                .unwrap_or_else(|| format!("refused by the broker (rc {})", reply.rc)),
        );
    }
    let value: Value = serde_json::from_str(&reply.body).unwrap_or(Value::String(reply.body));
    if reply.rc == 0 {
        return Settled::Accepted;
    }
    match value["status"].as_str() {
        Some("conflict") => Settled::Conflict,
        Some("outcome_unknown") => Settled::Unknown(value.to_string()),
        status => Settled::Refused(
            value["message"]
                .as_str()
                .or(status)
                .map_or_else(|| value.to_string(), str::to_owned),
        ),
    }
}

/// The schema-1 account as a settings account: its id derived from its
/// identity, named by its address, and its inline CA bundle written beside
/// the config (`ca-<id>.pem`, 0600) so settingsd can name it by path.
fn legacy_entry(
    account: &jmap::Account,
    dir: &Path,
) -> Result<(settings::email::AccountEntry, bool), String> {
    let (url, stripped) = sanitised_url(account.url.trim());
    let email = account.email.trim().to_owned();
    let id = crate::writer::legacy_id(&url, &email);
    let ca_path = match &account.ca_pem {
        Some(pem) => {
            let path = dir.join(format!("ca-{id}.pem"));
            crate::config::write_private(&path, pem.as_bytes())?;
            Some(path.to_string_lossy().into_owned())
        }
        None => None,
    };
    let name = if settings::email::valid_name(&email) {
        email.clone()
    } else {
        label("title")
    };
    Ok((
        settings::email::AccountEntry {
            id,
            name,
            url,
            email,
            ca_path,
            enabled: true,
            signature: String::new(),
        },
        stripped,
    ))
}

/// The legacy URL as an account may hold it: no userinfo, and no query or
/// fragment (settings refuses both, and a query can carry credentials too:
/// `?password=…`), no trailing slash. Whether anything was removed comes
/// back as well; the schema-1 source itself is left as it was.
pub fn sanitised_url(url: &str) -> (String, bool) {
    let (url, mut stripped) = without_userinfo(url);
    let end = url.find(['?', '#']).unwrap_or(url.len());
    stripped |= end < url.len();
    (url[..end].trim_end_matches('/').to_owned(), stripped)
}

/// `url` without credentials in its authority (`https://user:pw@host`):
/// they would be published in settingsd's retained snapshots. Whether any
/// were removed comes back too; the URL itself is never printed.
pub fn without_userinfo(url: &str) -> (String, bool) {
    let Some((scheme, rest)) = url.split_once("://") else {
        return (url.to_owned(), false);
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    match authority.rsplit_once('@') {
        Some((_, host)) => (format!("{scheme}://{host}{tail}"), true),
        None => (url.to_owned(), false),
    }
}

/// One save for the saver: its ticket, the snapshot version, the passwords.
type SaveJob = (u64, u64, Secrets);

/// The saver's queue and its thread.
type Saver = (mpsc::Sender<SaveJob>, Option<std::thread::JoinHandle<()>>);

/// The secrets saver: one thread, so saves land in the order they were
/// asked for; each answers with the snapshot version it wrote.
fn saver(file: SecretsFile, events: mpsc::Sender<Event>, wake: egui::Context) -> Saver {
    let (tx, rx) = mpsc::channel::<SaveJob>();
    let spawned = std::thread::Builder::new()
        .name("email-secrets".into())
        .spawn(move || {
            while let Ok((ticket, version, snapshot)) = rx.recv() {
                let result = file.save(&snapshot);
                if let Err(error) = &result {
                    eprintln!("email: {error}");
                }
                if events
                    .send(Event::SecretsSaved(ticket, version, result))
                    .is_err()
                {
                    return;
                }
                wake.request_repaint();
            }
        });
    let thread = match spawned {
        Ok(thread) => Some(thread),
        Err(error) => {
            eprintln!("email: no secrets saver: {error}");
            None
        }
    };
    (tx, thread)
}

/// What closing does with an event that arrived during its drain.
#[derive(Debug, PartialEq)]
enum Closing {
    /// A completion: applied, so whoever waits on it hears.
    Apply,
    /// A Bus command arriving while closing: answered CANCELLED.
    Cancel(u64),
    /// A Show finishing: answered with its result.
    Shown,
    /// Nothing waits on it.
    Ignore,
}

fn closing(event: &Event) -> Closing {
    match event {
        Event::Connected(..)
        | Event::Account(..)
        | Event::Settled(..)
        | Event::SecretsSaved(..)
        | Event::WriteDeadline(_)
        | Event::Printed(..)
        | Event::Attached(..) => Closing::Apply,
        Event::Delivery(citizen::Delivery::Command { id, .. }) => Closing::Cancel(*id),
        Event::Shown(..) => Closing::Shown,
        Event::Delivery(_) | Event::Look(..) | Event::LookRetry(_) | Event::PrefsOpened(_) => {
            Closing::Ignore
        }
    }
}

/// How long closing waits for the secrets saver to finish its queue.
const SAVER_DRAIN: std::time::Duration = std::time::Duration::from_secs(5);

/// The state a launch starts in, from the app's config directory: the
/// window's shape, the accounts cache, the passwords, and a schema-1
/// account still to migrate. Without a config directory nothing persists.
pub fn start(
    dir: Option<&Path>,
) -> (
    State,
    Option<crate::config::ConfigFile>,
    Option<SecretsFile>,
) {
    let Some(dir) = dir else {
        eprintln!("email: no config directory; nothing is persisted");
        let state = State::new(
            UiState::default(),
            Mode::Mail,
            None,
            settings::email::Email::default(),
            Secrets::default(),
        );
        return (state, None, None);
    };
    let (loaded, config) = crate::config::ConfigFile::load(dir);
    let (secrets, secrets_file) = SecretsFile::load(dir);
    let ui = UiState {
        places: loaded.config.ui.places,
        reader: loaded.config.ui.reader,
        expanded: loaded.config.ui.expanded.clone(),
        list: loaded.config.ui.list.clone(),
        ..UiState::default()
    };
    let mut state = State::new(
        ui,
        loaded.config.ui.mode,
        loaded.config.ui.account.clone(),
        loaded.config.cache.clone(),
        secrets,
    );
    if let Some(legacy) = &loaded.legacy {
        match legacy_entry(legacy, dir) {
            Ok((entry, stripped)) => {
                state.begin_migration(entry, &legacy.password);
                if stripped {
                    eprintln!("email: removed credentials from the old account's server URL");
                    state.status = label("migration-url-credentials");
                }
            }
            Err(error) => eprintln!("email: the old account cannot migrate yet: {error}"),
        }
    }
    (state, Some(config), Some(secrets_file))
}

impl Shell {
    /// A window on `bus`, with its deliveries forwarded from `deliveries`.
    pub fn new(
        ctx: egui::Context,
        theme: Theme,
        bus: citizen::Handle,
        deliveries: Deliveries,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("email-effects")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        drive::install(&ctx, APP);
        let (tx, rx) = mpsc::channel();
        let forward = tx.clone();
        let wake = ctx.clone();
        // The forwarder owns the deliveries from here.
        let source = Arc::new(Mutex::new(deliveries));
        runtime.spawn(async move {
            loop {
                // Take one delivery and hand it on under the inbox lock.
                let next =
                    futures::future::poll_fn(|cx| match locked(&source).poll_next_unpin(cx) {
                        Poll::Ready(Some(delivery)) => {
                            Poll::Ready(Some(forward.send(Event::Delivery(delivery)).is_ok()))
                        }
                        Poll::Ready(None) => Poll::Ready(None),
                        Poll::Pending => Poll::Pending,
                    })
                    .await;
                match next {
                    Some(true) => wake.request_repaint(),
                    Some(false) => return,
                    None => break,
                }
            }
            let _ = forward.send(Event::Delivery(citizen::Delivery::Disconnected));
            wake.request_repaint();
        });
        let dir: Option<PathBuf> = config::AppDirs::resolve(APP).map(|dirs| dirs.config());
        let (state, config, secrets) = start(dir.as_deref());
        let (saver, saver_thread) = match secrets.map(|file| saver(file, tx.clone(), ctx.clone())) {
            Some((saver, thread)) => (Some(saver), thread),
            None => (None, None),
        };
        let mut shell = Self {
            state,
            commands: commands::registry(),
            strings: crate::strings(),
            theme: theme.clone(),
            bus,
            runtime,
            tx,
            rx,
            ctx,
            clients: HashMap::new(),
            latest: HashMap::new(),
            adopted: Arc::new(Mutex::new(Vec::new())),
            exiting: false,
            installed: toolkit::theme_menu::Choice::default(),
            file_theme: theme,
            look: None,
            keymap: toolkit::keymap::Overrides::new(),
            follower: settings::follow::Follower::new(&hostname()),
            look_retry: 0,
            config,
            saver,
            saver_thread,
            applies: 0,
            held: std::collections::VecDeque::new(),
            comp: std::env::var("MIXOS_COMP_SERVICE").unwrap_or_else(|_| "comp".into()),
            shows: std::collections::BTreeSet::new(),
            downloads: HashMap::new(),
        };
        shell.read_look();
        shell.settle();
        Ok(shell)
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }

    /// Spawn `work` and send its result back as an event; the handle aborts
    /// it.
    fn spawn<F>(&self, work: F) -> tokio::task::AbortHandle
    where
        F: Future<Output = Event> + Send + 'static,
    {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.runtime
            .spawn(async move {
                let _ = tx.send(work.await);
                ctx.request_repaint();
            })
            .abort_handle()
    }

    /// Perform the state's queued work until none is left (a save answers
    /// at once and may queue more). A connect that finished parks its
    /// client for this thread first, so the fetches a connect schedules
    /// find it there.
    fn settle(&mut self) {
        let parked = std::mem::take(&mut *self.adopted.lock().unwrap());
        for (account, generation, client) in parked {
            if self.latest.get(&account) == Some(&generation) {
                self.clients.insert(account, (generation, client));
            }
        }
        loop {
            let work = self.state.take_work();
            if work.is_empty() {
                break;
            }
            for work in work {
                self.perform(work);
            }
        }
        for reply in self.state.take_replies() {
            self.bus.reply(reply.id, reply.rc, reply.body);
        }
        self.publish_executed();
        self.save_config_if_dirty();
    }

    /// Publish the commands run since the last call on
    /// `<service>.command.executed`.
    fn publish_executed(&self) {
        let (runs, lost) = self.commands.take_executed();
        if runs.is_empty() && lost == 0 {
            return;
        }
        let topic = toolkit::executed::topic(self.bus.service());
        let bodies = runs.iter().map(toolkit::executed::Executed::body).collect();
        self.bus.publish(&topic, bodies, lost);
    }

    /// `account`'s client and its generation, or a note on the status line
    /// when there is none (the token died, the app reconnected, the connect
    /// failed). The clone is cheap: the HTTP pool and the token cell are
    /// shared.
    fn client(&mut self, account: &str) -> Option<(Tag, Client)> {
        match self.clients.get(account) {
            Some((generation, client)) => Some((
                Tag {
                    account: account.to_owned(),
                    generation: *generation,
                },
                client.clone(),
            )),
            None => {
                self.state.status = label("not-connected");
                self.ctx.request_repaint();
                None
            }
        }
    }

    /// A Bus request whose account has no client: refused, never left
    /// unanswered.
    fn not_connected(&mut self, reply: Option<u64>) {
        if let Some(reply) = reply {
            self.state
                .refuse(reply, code::NOT_CONNECTED, &label("not-connected"));
        }
    }

    /// Perform one unit of work on the effects runtime.
    fn perform(&mut self, work: Work) {
        match work {
            Work::Connect {
                account,
                generation,
                login,
            } => {
                self.clients.remove(&account);
                self.latest.insert(account.clone(), generation);
                let adopted = self.adopted.clone();
                let tag = Tag {
                    account: account.clone(),
                    generation,
                };
                self.spawn(async move {
                    let connected = match connect(&login).await {
                        Ok(client) => {
                            adopted
                                .lock()
                                .unwrap()
                                .push((account, generation, client.clone()));
                            Ok(SessionInfo {
                                account_name: client
                                    .session()
                                    .account_name
                                    .clone()
                                    .unwrap_or_else(|| login.email.clone()),
                                account_id: client.account_id().to_owned(),
                            })
                        }
                        Err(error) => Err(error),
                    };
                    Event::Connected(tag, connected)
                });
            }
            Work::Disconnect { account } => {
                self.clients.remove(&account);
                self.latest.remove(&account);
            }
            Work::FetchMailboxes { account } => {
                if let Some((tag, client)) = self.client(&account) {
                    self.spawn(async move {
                        Event::Account(tag, Done::Mailboxes(client.mailboxes().await))
                    });
                }
            }
            Work::FetchEmails {
                account,
                ticket,
                mailbox,
                limit,
                position,
                sort,
            } => {
                if let Some((tag, client)) = self.client(&account) {
                    self.spawn(async move {
                        let page = client
                            .email_query_sorted(
                                Some(&mailbox),
                                None,
                                sort,
                                position,
                                u64::from(limit),
                            )
                            .await;
                        Event::Account(tag, Done::Page(ticket, page))
                    });
                }
            }
            Work::OpenEmail {
                account,
                ticket,
                id,
                reply,
            } => match self.client(&account) {
                Some((tag, client)) => {
                    self.spawn(async move {
                        let email = client.email(&id).await;
                        Event::Account(tag, Done::Opened(ticket, reply, Box::new(email)))
                    });
                }
                None => self.not_connected(reply),
            },
            Work::Query {
                account,
                reply,
                query,
                then,
            } => match self.client(&account) {
                Some((tag, client)) => {
                    self.spawn(async move {
                        let answer = crate::verbs::run_query(&client, query).await;
                        Event::Account(tag, Done::Answered(reply, then, answer))
                    });
                }
                None => self.not_connected(Some(reply)),
            },
            Work::Write {
                account,
                reply,
                form,
                expect,
                write,
            } => {
                let sent = self.state.sent_mailbox_id(&account);
                match self.client(&account) {
                    // A form's write through a connection other than the one
                    // it was opened on is refused as stale.
                    Some((tag, _)) if expect.is_some_and(|g| g != tag.generation) => {
                        let error = jmap::Error::Protocol(label("stale-connection"));
                        self.state.wrote(&tag, reply, form, write, Err(error));
                    }
                    Some((tag, client)) => {
                        self.spawn(async move {
                            let answer =
                                crate::verbs::run_write(&client, write.clone(), sent).await;
                            Event::Account(tag, Done::Wrote(reply, form, Box::new(write), answer))
                        });
                    }
                    // Settled at once, so neither a caller nor a form waits
                    // forever on a write that never went.
                    None => {
                        let generation = self
                            .state
                            .account(&account)
                            .map_or(0, crate::state::AccountView::generation);
                        let tag = Tag {
                            account,
                            generation,
                        };
                        let error = jmap::Error::Protocol(label("not-connected"));
                        self.state.wrote(&tag, reply, form, write, Err(error));
                    }
                }
            }
            Work::MarkRead { account, id, read } => {
                if let Some((tag, client)) = self.client(&account) {
                    self.spawn(async move {
                        Event::Account(tag, Done::Marked(client.mark_read(&id, read).await))
                    });
                }
            }
            Work::Send {
                account,
                ticket,
                draft,
            } => {
                // The send recipe needs the Sent mailbox; without the mail
                // listing yet, there is nothing to file under. Either way
                // the compose hears back, so it never stays busy.
                let sent = self.state.sent_mailbox_id(&account);
                match (self.client(&account), sent) {
                    (Some((tag, client)), Some(sent)) => {
                        self.spawn(async move {
                            Event::Account(
                                tag,
                                Done::Sent(ticket, client.send(&draft, &sent).await),
                            )
                        });
                    }
                    (found, _) => {
                        let generation = self
                            .state
                            .account(&account)
                            .map_or(0, crate::state::AccountView::generation);
                        let tag = found.map(|(tag, _)| tag).unwrap_or(Tag {
                            account,
                            generation,
                        });
                        let error = jmap::Error::Protocol(label("no-sent-mailbox"));
                        self.state
                            .apply(Event::Account(tag, Done::Sent(ticket, Err(error))));
                    }
                }
            }
            Work::FetchCalendars { account } => {
                if let Some((tag, client)) = self.client(&account) {
                    self.spawn(async move {
                        Event::Account(tag, Done::Calendars(client.calendars().await))
                    });
                }
            }
            Work::FetchEvents {
                account,
                ticket,
                calendar,
            } => {
                if let Some((tag, client)) = self.client(&account) {
                    self.spawn(async move {
                        let events = client.calendar_events(&calendar).await;
                        Event::Account(tag, Done::Events(ticket, events))
                    });
                }
            }
            Work::FetchBooks { account } => {
                if let Some((tag, client)) = self.client(&account) {
                    self.spawn(async move {
                        Event::Account(tag, Done::Books(client.address_books().await))
                    });
                }
            }
            Work::FetchContacts {
                account,
                ticket,
                book,
                filter,
            } => {
                if let Some((tag, client)) = self.client(&account) {
                    self.spawn(async move {
                        let contacts = client.contacts(&book, &filter).await;
                        Event::Account(tag, Done::Contacts(ticket, contacts))
                    });
                }
            }
            Work::Settings { ticket, changes } => {
                self.applies += 1;
                let operation = format!(
                    "email-{}-{}-{}",
                    std::process::id(),
                    self.applies,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis())
                );
                match self.follower.apply(&operation, changes) {
                    None => self
                        .state
                        .settled(ticket, Settled::Unavailable(label("settings-missing"))),
                    Some(apply) => {
                        let bus = self.bus.clone();
                        self.spawn(async move {
                            let result = bus
                                .raw_within(
                                    apply.service,
                                    apply.verb,
                                    apply.body.to_string(),
                                    LOOK_BUDGET,
                                )
                                .await;
                            Event::Settled(ticket, settled(result))
                        });
                    }
                }
            }
            Work::ReadSettings => self.read_look(),
            Work::WriteDeadline { op } => {
                self.spawn(async move {
                    tokio::time::sleep(crate::work::WRITE_DEADLINE).await;
                    Event::WriteDeadline(op)
                });
            }
            Work::SaveSecrets { ticket } => {
                let (version, snapshot) = self.state.secrets_snapshot();
                let queued = match &self.saver {
                    Some(saver) => saver.send((ticket, version, snapshot)).is_ok(),
                    None => {
                        // Nowhere to save: nothing persists this run.
                        self.state.secrets_saved(ticket, version, Ok(true));
                        true
                    }
                };
                if !queued {
                    let error = label("secrets-saver-gone");
                    self.state.secrets_saved(ticket, version, Err(error));
                }
            }
            Work::OpenPrefs => self.open_prefs(),
            Work::Print { html, reply } => {
                let bus = self.bus.clone();
                self.spawn(async move {
                    let written =
                        tokio::task::spawn_blocking(move || crate::print::write_page(&html))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()));
                    let result = match written {
                        Ok(path) => launch(&bus, crate::print::BROWSER, &path)
                            .await
                            .map(|()| path.display().to_string())
                            .map_err(|_| {
                                let path = path.display().to_string();
                                label_with("print-no-browser", &[("path", &path)])
                            }),
                        Err(error) => Err(error),
                    };
                    Event::Printed(reply, result)
                });
            }
            Work::Attachment {
                account,
                ticket,
                blob,
                name,
                index,
                mime,
                target,
                reply,
            } => {
                let open = target == Target::Open;
                let Some((_, client)) = self.client(&account) else {
                    let error = label("not-connected");
                    self.state
                        .apply(Event::Attached(ticket, reply, open, Err(error)));
                    return;
                };
                let bus = self.bus.clone();
                let handle = self.spawn(async move {
                    let result = attachment(&client, &bus, &blob, name, index, mime, target).await;
                    Event::Attached(ticket, reply, open, result)
                });
                self.downloads.insert(ticket, handle);
            }
        }
    }

    /// Show Prefs on its Email section: ask a running Prefs, else have the
    /// session's launcher (`apps.launch`, as Prefs launches Email) start it
    /// and ask again once it registers. The app has no launcher of its own:
    /// without one, the status line says so.
    fn open_prefs(&mut self) {
        let bus = self.bus.clone();
        self.spawn(async move {
            let section = json!({"name": "email"});
            let asked = bus.call("prefs", "prefs.section", section.clone()).await;
            if asked.as_ref().is_ok_and(|r| r.rc == 0) {
                return Event::PrefsOpened(Ok(()));
            }
            let launched = bus
                .call("apps", "apps.launch", json!({"id": PREFS_APP_ID}))
                .await;
            if !launched.as_ref().is_ok_and(|r| r.rc == 0) {
                return Event::PrefsOpened(Err(label("no-launcher")));
            }
            // A launch registers within seconds; ask until it answers.
            for _ in 0..40 {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let asked = bus.call("prefs", "prefs.section", section.clone()).await;
                if asked.is_ok_and(|r| r.rc == 0) {
                    return Event::PrefsOpened(Ok(()));
                }
            }
            Event::PrefsOpened(Err(label("prefs-silent")))
        });
    }

    /// Apply every finished event, settling after each one, so a later
    /// completion can never change what an earlier one acted on.
    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            if let Event::Look(ticket, result) = event {
                self.looked(ticket, result);
                self.settle();
                continue;
            }
            if let Event::LookRetry(generation) = event {
                self.look_retry_due(generation);
                continue;
            }
            if let Event::Delivery(citizen::Delivery::Topic(topic)) = &event {
                if *topic == self.follower.topic() {
                    self.read_look();
                }
                continue;
            }
            // settingsd may have come (or come back): read the look again.
            if let Event::Delivery(citizen::Delivery::Connected | citizen::Delivery::Changed) =
                event
            {
                self.read_look();
                continue;
            }
            if let Event::Delivery(citizen::Delivery::Command { id, verb, body }) = event {
                if self.exiting {
                    self.refuse_closing(id);
                } else {
                    self.held.push_back((id, verb, body));
                    self.release();
                }
                continue;
            }
            if let Event::PrefsOpened(result) = event {
                if let Err(error) = result {
                    self.state.status = error;
                }
                continue;
            }
            if let Event::Shown(id, result) = event {
                self.shown(id, result);
                continue;
            }
            if let Event::Attached(ticket, ..) = &event {
                self.downloads.remove(ticket);
            }
            self.state.apply(event);
            self.settle();
        }
        self.release();
    }

    /// Closing: no more saves are queued; the saver finishes the ones it
    /// has (waiting at most [`SAVER_DRAIN`]) and their answers are applied,
    /// so a password the caller was told about is on disk.
    fn drain_saver(&mut self) {
        self.saver = None;
        if let Some(thread) = self.saver_thread.take() {
            let deadline = std::time::Instant::now() + SAVER_DRAIN;
            while !thread.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                eprintln!("email: the secrets saver did not finish in time");
            }
        }
        // Nothing that arrived meanwhile is dropped: completions are
        // applied (their callers hear), a command is cancelled, a Show is
        // answered; then any Show still waiting is cancelled.
        while let Ok(event) = self.rx.try_recv() {
            match closing(&event) {
                Closing::Apply => self.state.apply(event),
                Closing::Cancel(id) => self.bus.reply(
                    id,
                    10,
                    crate::state::refusal(code::CANCELLED, &label("closing-cancelled")),
                ),
                Closing::Shown => {
                    if let Event::Shown(id, result) = event {
                        self.shown(id, result);
                    }
                }
                Closing::Ignore => {}
            }
        }
        for id in std::mem::take(&mut self.shows) {
            self.bus.reply(
                id,
                10,
                crate::state::refusal(code::CANCELLED, &label("closing-cancelled")),
            );
        }
    }

    /// `email.show` (request `id`) finished: its caller hears.
    fn shown(&mut self, id: u64, result: Result<Value, String>) {
        if !self.shows.remove(&id) {
            return;
        }
        match result {
            Ok(body) => self.bus.reply(id, 0, body),
            Err(error) => self
                .bus
                .reply(id, 10, crate::state::refusal("SHOW", &error)),
        }
    }

    fn refuse_closing(&self, id: u64) {
        self.bus.reply(
            id,
            10,
            crate::state::refusal(code::BUSY, &label("quitting")),
        );
    }

    /// Run held Bus commands in order. Input that only joins the drive
    /// queue goes straight on; anything else waits until the drive input
    /// before it is answered, so a caller's clicks and its later verbs act
    /// in the order sent.
    fn release(&mut self) {
        while let Some((_, verb, _)) = self.held.front() {
            let joins = verb.strip_prefix("email.").is_some_and(drive::queues);
            if self.exiting || (drive::pending(&self.ctx) && !joins) {
                return;
            }
            let (id, verb, body) = self.held.pop_front().expect("a held command");
            self.command(id, &verb, &body);
        }
    }

    /// One Bus command: the verbs layer answers or queues it; the drive
    /// verbs and the window's activation are the shell's.
    fn command(&mut self, id: u64, verb: &str, body: &str) {
        let forward = crate::verbs::command(
            &mut self.state,
            &self.commands,
            &self.strings,
            id,
            verb,
            body,
        );
        match forward {
            Some(Forward::Drive { id, verb, args }) => {
                if let Some(answer) = drive::request(&self.ctx, id, &verb, &args) {
                    self.bus.reply(answer.id, answer.rc, answer.body);
                }
            }
            Some(Forward::Show { id }) => {
                self.shows.insert(id);
                let (bus, comp) = (self.bus.clone(), self.comp.clone());
                self.spawn(async move { Event::Shown(id, citizen::show(bus, comp, APP_ID).await) });
            }
            None => {}
        }
        self.settle();
    }

    /// Persist the config when the state says it changed, and rewrite it as
    /// schema 2 once the schema-1 account has migrated.
    fn save_config_if_dirty(&mut self) {
        // The migration's schema-2 rewrite: confirmed, or the schema-1 form
        // is kept and the migration stays unfinished (and locked).
        if self.state.take_migrated() {
            let result = match &mut self.config {
                Some(file) => {
                    let legacy = file.migrated();
                    let result = file.save(&crate::config::EmailConfig::from(&self.state));
                    if result != Ok(true) {
                        file.restore(legacy);
                    }
                    result
                }
                None => Ok(true),
            };
            if let Err(error) = &result {
                eprintln!("email: {error}");
            }
            if result == Ok(true) {
                self.state.config_saved();
            }
            self.state.migration_rewritten(result);
            return;
        }
        if !self.state.config_dirty() {
            return;
        }
        if let Some(file) = &self.config {
            let config = crate::config::EmailConfig::from(&self.state);
            match file.save(&config) {
                Ok(true) => self.state.config_saved(),
                Ok(false) => {} // the poison pill; nothing to report
                Err(error) => eprintln!("email: {error}"),
            }
        } else {
            self.state.config_saved();
        }
    }

    /// The session theme: settingsd's look when this build can draw it,
    /// else the theme file's.
    fn session_theme(&self) -> Theme {
        self.look
            .as_ref()
            .and_then(toolkit::session_look::theme)
            .unwrap_or_else(|| self.file_theme.clone())
    }

    /// Apply the profile's bindings for email over the built chords.
    pub fn set_keymap(&mut self, keymap: toolkit::keymap::Overrides) {
        if keymap == self.keymap {
            return;
        }
        for note in self.commands.rebind(&keymap) {
            eprintln!("email: keymap: {note}");
        }
        self.keymap = keymap;
    }

    /// Show the session look `look` (settingsd's), or the theme file's with
    /// `None`; the window's own choice stays on top.
    pub fn set_look(&mut self, look: Option<settings::Appearance>) {
        self.look = look;
        self.theme = self.session_theme();
        self.install_theme();
        self.ctx.request_repaint();
    }

    /// Read the session look (start, a settingsd hint, a reconnection). A
    /// read going out voids any armed retry; closing reads nothing.
    fn read_look(&mut self) {
        if self.exiting {
            return;
        }
        if let Some(read) = self.follower.read() {
            self.perform_read(read);
        }
    }

    fn perform_read(&mut self, read: settings::follow::Read) {
        self.look_retry += 1;
        let bus = self.bus.clone();
        self.spawn(async move {
            let result = bus
                .raw_within(read.service, read.verb, read.body.to_string(), LOOK_BUDGET)
                .await;
            Event::Look(read.ticket, result)
        });
    }

    /// A profile read answered: a new look is installed at once (under the
    /// window's own choice) and the `email.*` accounts reconcile; without
    /// settingsd the theme file and the accounts cache stand; a transient
    /// fault is retried with backoff, the last look standing.
    pub fn looked(&mut self, ticket: u64, result: Result<citizen::Reply, citizen::CallError>) {
        // Closing: nothing more to draw. A superseded read's answer changes
        // nothing, retries included.
        if self.exiting || self.follower.pending() != Some(ticket) {
            return;
        }
        let reply = result.ok();
        let answer = reply
            .as_ref()
            .map(|r| (r.rc, r.body.as_str(), r.error.as_deref()));
        let (followed, next) = self.follower.answered(ticket, answer);
        match followed {
            Some(settings::follow::Followed::Look {
                appearance,
                custom_source,
                keymaps,
                email,
                ..
            }) => {
                self.set_look((!custom_source).then_some(*appearance));
                self.set_keymap(keymaps.get(APP).cloned().unwrap_or_default());
                self.state.set_settings(*email);
            }
            // Nothing new, but settingsd answered: a write waiting on a
            // read is released.
            Some(settings::follow::Followed::Unchanged) => self.state.settings_unchanged(),
            Some(settings::follow::Followed::Missing) => {
                if self.look.is_some() {
                    self.set_look(None);
                }
                self.state.settings_missing();
            }
            _ => {}
        }
        match next {
            Some(read) => self.perform_read(read),
            None => {
                if let Some(delay) = self.follower.retry_delay() {
                    self.look_retry += 1;
                    let generation = self.look_retry;
                    self.spawn(async move {
                        tokio::time::sleep(delay).await;
                        Event::LookRetry(generation)
                    });
                }
            }
        }
    }

    /// The look retry armed last; a retry of any other generation is void.
    pub fn look_generation(&self) -> u64 {
        self.look_retry
    }

    /// The retry armed as `generation` is due: it reads, unless a newer
    /// read voided it or the window is closing.
    pub fn look_retry_due(&mut self, generation: u64) {
        if generation == self.look_retry && !self.exiting {
            self.read_look();
        }
    }

    /// The window's theme choice (View › Theme, the title bar's switch).
    fn choice(&self) -> toolkit::theme_menu::Choice {
        commands::choice(&self.state)
    }

    /// Install the session theme with the window's choice over it. Neither
    /// is ever written. The session mode is recorded so the title-bar
    /// toggle inverts what is actually drawn.
    fn install_theme(&mut self) {
        let choice = self.choice();
        toolkit::install(&self.ctx, &self.theme.with_choice(&choice));
        self.state.set_session_mode(self.theme.mode());
        self.installed = choice;
    }

    /// The choice changed since the theme was installed: install again.
    fn follow_theme(&mut self) {
        if self.choice() != self.installed {
            self.install_theme();
            self.ctx.request_repaint();
        }
    }

    /// One frame's logic; it runs even while the window is hidden.
    pub fn logic(&mut self, ctx: &egui::Context) {
        // Drive answers first: a `window close` answer goes out before the
        // close it asked for is handled.
        for answer in drive::logic(ctx) {
            self.bus.reply(answer.id, answer.rc, answer.body);
        }
        self.pump();
        self.follow_theme();
        // Closing waits for the drive layer's accepted work. A close is
        // asked for by the window (the close button) or a command
        // (file.quit, which sets the state's exiting flag).
        let closing = ctx.input(|i| i.viewport().close_requested()) || self.state.exiting;
        if closing && !self.exiting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.exiting = true;
            for answer in drive::finish(&self.ctx) {
                self.bus.reply(answer.id, answer.rc, answer.body);
            }
            // Every accepted command gets its answer before the Bus stops.
            for (id, _, _) in std::mem::take(&mut self.held) {
                self.refuse_closing(id);
            }
            // Downloads stop here; their callers hear from close_out.
            for (_, download) in self.downloads.drain() {
                download.abort();
            }
            self.drain_saver();
            self.state.close_out();
            for reply in self.state.take_replies() {
                self.bus.reply(reply.id, reply.rc, reply.body);
            }
            self.save_config_if_dirty();
            self.bus.quit();
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// One frame's drawing and interaction.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let fired = self.commands.shortcuts(ui.ctx(), &self.state);
        let stroke = toolkit::icons::stroke_width(&self.theme);
        let mut events = view(ui, &self.state, &self.commands, &self.strings, stroke);
        open_links(ui.ctx());
        events.extend(
            fired
                .into_iter()
                .map(|id| UiEvent::Command(id, Origin::Shortcut)),
        );
        // Edits first, then commands, so a command sees every edit made in
        // the same frame (the busviewer ordering contract).
        let (commands, edits): (Vec<_>, Vec<_>) = events
            .into_iter()
            .partition(|event| matches!(event, UiEvent::Command(..)));
        for event in edits {
            self.state.apply_ui(event);
        }
        for event in commands {
            if let UiEvent::Command(id, origin) = event
                && let Err(error) = self.commands.execute(id, &mut self.state, origin)
            {
                eprintln!("email: {id}: {error}");
            }
        }
        self.settle();
        // A theme change is installed by the next frame's logic, before any
        // drawing: never midway through the frame that saw the click.
        if self.choice() != self.installed {
            ui.ctx().request_repaint();
        }
    }
}

/// Hand the frame's link requests (a Markdown link clicked in the reader)
/// to the desktop's handler, detached. eframe is built without its own
/// browser launcher, so the requests are taken out of egui's output here;
/// only http, https and mailto addresses are opened, as one argument and
/// never through a shell.
fn open_links(ctx: &egui::Context) {
    let urls = ctx.output_mut(|output| {
        let mut urls = Vec::new();
        output.commands.retain(|command| match command {
            egui::OutputCommand::OpenUrl(open) => {
                urls.push(open.url.clone());
                false
            }
            _ => true,
        });
        urls
    });
    for url in urls
        .into_iter()
        .filter(|url| toolkit::markdown::openable(url))
    {
        match std::process::Command::new("xdg-open")
            .arg(url.trim())
            .spawn()
        {
            Ok(mut child) => {
                // Reaped off the UI thread, so it never lingers as a zombie.
                let reaped = std::thread::Builder::new()
                    .name("email-xdg-open-reap".into())
                    .spawn(move || drop(child.wait()));
                if reaped.is_err() {
                    eprintln!("email: the link opener's reaper did not start");
                }
            }
            Err(error) => eprintln!("email: opening a link: {error}"),
        }
    }
}

/// Download one attachment, put it where `target` says (on the blocking
/// pool), and when the target is Open, open it in the first of its type's
/// applications the launcher starts; the file's path.
async fn attachment(
    client: &Client,
    bus: &citizen::Handle,
    blob: &str,
    name: Option<String>,
    index: usize,
    mime: String,
    target: Target,
) -> Result<String, String> {
    let data = client
        .download_blob(blob)
        .await
        .map_err(|e| e.to_string())?;
    let open = target == Target::Open;
    let (path, apps) = tokio::task::spawn_blocking(move || {
        let path = attachments::place(&target, name.as_deref(), index, &mime, &data)?;
        let apps = if open {
            attachments::open_with(&path, &mime)?
        } else {
            Vec::new()
        };
        Ok::<_, String>((path, apps))
    })
    .await
    .map_err(|e| e.to_string())??;
    let shown = path.display().to_string();
    if !open {
        return Ok(shown);
    }
    for app in &apps {
        if launch(bus, app, &path).await.is_ok() {
            return Ok(shown);
        }
    }
    Err(label_with("attachment-no-app", &[("path", &shown)]))
}

/// Open `path` in desktop app `id` through the session's launcher
/// (`apps.launch` on `apps`); the launcher's refusal when it would not.
async fn launch(bus: &citizen::Handle, id: &str, path: &Path) -> Result<(), String> {
    let uris = [crate::print::file_uri(path)];
    match bus
        .call("apps", "apps.launch", json!({"id": id, "uris": uris}))
        .await
    {
        Ok(reply) if reply.rc == 0 => Ok(()),
        Ok(reply) => Err(reply.error.unwrap_or(reply.body)),
        Err(error) => Err(error),
    }
}

/// The largest CA bundle read: a root and its intermediates fit many times.
pub const CA_MAX_BYTES: u64 = 64 * 1024;

/// Read the PEM bundle at `path`: a regular file of at most
/// [`CA_MAX_BYTES`], opened without following it into a device or FIFO
/// (checked on the open descriptor) and read no further than the cap. It
/// blocks, so it runs off the async workers.
pub fn read_ca(path: &str) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        // A FIFO opened for reading would block until a writer appears.
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(label("ca-not-file"));
    }
    if meta.len() > CA_MAX_BYTES {
        return Err(label("ca-too-large"));
    }
    let mut pem = String::new();
    file.take(CA_MAX_BYTES + 1)
        .read_to_string(&mut pem)
        .map_err(|e| e.to_string())?;
    if pem.len() as u64 > CA_MAX_BYTES {
        return Err(label("ca-too-large"));
    }
    Ok(pem)
}

/// Connect with `login`, reading its CA bundle first (on the blocking
/// pool, never an async worker).
async fn connect(login: &Login) -> Result<Client, jmap::Error> {
    let ca_pem = match login.ca_path.clone() {
        Some(path) => {
            let shown = path.clone();
            let read = tokio::task::spawn_blocking(move || read_ca(&path))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
            Some(read.map_err(|e| {
                jmap::Error::Protocol(format!("{}: {shown}: {e}", label("ca-unreadable")))
            })?)
        }
        None => None,
    };
    Client::connect(&jmap::Account {
        url: login.url.clone(),
        email: login.email.clone(),
        password: login.password.clone(),
        ca_pem,
    })
    .await
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        Shell::logic(self, ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        Shell::ui(self, ui);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(rc: u8, body: &str) -> Result<citizen::Reply, citizen::CallError> {
        Ok(citizen::Reply {
            rc,
            body: body.into(),
            error: None,
        })
    }

    #[test]
    fn apply_answers_are_read_as_settled() {
        assert_eq!(
            settled(reply(0, r#"{"status":"applied"}"#)),
            Settled::Accepted
        );
        assert_eq!(
            settled(reply(10, r#"{"status":"conflict"}"#)),
            Settled::Conflict
        );
        assert_eq!(
            settled(reply(10, r#"{"status":"invalid","message":"bad url"}"#)),
            Settled::Refused("bad url".into())
        );
        assert!(matches!(settled(reply(10, "")), Settled::Unavailable(_)));
        let lost = Err(citizen::CallError {
            message: "timed out".into(),
            outcome_unknown: true,
        });
        assert!(matches!(settled(lost), Settled::Unknown(_)));
    }

    /// The live install's shape, in a tempdir: a schema-1 file with one
    /// account, its password and an inline CA bundle. Starting migrates
    /// nothing away yet: the account is shown and connects at once, its
    /// password lands in the secrets file (0600), its CA bundle beside it,
    /// and the schema-1 file still holds everything until settingsd takes it.
    #[test]
    fn a_schema_one_install_starts_with_its_account_and_loses_nothing() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config");
        std::fs::create_dir(&dir).unwrap();
        let raw = "schema_version: 1\naccount: {url: \"https://mail.example.invalid\", email: \"admin@example.invalid\", password: \"pw-live\", ca_pem: \"-----BEGIN CERTIFICATE-----\\nAA\\n-----END CERTIFICATE-----\\n\"}\nui: {places: 0.2}\n";
        std::fs::write(dir.join("config.conf.mix"), raw).unwrap();
        let (mut state, config, secrets) = start(Some(&dir));
        let id = crate::writer::legacy_id("https://mail.example.invalid", "admin@example.invalid");
        let view = state.account(&id).expect("the old account is shown");
        assert_eq!(view.entry.email, "admin@example.invalid");
        let ca = view.entry.ca_path.clone().expect("the CA bundle by path");
        assert!(
            std::fs::read_to_string(&ca)
                .unwrap()
                .contains("BEGIN CERTIFICATE")
        );
        let work = state.take_work();
        let save = work.iter().find_map(|w| match w {
            Work::SaveSecrets { ticket } => Some(*ticket),
            _ => None,
        });
        assert!(work.iter().any(|w| matches!(w, Work::Connect { .. })));
        // The shell performs the save.
        let file = secrets.unwrap();
        assert!(file.save(state.secrets()).unwrap());
        let version = state.secrets_snapshot().0;
        state.secrets_saved(save.unwrap(), version, Ok(true));
        let (loaded, _) = SecretsFile::load(&dir);
        assert_eq!(loaded.password(&id), Some("pw-live"));
        let mode = std::fs::metadata(&file.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // Until settingsd has it, a save keeps the schema-1 file whole.
        let config = config.unwrap();
        assert!(config.legacy());
        config
            .save(&crate::config::EmailConfig::from(&state))
            .unwrap();
        let kept = std::fs::read_to_string(dir.join("config.conf.mix")).unwrap();
        let kept: Value = config::from_conf_mix_str(&kept).unwrap();
        assert_eq!(kept["schema_version"].as_f64(), Some(1.0));
        assert_eq!(kept["account"]["password"], "pw-live");
        // A second start finds the same account under the same id.
        let (again, _, _) = start(Some(&dir));
        assert!(again.account(&id).is_some());
        assert_eq!(again.accounts.len(), 1);
    }

    /// Review #19: secrets are saved on the saver thread, in order, and each
    /// ack names the snapshot version it wrote.
    #[test]
    fn review_19_the_saver_writes_off_the_ui_thread_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (_, file) = SecretsFile::load(dir.path());
        let (events, received) = mpsc::channel();
        let (saver, thread) = saver(file, events, egui::Context::default());
        let ui = std::thread::current().id();
        for (ticket, version, password) in [(1, 5, "pw-1"), (2, 6, "pw-2")] {
            let mut secrets = Secrets::default();
            secrets.set("a", password);
            saver.send((ticket, version, secrets)).unwrap();
        }
        for expected in [(1, 5), (2, 6)] {
            match received.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(Event::SecretsSaved(ticket, version, Ok(true))) => {
                    assert_eq!((ticket, version), expected);
                }
                _ => panic!("no ack for {expected:?}"),
            }
        }
        assert_eq!(std::thread::current().id(), ui);
        let (saved, _) = SecretsFile::load(dir.path());
        assert_eq!(
            saved.password("a"),
            Some("pw-2"),
            "the later save is the one kept"
        );
        // Dropping the sender ends the thread: closing can join it.
        drop(saver);
        thread.unwrap().join().unwrap();
    }

    /// Review finding 13: a CA path must name a small regular file; a
    /// device, a FIFO (which must not block) or a large file is refused.
    #[test]
    fn review_13_a_ca_path_is_a_small_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let pem = dir.path().join("ca.pem");
        std::fs::write(&pem, "-----BEGIN CERTIFICATE-----\nAA\n").unwrap();
        assert!(read_ca(pem.to_str().unwrap()).unwrap().contains("BEGIN"));
        assert_eq!(read_ca("/dev/zero"), Err(label("ca-not-file")));
        let big = dir.path().join("big.pem");
        std::fs::write(&big, vec![b'A'; (CA_MAX_BYTES + 1) as usize]).unwrap();
        assert_eq!(read_ca(big.to_str().unwrap()), Err(label("ca-too-large")));
        let fifo = dir.path().join("fifo");
        let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path in a private tempdir.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(read_ca(fifo.to_str().unwrap()), Err(label("ca-not-file")));
    }

    /// Review finding 5 (the migration's part): credentials in the old
    /// account's URL never reach settingsd, and the note never echoes them.
    #[test]
    fn round5_23_the_close_drain_drops_nothing_that_waits() {
        let command = Event::Delivery(citizen::Delivery::Command {
            id: 9,
            verb: "email.ping".into(),
            body: "{}".into(),
        });
        assert_eq!(closing(&command), Closing::Cancel(9));
        assert_eq!(closing(&Event::Shown(3, Ok(json!({})))), Closing::Shown);
        assert_eq!(
            closing(&Event::SecretsSaved(1, 1, Ok(true))),
            Closing::Apply
        );
        assert_eq!(
            closing(&Event::Settled(1, Settled::Accepted)),
            Closing::Apply
        );
        assert_eq!(closing(&Event::LookRetry(1)), Closing::Ignore);
    }

    #[test]
    fn round5_5_a_legacy_query_credential_never_reaches_a_reply() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config");
        std::fs::create_dir(&dir).unwrap();
        let raw = "schema_version: 1\naccount: {url: \"https://mail.example.invalid/jmap?password=hunter2#frag\", email: \"admin@example.invalid\", password: \"pw\"}\n";
        std::fs::write(dir.join("config.conf.mix"), raw).unwrap();
        let (state, _, _) = start(Some(&dir));
        assert_eq!(
            state.accounts[0].entry.url,
            "https://mail.example.invalid/jmap"
        );
        assert!(!state.accounts_view().to_string().contains("hunter2"));
        assert!(!state.info().to_string().contains("hunter2"));
        // The schema-1 source is left as it was.
        assert_eq!(
            std::fs::read_to_string(dir.join("config.conf.mix")).unwrap(),
            raw
        );
    }

    #[test]
    fn review_5_userinfo_is_stripped_from_the_migrated_url() {
        assert_eq!(
            without_userinfo("https://user:hunter2@mail.example.invalid/jmap"),
            ("https://mail.example.invalid/jmap".to_owned(), true)
        );
        assert_eq!(
            without_userinfo("https://mail.example.invalid/a@b"),
            ("https://mail.example.invalid/a@b".to_owned(), false)
        );
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(
            dir.join("config.conf.mix"),
            "schema_version: 1\naccount: {url: \"https://admin:hunter2@mail.example.invalid\", email: \"admin@example.invalid\", password: \"pw\"}\n",
        )
        .unwrap();
        let (state, _, _) = start(Some(&dir));
        let view = &state.accounts[0];
        assert_eq!(view.entry.url, "https://mail.example.invalid");
        assert_eq!(state.status, label("migration-url-credentials"));
        assert!(!state.status.contains("hunter2"));
    }
}
