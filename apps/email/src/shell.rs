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

use crate::commands;
use crate::secrets::{Secrets, SecretsFile};
use crate::state::{Mode, SessionInfo, Settled, State, Tag, UiState};
use crate::view::{UiEvent, view};
use crate::work::{Login, Work};
use crate::{APP, label};

/// One account's answer, addressed by [`Tag`] in [`Event::Account`]. The
/// tickets are the requests' (see [`crate::work::Work`]).
pub enum Done {
    Mailboxes(Result<Vec<jmap::Mailbox>, jmap::Error>),
    Page(u64, Result<EmailPage, jmap::Error>),
    /// One email with its bodies; boxed — a full [`jmap::Email`] is large.
    Opened(u64, Box<Result<jmap::Email, jmap::Error>>),
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
    SecretsSaved(u64, Result<bool, String>),
    /// A read of the session profile (settingsd), for the shell.
    Look(u64, Result<citizen::Reply, citizen::CallError>),
    /// A retry of the session look after a transient fault (its generation).
    LookRetry(u64),
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
    secrets: Option<SecretsFile>,
    /// Settings writes sent, for unique operation ids.
    applies: u64,
}

/// This host's name: the settingsd instance to try first (settingsd runs
/// as `--instance %H`; the follower adopts the right one from its answer).
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}

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
) -> Result<settings::email::AccountEntry, String> {
    let url = account.url.trim().trim_end_matches('/').to_owned();
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
    Ok(settings::email::AccountEntry {
        id,
        name,
        url,
        email,
        ca_path,
        enabled: true,
        signature: String::new(),
    })
}

/// The state a launch starts in, from the app's config directory: the
/// window's shape, the accounts cache, the passwords, and a schema-1
/// account still to migrate. Without a config directory nothing persists.
pub fn start(dir: Option<&Path>) -> (State, Option<crate::config::ConfigFile>, Option<SecretsFile>) {
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
            Ok(entry) => state.begin_migration(entry, &legacy.password),
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
            secrets,
            applies: 0,
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

    /// Spawn `work` and send its result back as an event.
    fn spawn<F>(&self, work: F)
    where
        F: Future<Output = Event> + Send + 'static,
    {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.runtime.spawn(async move {
            let _ = tx.send(work.await);
            ctx.request_repaint();
        });
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
            } => {
                if let Some((tag, client)) = self.client(&account) {
                    self.spawn(async move {
                        let page = client.email_page(&mailbox, 0, limit).await;
                        Event::Account(tag, Done::Page(ticket, page))
                    });
                }
            }
            Work::OpenEmail {
                account,
                ticket,
                id,
            } => {
                if let Some((tag, client)) = self.client(&account) {
                    self.spawn(async move {
                        let email = client.email(&id).await;
                        Event::Account(tag, Done::Opened(ticket, Box::new(email)))
                    });
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
                            Event::Account(tag, Done::Sent(ticket, client.send(&draft, &sent).await))
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
                    None => self.state.settled(
                        ticket,
                        Settled::Unavailable(label("settings-missing")),
                    ),
                    Some(apply) => {
                        let bus = self.bus.clone();
                        self.spawn(async move {
                            let result = bus
                                .raw_within(apply.service, apply.verb, apply.body.to_string(), LOOK_BUDGET)
                                .await;
                            Event::Settled(ticket, settled(result))
                        });
                    }
                }
            }
            Work::ReadSettings => self.read_look(),
            Work::SaveSecrets { ticket } => {
                let result = match &self.secrets {
                    Some(file) => file.save(self.state.secrets()),
                    None => Ok(true),
                };
                if let Err(error) = &result {
                    eprintln!("email: {error}");
                }
                self.state.secrets_saved(ticket, result);
            }
            Work::OpenPrefs => self.open_prefs(),
        }
    }

    /// Show Prefs on its Email section: ask a running Prefs, else launch it
    /// and ask again once it registers.
    fn open_prefs(&mut self) {
        let bus = self.bus.clone();
        self.runtime.spawn(async move {
            let section = json!({"name": "email"});
            let asked = bus.call("prefs", "prefs.section", section.clone()).await;
            if asked.as_ref().is_ok_and(|r| r.rc == 0) {
                return;
            }
            if let Err(error) = launch_prefs() {
                eprintln!("email: cannot launch Prefs: {error}");
                return;
            }
            // A launch registers within seconds; ask until it answers.
            for _ in 0..40 {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let asked = bus.call("prefs", "prefs.section", section.clone()).await;
                if asked.is_ok_and(|r| r.rc == 0) {
                    return;
                }
            }
            eprintln!("email: Prefs did not answer prefs.section");
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
            if let Event::Delivery(citizen::Delivery::Command { .. }) = event {
                // email answers no Bus verbs of its own in this slice; the
                // drive verbs (window commands, captures) go to toolkit.
                continue;
            }
            self.state.apply(event);
            self.settle();
        }
    }

    /// Persist the config when the state says it changed, and rewrite it as
    /// schema 2 once the schema-1 account has migrated.
    fn save_config_if_dirty(&mut self) {
        let migrated = self.state.take_migrated();
        if let (true, Some(file)) = (migrated, &mut self.config) {
            file.migrated();
        }
        if !self.state.config_dirty() && !migrated {
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

/// Connect with `login`, reading its CA bundle first.
async fn connect(login: &Login) -> Result<Client, jmap::Error> {
    let ca_pem = match &login.ca_path {
        Some(path) => Some(std::fs::read_to_string(path).map_err(|e| {
            jmap::Error::Protocol(format!("{}: {path}: {e}", label("ca-unreadable")))
        })?),
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

/// Start Prefs, detached: from `PATH`, else the MixOS install.
fn launch_prefs() -> Result<(), String> {
    let mut last = String::new();
    for program in ["prefs", "/opt/mixos/bin/prefs"] {
        match std::process::Command::new(program)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            // Reaped by a thread so it never lingers as a zombie.
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
                return Ok(());
            }
            Err(error) => last = format!("{program}: {error}"),
        }
    }
    Err(last)
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
        assert_eq!(settled(reply(0, r#"{"status":"applied"}"#)), Settled::Accepted);
        assert_eq!(settled(reply(10, r#"{"status":"conflict"}"#)), Settled::Conflict);
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
        assert!(std::fs::read_to_string(&ca).unwrap().contains("BEGIN CERTIFICATE"));
        let work = state.take_work();
        let save = work.iter().find_map(|w| match w {
            Work::SaveSecrets { ticket } => Some(*ticket),
            _ => None,
        });
        assert!(work.iter().any(|w| matches!(w, Work::Connect { .. })));
        // The shell performs the save.
        let file = secrets.unwrap();
        assert!(file.save(state.secrets()).unwrap());
        state.secrets_saved(save.unwrap(), Ok(true));
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
        assert!(kept.contains("schema_version: 1") && kept.contains("pw-live"));
        // A second start finds the same account under the same id.
        let (again, _, _) = start(Some(&dir));
        assert!(again.account(&id).is_some());
        assert_eq!(again.accounts.len(), 1);
    }
}
