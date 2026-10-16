// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The running window: feeds the state its events and carries out its
//! work. JMAP calls run on a small tokio runtime; each completion is sent
//! back as an [`Event`] with a repaint request, so the window wakes only
//! when something happened (no polling). The Bus connection carries the
//! session-look topic (theme-follow) and single-instance activation.
//!
//! This is the app's one architectural exception: the effects runtime
//! speaks HTTPS itself, through the `jmap` engine, because a pure JMAP
//! client's server is whatever account the person pointed it at.

use std::sync::{Arc, Mutex, mpsc};
use std::task::Poll;

use futures::StreamExt;
use jmap::{Client, EmailPage};
use toolkit::command::Origin;
use toolkit::{Registry, Strings, Theme, drive};

use crate::commands;
use crate::state::{SessionInfo, State, UiState};
use crate::view::{UiEvent, view};
use crate::work::Work;
use crate::{APP, label};

/// What finished off the UI thread. Completions carry their result; the
/// shell resolves the Look pair itself (theme-follow, below).
pub enum Event {
    /// A Bus delivery.
    Delivery(citizen::Delivery),
    /// The connect finished. The client stays in the shell; the state gets
    /// the summary.
    Connected(Result<SessionInfo, jmap::Error>),
    Mailboxes(Result<Vec<jmap::Mailbox>, jmap::Error>),
    /// The ticket is the request's (see [`crate::work::Work`]).
    Page(u64, Result<EmailPage, jmap::Error>),
    /// One email with its bodies; boxed — a full [`jmap::Email`] is large.
    Opened(u64, Box<Result<jmap::Email, jmap::Error>>),
    Marked(Result<(), jmap::Error>),
    Sent(u64, Result<(), jmap::Error>),
    Calendars(Result<Vec<jmap::Calendar>, jmap::Error>),
    Events(u64, Result<Vec<jmap::CalendarEvent>, jmap::Error>),
    Books(Result<Vec<jmap::AddressBook>, jmap::Error>),
    Contacts(u64, Result<Vec<jmap::Contact>, jmap::Error>),
    /// A read of the session look (settingsd), for the shell.
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
    client: Option<Client>,
    /// Where a finished connect parks its client for the UI thread to
    /// adopt; the event channel carries only the summary.
    adopted: Arc<Mutex<Option<Client>>>,
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
}

/// This host's name: the settingsd instance to try first (settingsd runs
/// as `--instance %H`; the follower adopts the right one from its answer).
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}

/// How long one settings read may take.
const LOOK_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

type Deliveries = futures::channel::mpsc::Receiver<citizen::Delivery>;

fn locked(inbox: &Mutex<Deliveries>) -> std::sync::MutexGuard<'_, Deliveries> {
    inbox
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        // The forwarder owns the deliveries from here: the shell never
        // reads them (email answers no Bus commands of its own).
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
        // The persisted account and window shape, under the poison pill.
        let (config, file) = match config::AppDirs::resolve(APP) {
            Some(dirs) => {
                let (config, file) = crate::config::ConfigFile::load(&dirs.config());
                (config, Some(file))
            }
            None => {
                eprintln!("email: no config directory; nothing is persisted");
                (crate::config::EmailConfig::default(), None)
            }
        };
        let ui = UiState {
            places: config.ui.places,
            reader: config.ui.reader,
            dialog: None,
            theme: Default::default(),
        };
        let mut shell = Self {
            state: State::new(config.account, ui, config.ui.mode),
            commands: commands::registry(),
            strings: crate::strings(),
            theme: theme.clone(),
            bus,
            runtime,
            tx,
            rx,
            ctx,
            client: None,
            adopted: Arc::new(Mutex::new(None)),
            exiting: false,
            installed: toolkit::theme_menu::Choice::default(),
            file_theme: theme,
            look: None,
            keymap: toolkit::keymap::Overrides::new(),
            follower: settings::follow::Follower::new(&hostname()),
            look_retry: 0,
            config: file,
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
        F: Future + Send + 'static,
        F::Output: Into<Event>,
    {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        self.runtime.spawn(async move {
            let _ = tx.send(work.await.into());
            ctx.request_repaint();
        });
    }

    /// Perform the state's queued work. A connect that finished parks its
    /// client for this thread first, so the fetches a connect schedules
    /// find it there.
    fn settle(&mut self) {
        if let Some(client) = self.adopted.lock().unwrap().take() {
            self.client = Some(client);
        }
        for work in self.state.take_work() {
            self.perform(work);
        }
        self.save_config_if_dirty();
    }

    /// Perform one unit of work on the effects runtime.
    fn perform(&mut self, work: Work) {
        match work {
            Work::Connect(account) => {
                self.client = None;
                let adopted = self.adopted.clone();
                self.spawn(async move {
                    let connected = match Client::connect(&account).await {
                        Ok(client) => {
                            *adopted.lock().unwrap() = Some(client.clone());
                            Ok(SessionInfo {
                                account_name: client
                                    .session()
                                    .account_name
                                    .clone()
                                    .unwrap_or_else(|| account.email.clone()),
                                account_id: client.account_id().to_owned(),
                            })
                        }
                        Err(error) => Err(error),
                    };
                    Event::Connected(connected)
                });
            }
            Work::FetchMailboxes => {
                if let Some(client) = self.client() {
                    self.spawn(async move { Event::Mailboxes(client.mailboxes().await) });
                }
            }
            Work::FetchEmails { ticket, mailbox } => {
                if let Some(client) = self.client() {
                    self.spawn(async move {
                        Event::Page(ticket, client.email_page(&mailbox, 0, 100).await)
                    });
                }
            }
            Work::OpenEmail { ticket, id } => {
                if let Some(client) = self.client() {
                    self.spawn(
                        async move { Event::Opened(ticket, Box::new(client.email(&id).await)) },
                    );
                }
            }
            Work::MarkRead { id, read } => {
                if let Some(client) = self.client() {
                    self.spawn(async move { Event::Marked(client.mark_read(&id, read).await) });
                }
            }
            Work::Send { ticket, draft } => {
                // The send recipe needs the Sent mailbox; without the mail
                // listing yet, there is nothing to file under.
                let Some(sent) = self.state.sent_mailbox_id() else {
                    self.state.apply(Event::Sent(
                        ticket,
                        Err(jmap::Error::Protocol("no Sent mailbox yet".into())),
                    ));
                    return;
                };
                if let Some(client) = self.client() {
                    self.spawn(
                        async move { Event::Sent(ticket, client.send(&draft, &sent).await) },
                    );
                }
            }
            Work::FetchCalendars => {
                if let Some(client) = self.client() {
                    self.spawn(async move { Event::Calendars(client.calendars().await) });
                }
            }
            Work::FetchEvents { ticket, calendar } => {
                if let Some(client) = self.client() {
                    self.spawn(async move {
                        Event::Events(ticket, client.calendar_events(&calendar).await)
                    });
                }
            }
            Work::FetchBooks => {
                if let Some(client) = self.client() {
                    self.spawn(async move { Event::Books(client.address_books().await) });
                }
            }
            Work::FetchContacts {
                ticket,
                book,
                filter,
            } => {
                if let Some(client) = self.client() {
                    self.spawn(async move {
                        Event::Contacts(ticket, client.contacts(&book, &filter).await)
                    });
                }
            }
        }
    }

    /// The connected client, or a note on the status line when there is
    /// none (the token died, the app reconnected, the connect failed).
    /// The clone is cheap: the HTTP pool and the token cell are shared.
    fn client(&mut self) -> Option<Client> {
        if self.client.is_none() {
            self.state.status = label("not-connected");
            self.ctx.request_repaint();
        }
        self.client.clone()
    }

    /// Apply every finished event, settling after each one, so a later
    /// completion can never change what an earlier one acted on.
    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            if let Event::Look(ticket, result) = event {
                self.looked(ticket, result);
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

    /// Persist the config when the state says it changed.
    fn save_config_if_dirty(&mut self) {
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

    /// A look read answered: a new look is installed at once (under the
    /// window's own choice); without settingsd the theme file stands; a
    /// transient fault is retried with backoff, the last look standing.
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
                ..
            }) => {
                self.set_look((!custom_source).then_some(*appearance));
                self.set_keymap(keymaps.get(APP).cloned().unwrap_or_default());
            }
            Some(settings::follow::Followed::Missing) if self.look.is_some() => {
                self.set_look(None);
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

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        Shell::logic(self, ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        Shell::ui(self, ui);
    }
}
