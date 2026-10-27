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
use toolkit::action::Origin;
use toolkit::{Registry, Strings, Theme, drive};

use crate::actions;
use crate::commands::{Forward, Then, Write};
use crate::drafts::{DirStore, DraftStore, MemoryStore};
use crate::secrets::{Secrets, SecretsFile};
use crate::state::{
    DRAFT_DEBOUNCE, Mode, SendOrigin, SessionInfo, Settled, State, Tag, UiState, code,
};
use crate::view::{UiEvent, view};
use crate::work::{Login, Work};
use crate::{APP, APP_ID, label};

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
    /// A batched message action finished: each id done or failed.
    Batched(Box<crate::work::Batch>, jmap::SetOutcome),
    /// A page of "Select all N" fetch `ticket`'s ids.
    Ids(u64, Result<jmap::IdPage, jmap::Error>),
    /// A send finished: its ticket, what the try did on the server (kept
    /// for a retry), and the outcome.
    Sent(
        u64,
        jmap::SendAttempt,
        Result<jmap::SendReport, jmap::Error>,
    ),
    Calendars(Result<Vec<jmap::Calendar>, jmap::Error>),
    Events(u64, Result<Vec<jmap::CalendarEvent>, jmap::Error>),
    Books(Result<Vec<jmap::AddressBook>, jmap::Error>),
    Contacts(u64, Result<Vec<jmap::Contact>, jmap::Error>),
    /// An attachment save or open finished (boxed: it carries a path).
    Attachment(Box<AttachmentDone>),
    /// Contacts for compose suggestion `ticket`.
    Suggested(u64, Result<Vec<jmap::Contact>, jmap::Error>),
}

/// An attachment save or open: which, for whom, and where it landed.
pub struct AttachmentDone {
    pub email: String,
    pub part: String,
    pub open: bool,
    pub reply: Option<u64>,
    /// The saved file's path, or why there is none.
    pub result: Result<String, String>,
}

/// What finished off the UI thread. Completions carry their result; the
/// shell resolves the Look pair itself (theme-follow, below).
pub enum Event {
    MailDocument(u64, crate::documents::Answer),
    /// A received image downloaded and decoded on the effects runtime.
    Preview(u64, Result<egui::ColorImage, String>),
    ComposeAttachments(
        crate::work::ComposeInspection,
        crate::work::InspectedAttachments,
    ),
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
    /// A print page was written: its path and whether the browser took it,
    /// for Bus request `reply`; or why it could not be written.
    Printed(Option<u64>, Result<(String, bool), String>),
    /// Compose suggestion `ticket`'s debounce has passed.
    SuggestDue(u64),
}

async fn inspect_compose_attachments(inspection: crate::work::ComposeInspection) -> Event {
    let worker = inspection.clone();
    let result = tokio::task::spawn_blocking(move || worker.inspect())
        .await
        .unwrap_or_else(|error| Err((code::SERVER, error.to_string())));
    Event::ComposeAttachments(inspection, result)
}

pub struct Shell {
    composers: crate::composer::Windows,
    launch_compose: bool,
    launch_document: Option<PathBuf>,
    state: State,
    commands: Registry<State>,
    strings: Strings,
    theme: Theme,
    palette: editor::Palette,
    composer_light: bool,
    bus: citizen::Handle,
    /// Caller identities outlive command() while toolkit input is replayed.
    drive_callers: HashMap<u64, SendOrigin>,
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
    common_ui: settings::model::CommonUi,
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
    /// Where unsent messages are kept (the app's state directory).
    drafts: Box<dyn DraftStore>,
    /// When the open draft's unsaved edits are saved, if it has any.
    draft_due: Option<std::time::Instant>,
}

fn audit_attachments(caller: &str, draft: &jmap::Composed) {
    audit_compose_attachments(
        &SendOrigin {
            caller_key: caller.into(),
            drive_request_id: None,
            mixed_input: false,
        },
        draft,
    );
}

fn audit_compose_attachments(origin: &SendOrigin, draft: &jmap::Composed) {
    if !draft.attachments.is_empty() {
        tracing::info!(
            caller_key = origin.caller_key.as_str(),
            drive_request_id = ?origin.drive_request_id,
            mixed_input = origin.mixed_input,
            attachments = ?draft.attachments,
            "sending email attachments"
        );
    }
}

fn audit_drive(id: u64, caller: &str, command: &str) {
    tracing::info!(
        request_id = id,
        caller_key = caller,
        command,
        "email drive request"
    );
}

fn request_drive(
    ctx: &egui::Context,
    callers: &mut HashMap<u64, SendOrigin>,
    id: u64,
    caller: String,
    command: &str,
    args: &Value,
) -> Option<drive::Answer> {
    audit_drive(id, &caller, command);
    callers.insert(
        id,
        SendOrigin {
            caller_key: caller,
            drive_request_id: Some(id),
            mixed_input: false,
        },
    );
    let answer = drive::request(ctx, id, command, args);
    if answer.is_some() {
        callers.remove(&id);
    }
    answer
}

fn driven_origin(ctx: &egui::Context, callers: &HashMap<u64, SendOrigin>) -> Option<SendOrigin> {
    drive::input_request(ctx)
        .and_then(|id| callers.get(&id))
        .cloned()
        .map(|mut origin| {
            origin.mixed_input = drive::input_mixed(ctx);
            origin
        })
}

fn apply_ui_events(
    state: &mut State,
    registry: &Registry<State>,
    events: Vec<UiEvent>,
    origin: Option<SendOrigin>,
) {
    state.with_send_origin(origin, |state| {
        // Edits first so commands see every edit made in this frame.
        let (closed, events): (Vec<_>, Vec<_>) = events
            .into_iter()
            .partition(|e| matches!(e, UiEvent::CloseMacros));
        let (commands, edits): (Vec<_>, Vec<_>) = events
            .into_iter()
            .partition(|event| matches!(event, UiEvent::Command(..)));
        for event in edits {
            state.apply_ui(event);
        }
        for event in commands {
            if let UiEvent::Command(id, origin) = event
                && let Err(error) = registry.execute(id, state, origin)
            {
                eprintln!("email: {id}: {error}");
            }
        }
        for event in closed {
            state.apply_ui(event);
        }
    });
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

/// The desktop entry a print page opens in: its print dialog prints or
/// saves a PDF.
const BROWSER_APP_ID: &str = "firefox";

/// Print pages older than this are removed on the next print.
pub const PRINT_KEEP: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Download blob `blob` and save it as `name`: in `dir`, else the
/// downloads directory (made if missing), or (`open`) in the private open
/// directory and then handed to the desktop's default application
/// (`xdg-open`, as Dopus opens files). The disk work is off the async
/// workers; a window closing mid-download drops it before anything is
/// written, since the file is only linked into place whole.
async fn attachment(
    client: &Client,
    blob: &str,
    name: String,
    open: bool,
    dir: Option<String>,
) -> Result<String, String> {
    let bytes = client
        .download_blob(blob)
        .await
        .map_err(|e| e.to_string())?;
    let path = tokio::task::spawn_blocking(move || -> Result<PathBuf, String> {
        let dir = match (open, dir) {
            (true, _) => crate::attachments::open_dir()?,
            (false, Some(dir)) => PathBuf::from(dir),
            (false, None) => {
                let dir = crate::attachments::downloads()?;
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                dir
            }
        };
        crate::attachments::save(&dir, &name, &bytes)
    })
    .await
    .map_err(|e| e.to_string())??;
    if open {
        let mut child = std::process::Command::new("xdg-open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
        // Reaped off the UI thread, so it never lingers as a zombie.
        let _ = std::thread::Builder::new()
            .name("email-xdg-open-reap".into())
            .spawn(move || drop(child.wait()));
    }
    Ok(path.display().to_string())
}

/// The private print directory, `$XDG_RUNTIME_DIR/email-print`.
fn print_dir() -> Result<PathBuf, String> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(dir).join("email-print"))
        .ok_or_else(|| label("no-runtime-dir"))
}

/// Write print page `html` as `dir/name`: the directory made private
/// (0700), the page written whole to a temporary file (0600) and renamed
/// into place, and pages older than [`PRINT_KEEP`] at `now` removed first.
pub fn write_print(
    dir: &Path,
    name: &str,
    html: &str,
    now: std::time::SystemTime,
) -> Result<PathBuf, String> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    if name.contains('/') || name.starts_with('.') {
        return Err(format!("bad print page name {name:?}"));
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| e.to_string())?;
    // A directory that was already there keeps its mode: make it private.
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|modified| {
                    now.duration_since(modified)
                        .is_ok_and(|age| age > PRINT_KEEP)
                });
            if stale && path.extension().is_some_and(|e| e == "html") {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    let path = dir.join(name);
    let partial = dir.join(format!(".{name}.partial"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&partial)
        .map_err(|e| e.to_string())?;
    file.write_all(html.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    Ok(path)
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
            identities: Vec::new(),
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
        | Event::MailDocument(..)
        | Event::ComposeAttachments(..)
        | Event::Account(..)
        | Event::Settled(..)
        | Event::SecretsSaved(..)
        | Event::Printed(..)
        | Event::WriteDeadline(_) => Closing::Apply,
        Event::SuggestDue(_) | Event::Preview(..) => Closing::Ignore,
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
        attachments_expanded: loaded.config.ui.attachments_expanded,
        attachments_inline: loaded.config.ui.attachments_inline,
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
        let dirs = config::AppDirs::resolve(APP);
        let dir: Option<PathBuf> = dirs.as_ref().map(config::AppDirs::config);
        let (state, config, secrets) = start(dir.as_deref());
        // Drafts live in the state directory, not the config: they are not
        // settings. Without one they are kept in memory for this run only.
        let drafts: Box<dyn DraftStore> = match &dirs {
            Some(dirs) => Box::new(DirStore::new(&dirs.state())),
            None => Box::new(MemoryStore::default()),
        };
        let (saver, saver_thread) = match secrets.map(|file| saver(file, tx.clone(), ctx.clone())) {
            Some((saver, thread)) => (Some(saver), thread),
            None => (None, None),
        };
        let composer_light = state.settings.composer.light;
        let palette = editor::Palette::from_theme(if composer_light {
            toolkit::content::light_theme()
        } else {
            &theme
        });
        let mut shell = Self {
            composers: crate::composer::Windows::default(),
            launch_compose: false,
            launch_document: None,
            state,
            palette,
            composer_light,
            commands: actions::registry(),
            strings: crate::strings(),
            theme: theme.clone(),
            bus,
            drive_callers: HashMap::new(),
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
            common_ui: settings::model::CommonUi::default(),
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
            drafts,
            draft_due: None,
        };
        // Drafts an earlier run kept are offered once the window is up.
        let kept = shell.drafts.list();
        shell.state.offer_drafts(kept);
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

    pub fn compose_launch(&mut self, compose: bool, document: Option<PathBuf>) {
        self.composers.compose_only = compose;
        self.launch_compose = compose;
        self.launch_document = document;
        if compose {
            self.composers.insert(&mut self.state, self.palette.clone());
            self.start_composer();
        }
    }

    fn start_composer(&mut self) {
        if !self.launch_compose { return; }
        let Some(account) = self.state.compose_account() else { return; };
        self.launch_compose = false;
        if let Some(path) = self.launch_document.take() {
            if let Err((_, error)) = self.state.read_document(account, path, None, false, false) {
                self.state.status = error;
            }
        } else { self.state.compose(); }
        self.sync_composers();
        self.settle();
    }

    fn sync_composers(&mut self) {
        let palette = self.editor_palette();
        self.composers.adopt(&mut self.state, palette.clone());
        for window in self.composers.windows.values() {
            let mut window = window.lock().unwrap();
            let previous = window.view.compose.as_ref().and_then(|c| window.view.account(&c.account)).map(|a| a.entry.clone());
            crate::composer::swap(&mut self.state, &mut window.view);
            self.state.recheck_composer(previous.as_ref());
            crate::composer::swap(&mut self.state, &mut window.view);
        }
        self.composers.refresh(&self.ctx, &self.state, &palette);
    }

    fn composer_input(&mut self) {
        let windows: Vec<_> = self.composers.windows.values().cloned().collect();
        let mut registry = crate::composer::registry();
        registry.rebind(&self.keymap);
        let mut cancelled_quit = false;
        let mut dropped_documents = Vec::new();
        for window in windows {
            let mut window = window.lock().unwrap();
            crate::composer::swap(&mut self.state, &mut window.view);
            std::mem::swap(&mut self.draft_due, &mut window.draft_due);
            for mut input in std::mem::take(&mut window.pending) {
                for event in &mut input.events {
                    if let UiEvent::ComposeDrop(paths) = event {
                        let account = self.state.compose.as_ref().map(|c| c.account.clone()).or_else(|| self.state.compose_account());
                        paths.retain(|path| {
                            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
                                dropped_documents.push((account.clone(),path.clone())); false
                            } else { true }
                        });
                    }
                }
                let close = input.events.iter().any(|e| matches!(e, UiEvent::CloseCompose | UiEvent::Command("compose.close", _)));
                if close && self.composers.hold_owner(window.viewport, &mut self.state) {
                    input.events.retain(|e| !matches!(e, UiEvent::CloseCompose | UiEvent::Command("compose.close", _)));
                }
                if input.events.iter().any(|e| matches!(e, UiEvent::ConfirmDraft(crate::state::DraftChoice::Back))) {
                    self.composers.quitting = false;
                    self.composers.owner_closing = false;
                    cancelled_quit = true;
                }
                let quit = input.events.iter().any(|e| matches!(e, UiEvent::Command("file.quit", _)));
                let origin = input.request.and_then(|id| self.drive_callers.get(&id)).cloned().map(|mut o| { o.mixed_input = input.mixed; o });
                apply_ui_events(&mut self.state, &registry, input.events, origin);
                if quit { self.composers.quitting = true; }
                // Per-window Quit confirmations never terminate siblings.
                self.state.exiting = false;
                if close && self.state.compose.is_none() && !self.state.documents.opening() { window.closed = true; }
                self.ctx.request_repaint_of(window.viewport);
            }
            self.draft_timer();
            self.settle();
            std::mem::swap(&mut self.draft_due, &mut window.draft_due);
            crate::composer::swap(&mut self.state, &mut window.view);
        }
        if cancelled_quit {
            for window in self.composers.windows.values() {
                let mut window = window.lock().unwrap();
                if window.view.confirm == Some(crate::state::Confirm::Quit) { window.view.confirm = None; }
            }
        }
        let (runs, lost) = registry.take_executed();
        if !runs.is_empty() || lost != 0 {
            self.bus.publish(&toolkit::executed::topic(self.bus.service()), runs.iter().map(toolkit::executed::Executed::body).collect(), lost);
        }
        if std::mem::take(&mut self.state.compose_new_requested) {
            self.state.compose();
        }
        self.sync_composers();
        for (account,path) in dropped_documents {
            self.open_dropped_document(account,path);
        }
        self.advance_close();
    }

    fn open_dropped_document(&mut self, account: Option<String>, path: PathBuf) {
        let result = account.ok_or_else(|| (code::UNKNOWN_ACCOUNT,label("no-account")))
            .and_then(|a| self.state.read_document(a,path,None,false,false));
        if let Err((_,message)) = result { self.state.status = message; }
        self.sync_composers();
        self.settle();
    }

    fn advance_close(&mut self) {
        if self.state.exiting { self.state.exiting = false; self.composers.quitting = true; }
        let children = self.composers.windows.len() > usize::from(self.composers.root.is_some());
        if self.composers.quitting {
            // Children first; the fixed root owner is always the final window.
            let next = self.composers.windows.values().find(|w| w.lock().unwrap().viewport != egui::ViewportId::ROOT)
                .or_else(|| self.composers.windows.values().next()).cloned();
            if let Some(window) = next {
                let mut window = window.lock().unwrap();
                if window.view.confirm.is_none() {
                    crate::composer::swap(&mut self.state, &mut window.view);
                    self.state.apply_ui(UiEvent::CloseCompose);
                    if self.state.confirm.is_some() { self.state.confirm = Some(crate::state::Confirm::Quit); }
                    else { window.closed = true; }
                    self.settle();
                    crate::composer::swap(&mut self.state, &mut window.view);
                    self.ctx.send_viewport_cmd_to(window.viewport, egui::ViewportCommand::Focus);
                    self.ctx.request_repaint_of(window.viewport);
                }
            } else { self.state.exiting = true; }
        } else if self.composers.owner_closing && !children {
            if let Some(window) = self.composers.root.as_ref().and_then(|n| self.composers.windows.get(n)) {
                let mut window = window.lock().unwrap();
                if window.view.confirm.is_none() {
                    window.pending.push(crate::composer::Input { events: vec![UiEvent::CloseCompose], request: None, mixed: false });
                    self.ctx.request_repaint();
                }
            } else { self.state.exiting = true; }
        }
        if self.composers.compose_only && self.composers.windows.is_empty() { self.state.exiting = true; }
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
        self.state.prepare_previews(&self.ctx);
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
        self.apply_drafts();
        self.publish_executed();
        self.save_config_if_dirty();
    }

    /// Carry out the state's draft saves and removals on the store. A
    /// failure is shown: the draft is still open, and the next save tries
    /// again.
    fn apply_drafts(&mut self) {
        let ops = self.state.take_draft_ops();
        if ops.is_empty() {
            return;
        }
        for error in crate::drafts::apply(self.drafts.as_mut(), ops) {
            eprintln!("email: draft: {error}");
            self.state.status = crate::label_with("draft-save-failed", &[("error", &error)]);
        }
    }

    /// Save the open draft's edits within [`DRAFT_DEBOUNCE`] of the first
    /// unsaved one; a timed repaint wakes the window for it (no polling).
    fn draft_timer(&mut self) {
        if self.state.draft_unsaved().is_none() {
            self.draft_due = None;
            return;
        }
        let now = std::time::Instant::now();
        let due = *self.draft_due.get_or_insert(now + DRAFT_DEBOUNCE);
        if now >= due {
            self.draft_due = None;
            self.state.save_draft();
            self.apply_drafts();
        } else {
            self.ctx.request_repaint_after(due - now);
        }
    }

    /// Publish the commands run since the last call on
    /// `<service>.action.executed`.
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
            Work::MailDocument(work) => self.spawn(async move {
                let (ticket, answer) = tokio::task::spawn_blocking(move || work.perform())
                    .await
                    .expect("mail document worker panicked");
                Event::MailDocument(ticket, answer)
            }),
            Work::InspectComposeAttachments(inspection) => {
                self.spawn(inspect_compose_attachments(inspection));
            }
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
                                max_size_upload: client.session().max_size_upload,
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
                sort,
                position,
                limit,
            } => {
                if let Some((tag, client)) = self.client(&account) {
                    self.spawn(async move {
                        let page = client.email_page(&mailbox, sort, position, limit).await;
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
                        let answer = crate::commands::run_query(&client, query).await;
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
                let sent = self.state.send_boxes(&account).ok();
                match self.client(&account) {
                    // A form's write through a connection other than the one
                    // it was opened on is refused as stale.
                    Some((tag, _)) if expect.is_some_and(|g| g != tag.generation) => {
                        let error = jmap::Error::Protocol(label("stale-connection"));
                        self.state.wrote(&tag, reply, form, write, Err(error));
                    }
                    Some((tag, client)) => {
                        if let Write::Send { draft } = &write {
                            let caller = reply
                                .map(|id| self.bus.caller_key(id))
                                .unwrap_or_else(|| format!("local:{}", self.bus.service()));
                            audit_attachments(&caller, draft);
                        }
                        self.spawn(async move {
                            let answer =
                                crate::commands::run_write(&client, write.clone(), sent).await;
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
            Work::Batch {
                account,
                generation,
                batch,
            } => {
                // Not sent at all: every id fails with the reason, so the
                // list puts back what it showed ahead of the server.
                let refused = |batch: &crate::work::Batch, why: String| jmap::SetOutcome {
                    done: Vec::new(),
                    failed: batch
                        .ids
                        .iter()
                        .map(|id| (id.clone(), why.clone()))
                        .collect(),
                };
                match self.client(&account) {
                    Some((tag, _)) if tag.generation != generation => {
                        let outcome = refused(&batch, label("stale-connection"));
                        self.state
                            .apply(Event::Account(tag, Done::Batched(batch, outcome)));
                    }
                    Some((tag, client)) => {
                        self.spawn(async move {
                            let outcome = client.change_emails(&batch.ids, &batch.change).await;
                            Event::Account(tag, Done::Batched(batch, outcome))
                        });
                    }
                    None => {
                        let tag = Tag {
                            account,
                            generation,
                        };
                        let outcome = refused(&batch, label("not-connected"));
                        self.state
                            .apply(Event::Account(tag, Done::Batched(batch, outcome)));
                    }
                }
            }
            Work::FetchIds {
                account,
                ticket,
                mailbox,
                sort,
                position,
            } => {
                match self.client(&account) {
                    Some((tag, client)) => {
                        self.spawn(async move {
                            let limit = jmap::MAX_IDS_PER_REQUEST as u64;
                            let page = client.email_ids(&mailbox, sort, position, limit).await;
                            Event::Account(tag, Done::Ids(ticket, page))
                        });
                    }
                    // Settled at once: "Fetching N…" never waits forever.
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
                        self.state
                            .apply(Event::Account(tag, Done::Ids(ticket, Err(error))));
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
                attempt,
                origin,
            } => {
                // The send recipe needs the Drafts and Sent mailboxes;
                // without the mail listing yet, there is nothing to file
                // under. Either way the compose hears back, so it never
                // stays busy.
                let boxes = self.state.send_boxes(&account);
                match (self.client(&account), boxes) {
                    (Some((tag, client)), Ok(boxes)) => {
                        audit_compose_attachments(&origin, &draft);
                        self.spawn(async move {
                            let (attempt, result) = client.send(&draft, &boxes, attempt).await;
                            Event::Account(tag, Done::Sent(ticket, attempt, result))
                        });
                    }
                    (found, missing) => {
                        let generation = self
                            .state
                            .account(&account)
                            .map_or(0, crate::state::AccountView::generation);
                        let tag = found.map(|(tag, _)| tag).unwrap_or(Tag {
                            account,
                            generation,
                        });
                        let key = missing.err().unwrap_or("not-connected");
                        let error = jmap::Error::Protocol(label(key));
                        self.state
                            .apply(Event::Account(tag, Done::Sent(ticket, attempt, Err(error))));
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
                                    apply.command,
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
            Work::Print { reply, name, html } => self.print(reply, name, html),
            Work::SuggestWait { ticket } => self.spawn(async move {
                tokio::time::sleep(crate::state::SUGGEST_DEBOUNCE).await;
                Event::SuggestDue(ticket)
            }),
            Work::Suggest {
                account,
                ticket,
                text,
            } => {
                if let Some((tag, client)) = self.client(&account) {
                    self.spawn(async move {
                        let found = client
                            .contacts_search(&text, crate::commands::SUGGEST_FETCH)
                            .await;
                        Event::Account(tag, Done::Suggested(ticket, found))
                    });
                }
            }
            Work::Preview {
                account,
                ticket,
                blob,
                kind,
            } => match self.client(&account) {
                Some((_, client)) => self.spawn(async move {
                    let result = match client.download_blob(&blob).await {
                        Ok(bytes) => tokio::task::spawn_blocking(move || {
                            crate::previews::decode(&bytes, kind)
                        })
                        .await
                        .unwrap_or_else(|error| Err(error.to_string())),
                        Err(error) => Err(error.to_string()),
                    };
                    Event::Preview(ticket, result)
                }),
                None => self
                    .state
                    .apply(Event::Preview(ticket, Err(label("not-connected")))),
            },
            Work::Attachment {
                account,
                email,
                part,
                blob,
                name,
                open,
                dir,
                reply,
            } => match self.client(&account) {
                Some((tag, client)) => self.spawn(async move {
                    let result = attachment(&client, &blob, name, open, dir).await;
                    let done = AttachmentDone {
                        email,
                        part,
                        open,
                        reply,
                        result,
                    };
                    Event::Account(tag, Done::Attachment(Box::new(done)))
                }),
                None => self.not_connected(reply),
            },
        }
    }

    /// Show Prefs on its Email section: ask a running Prefs, else have the
    /// session's launcher (`apps.launch`, as Prefs launches Email) start it
    /// and ask again once it registers. The app has no launcher of its own:
    /// without one, the status line says so.
    fn open_prefs(&mut self) {
        let bus = self.bus.clone();
        self.spawn(async move {
            let result = citizen::call_app(
                &bus,
                PREFS_APP_ID,
                "prefs.section",
                json!({"name":"apps_email"}),
                std::time::Duration::from_secs(10),
            )
            .await
            .map(|_| ())
            .map_err(prefs_open_error);
            Event::PrefsOpened(result)
        });
    }

    /// Write a print page off the UI thread and hand it to the browser
    /// through the session's launcher (`apps.launch`).
    fn print(&mut self, reply: Option<u64>, name: String, html: String) {
        let bus = self.bus.clone();
        self.spawn(async move {
            let written = tokio::task::spawn_blocking(move || {
                let dir = print_dir()?;
                write_print(&dir, &name, &html, std::time::SystemTime::now())
            })
            .await;
            let path = match written {
                Ok(Ok(path)) => path,
                Ok(Err(error)) => return Event::Printed(reply, Err(error)),
                Err(error) => return Event::Printed(reply, Err(error.to_string())),
            };
            let uri = format!("file://{}", path.display());
            let launched = bus
                .call(
                    "apps",
                    "apps.launch",
                    json!({"id": BROWSER_APP_ID, "uris": [uri]}),
                )
                .await;
            let opened = launched.is_ok_and(|r| r.rc == 0);
            Event::Printed(reply, Ok((path.display().to_string(), opened)))
        });
    }

    /// Apply every finished event, settling after each one, so a later
    /// completion can never change what an earlier one acted on.
    fn pump(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match &event {
                Event::Delivery(citizen::Delivery::Connected) => {
                    drive::set_service(&self.ctx, Some(self.bus.service()))
                }
                Event::Delivery(citizen::Delivery::Disconnected) => {
                    drive::set_service(&self.ctx, None)
                }
                _ => {}
            }
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
            if let Event::Delivery(citizen::Delivery::Command {
                id,
                command: command_name,
                body,
            }) = event
            {
                if self.exiting {
                    self.refuse_closing(id);
                } else {
                    self.held.push_back((id, command_name, body));
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
            if let Some(window) = self.composers.target(&event) {
                let mut window = window.lock().unwrap();
                crate::composer::swap(&mut self.state, &mut window.view);
                self.state.apply(event);
                self.settle();
                crate::composer::swap(&mut self.state, &mut window.view);
                self.ctx.request_repaint_of(window.viewport);
            } else {
                self.state.apply(event);
                self.settle();
            }
            self.sync_composers();
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
    /// before it is answered, so a caller's clicks and its later commands act
    /// in the order sent.
    fn release(&mut self) {
        while let Some((_, command_name, _)) = self.held.front() {
            let joins = command_name
                .strip_prefix("email.")
                .is_some_and(drive::queues);
            if self.exiting || (drive::pending(&self.ctx) && !joins) {
                return;
            }
            let (id, command_name, body) = self.held.pop_front().expect("a held command");
            self.command(id, &command_name, &body);
        }
    }

    /// One Bus command: the commands layer answers or queues it; the drive
    /// commands and the window's activation are the shell's.
    fn command(&mut self, id: u64, command_name: &str, body: &str) {
        let args = serde_json::from_str::<Value>(body).ok().filter(Value::is_object);
        let routed = command_name.starts_with("email.compose.") || command_name == "email.info"
            || (command_name == "email.execute" && args.as_ref().is_some_and(|a| a["id"].as_str().is_some_and(|id| id.starts_with("compose.") || id == "mail.send")));
        let target = if routed { args.as_ref().and_then(|a| self.composers.selected(a)) } else { None };
        if routed && args.as_ref().is_some_and(|a| a.get("window").is_some()) && target.is_none() {
            self.state.refuse(id, code::NOT_FOUND, &label("compose-not-open"));
            self.settle();
            return;
        }
        if command_name == "email.info" && args.is_some() {
            let mut info = self.state.info();
            if let Some(window) = &target {
                let view = window.lock().unwrap().view.info();
                for field in ["compose", "confirm", "status"] { info[field] = view[field].clone(); }
            }
            info["window"] = target.as_ref().map(|w| json!(w.lock().unwrap().name)).unwrap_or(Value::Null);
            info["compose_windows"] = Value::Array(self.composers.windows.values().map(|w| {
                let w = w.lock().unwrap();
                json!({"window":w.name,"root":w.viewport == egui::ViewportId::ROOT,"compose":w.view.info()["compose"]})
            }).collect());
            self.bus.reply(id,0,info);
            return;
        }
        let mut target = target.as_ref().map(|w| w.lock().unwrap());
        if let Some(window) = &mut target { crate::composer::swap(&mut self.state, &mut window.view); }
        let origin = SendOrigin {
            caller_key: self.bus.caller_key(id),
            drive_request_id: None,
            mixed_input: false,
        };
        let forward = self.state.with_send_origin(Some(origin), |state| {
            crate::commands::command(state, &self.commands, &self.strings, id, command_name, body)
        });
        self.settle();
        if let Some(window) = &mut target {
            crate::composer::swap(&mut self.state, &mut window.view);
            self.ctx.request_repaint_of(window.viewport);
        }
        drop(target);
        match forward {
            Some(Forward::Drive {
                id,
                command: command_name,
                args,
            }) => {
                if let Some(answer) = request_drive(
                    &self.ctx,
                    &mut self.drive_callers,
                    id,
                    self.bus.caller_key(id),
                    &command_name,
                    &args,
                ) {
                    self.bus.reply(answer.id, answer.rc, answer.body);
                }
            }
            Some(Forward::Show { id }) => {
                self.shows.insert(id);
                let (bus, comp) = (self.bus.clone(), self.comp.clone());
                let app_id = self.composers.root.as_ref().map(|name| format!("{APP_ID}.{name}")).unwrap_or_else(|| APP_ID.into());
                self.ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
                self.spawn(async move { Event::Shown(id, citizen::show(bus, comp, &app_id).await) });
            }
            None => {}
        }
        self.settle();
        self.sync_composers();
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
            .map(|look| toolkit::session_look::theme_or(look, &self.file_theme))
            .unwrap_or_else(|| self.file_theme.clone())
            .with_ui(self.common_ui.clone())
    }

    /// Apply the profile's bindings for email over the built chords.
    pub fn set_keymap(&mut self, keymap: toolkit::keymap::Overrides) {
        if keymap == self.keymap {
            return;
        }
        for note in self.commands.rebind(&keymap) {
            eprintln!("email: keymap: {note}");
        }
        self.composers.keymap = keymap.clone();
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
                .raw_within(
                    read.service,
                    read.command,
                    read.body.to_string(),
                    LOOK_BUDGET,
                )
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
                ui,
                custom_source,
                keymaps,
                email,
                ..
            }) => {
                toolkit::debug!(app = "email", "settings applied");
                self.common_ui = ui;
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
        actions::choice(&self.state)
    }

    /// Install the session theme with the window's choice over it. Neither
    /// is ever written. The session mode is recorded so the title-bar
    /// toggle inverts what is actually drawn.
    fn install_theme(&mut self) {
        let choice = self.choice();
        let theme = match self.theme.with_choice(&choice) {
            Ok(theme) => theme,
            Err(error) => {
                eprintln!("toolkit: theme choice refused: {error}");
                return;
            }
        };
        toolkit::install(&self.ctx, &theme);
        self.state.set_session_mode(self.theme.mode());
        self.installed = choice;
        self.palette = self.editor_palette();
        self.composer_light = self.state.settings.composer.light;
    }

    /// The choice changed since the theme was installed: install again.
    fn follow_theme(&mut self) {
        if self.choice() != self.installed {
            self.install_theme();
            self.ctx.request_repaint();
        }
        if self.composer_light != self.state.settings.composer.light {
            self.palette = self.editor_palette();
            self.composer_light = self.state.settings.composer.light;
        }
    }

    /// One frame's logic; it runs even while the window is hidden.
    pub fn logic(&mut self, ctx: &egui::Context) {
        // Root closes can arrive on a logic-only wake. Cancel before eframe
        // decides process lifetime, even if Wayland grants no paint callback.
        if self.composers.compose_only && !self.exiting && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if let Some(window) = self.composers.root.as_ref().and_then(|n| self.composers.windows.get(n)) {
                window.lock().unwrap().pending.push(crate::composer::Input {
                    events: vec![UiEvent::CloseCompose], request: None, mixed: false,
                });
            }
        }
        self.composer_input();
        // Drive answers first: a `window close` answer goes out before the
        // close it asked for is handled.
        drive::publish_titlebar(ctx, |topic, bodies, lost| {
            self.bus.publish(topic, bodies, lost);
        });
        for answer in drive::logic(ctx) {
            self.drive_callers.remove(&answer.id);
            self.bus.reply(answer.id, answer.rc, answer.body);
        }
        self.pump();
        self.start_composer();
        self.sync_composers();
        self.advance_close();
        self.follow_theme();
        self.draft_timer();
        // Closing waits for the drive layer's accepted work. A close is
        // asked for by the window (the close button) or a command
        // (file.quit, which sets the state's exiting flag). A draft open
        // with something typed is saved and the person asked first.
        let asked = !self.composers.compose_only && ctx.input(|i| i.viewport().close_requested());
        let mut closing = self.state.exiting;
        if asked && !closing && !self.exiting {
            if self.composers.hold_owner(egui::ViewportId::ROOT, &mut self.state) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.request_repaint();
            } else if self.state.request_quit() {
                closing = true;
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.apply_drafts();
                ctx.request_repaint();
            }
        }
        if closing && !self.exiting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.exiting = true;
            // Whatever is typed is on disk before anything else closes.
            self.state.save_draft();
            for window in self.composers.windows.values() {
                let mut window = window.lock().unwrap();
                crate::composer::swap(&mut self.state, &mut window.view);
                self.state.save_draft();
                self.state.cancel_document_open();
                crate::composer::swap(&mut self.state, &mut window.view);
            }
            toolkit::saved::secondary::close(ctx, APP);
            self.apply_drafts();
            for answer in drive::finish(&self.ctx) {
                self.drive_callers.remove(&answer.id);
                self.bus.reply(answer.id, answer.rc, answer.body);
            }
            // Every accepted command gets its answer before the Bus stops.
            for (id, _, _) in std::mem::take(&mut self.held) {
                self.refuse_closing(id);
            }
            self.drain_saver();
            self.state.close_out();
            for reply in self.state.take_replies() {
                self.bus.reply(reply.id, reply.rc, reply.body);
            }
            self.apply_drafts();
            self.save_config_if_dirty();
            self.bus.quit();
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// The composer's colours follow its surface or the window theme.
    fn editor_palette(&self) -> editor::Palette {
        if self.state.settings.composer.light {
            editor::Palette::from_theme(toolkit::content::light_theme())
        } else {
            editor::Palette::from_theme(
                &self
                    .theme
                    .with_choice(&self.choice())
                    .unwrap_or_else(|_| self.theme.clone()),
            )
        }
    }

    /// One frame's drawing and interaction.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if self.exiting { return; }
        if self.composers.compose_only {
            self.composers.show(ui);
            return;
        }
        let fired = crate::actions::shortcuts(ui.ctx(), &self.state, &self.commands);
        let mut events = view(
            ui,
            &self.state,
            &self.commands,
            &self.strings,
            &self.palette,
        );
        toolkit::markdown::open_links(ui.ctx(), "email");
        events.extend(
            fired
                .into_iter()
                .map(|id| UiEvent::Command(id, Origin::Shortcut)),
        );
        let origin = driven_origin(ui.ctx(), &self.drive_callers);
        apply_ui_events(&mut self.state, &self.commands, events, origin);
        self.state.document_chooser_ui(ui.ctx());
        if self.state.compose.is_none() {
            self.state.attachment_chooser = None;
            self.state.attachment_notice = None;
        }
        let picked = self
            .state
            .attachment_chooser
            .as_mut()
            .and_then(|chooser| chooser.show(ui.ctx()));
        if let Some(outcome) = picked {
            ui.ctx().request_repaint();
            self.state.attachment_chooser = None;
            if let toolkit::file_chooser::Outcome::Picked(paths) = outcome
                && let Err((_, message)) = self.state.add_compose_attachments(&paths)
            {
                self.state.attachment_notice = Some(message);
            }
        }
        {
            let dropped = ui.ctx().input(|input| input.raw.dropped_files.clone());
            if !dropped.is_empty() {
                ui.ctx().request_repaint();
            }
            let mut errors = Vec::new();
            let mut paths = Vec::new();
            for file in dropped {
                // egui gives every drop a path; only a native drop's is absolute.
                let path = file.path();
                if path.is_absolute() {
                    paths.push(path.to_path_buf());
                } else {
                    errors.push(label("compose-drop-no-path"));
                }
            }
            if !paths.is_empty() {
                for path in paths {
                    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
                        self.open_dropped_document(self.state.compose_account(),path);
                    }
                }
            }
            if !errors.is_empty() {
                self.state.attachment_notice = Some(errors.join("\n"));
            }
        }
        self.settle();
        self.sync_composers();
        self.composers.show(ui);
        // A theme change is installed by the next frame's logic, before any
        // drawing: never midway through the frame that saw the click. So is
        // a close the person just confirmed.
        if self.choice() != self.installed || self.state.exiting {
            ui.ctx().request_repaint();
        }
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
    fn on_exit(&mut self) {
        toolkit::macros::close_app(&self.ctx, "email");
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        Shell::logic(self, ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        Shell::ui(self, ui);
    }
}

fn prefs_open_error(error: citizen::AppCallError) -> String {
    eprintln!("email: opening Prefs failed: {error}");
    crate::label(match error {
        citizen::AppCallError::LaunchFailed(_) => "no-launcher",
        citizen::AppCallError::DidNotAppear
        | citizen::AppCallError::Transport(_)
        | citizen::AppCallError::Refused { .. } => "prefs-silent",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compose_fixture(standalone: bool) -> (Shell, test_broker::Broker) {
        let broker = test_broker::Broker::start();
        let (bus, _deliveries) = citizen::start(APP, "email-windows-test", &broker.url).unwrap();
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let theme = Theme::embedded();
        toolkit::install(&ctx,&theme);
        drive::install(&ctx,APP);
        toolkit::saved::Store::<Value>::load(None,0).install_windows(&ctx,APP);
        let (tx,rx) = mpsc::channel();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let mut state = crate::state::tests::connected_many(&["a"]);
        state.take_work();
        let shell = Shell {
            composers: crate::composer::Windows { compose_only:standalone,..Default::default() },
            launch_compose:false,launch_document:None,state,commands:actions::registry(),strings:crate::strings(),
            palette:editor::Palette::from_theme(&theme),composer_light:false,theme:theme.clone(),bus,
            drive_callers:HashMap::new(),runtime,tx,rx,ctx,clients:HashMap::new(),latest:HashMap::new(),
            adopted:Arc::new(Mutex::new(Vec::new())),exiting:false,installed:Default::default(),file_theme:theme,
            look:None,common_ui:Default::default(),keymap:Default::default(),follower:settings::follow::Follower::new("test"),
            look_retry:0,config:None,saver:None,saver_thread:None,applies:0,held:Default::default(),comp:"comp".into(),shows:Default::default(),
            drafts:Box::new(MemoryStore::default()),draft_due:None,
        };
        (shell,broker)
    }

    fn add_composer(shell: &mut Shell, subject: &str) {
        shell.state.compose();
        shell.state.apply_ui(UiEvent::ComposeField(crate::state::ComposeField::Subject,subject.into()));
        shell.sync_composers();
    }

    fn composer_choice(shell: &mut Shell, name: &str, choice: crate::state::DraftChoice) {
        shell.composers.windows[name].lock().unwrap().pending.push(crate::composer::Input {
            events:vec![UiEvent::ConfirmDraft(choice)],request:None,mixed:false,
        });
        shell.composer_input();
    }

    #[test]
    fn quit_asks_each_draft_and_cancel_leaves_the_remaining_window_alive() {
        use crate::state::{Confirm,DraftChoice};
        let (mut shell,_broker) = compose_fixture(false);
        add_composer(&mut shell,"one"); add_composer(&mut shell,"two");
        shell.state.exiting = true;
        shell.advance_close();
        assert_eq!(shell.composers.windows["compose-1"].lock().unwrap().view.confirm,Some(Confirm::Quit));
        assert!(shell.composers.windows["compose-2"].lock().unwrap().view.confirm.is_none());
        composer_choice(&mut shell,"compose-1",DraftChoice::Keep);
        assert!(!shell.state.exiting);
        assert!(!shell.composers.windows.contains_key("compose-1"));
        assert_eq!(shell.composers.windows["compose-2"].lock().unwrap().view.confirm,Some(Confirm::Quit));
        composer_choice(&mut shell,"compose-2",DraftChoice::Back);
        assert!(!shell.composers.quitting);
        assert!(shell.composers.windows["compose-2"].lock().unwrap().view.confirm.is_none());
        shell.state.exiting = true; shell.advance_close();
        composer_choice(&mut shell,"compose-2",DraftChoice::Discard);
        assert!(shell.composers.windows.is_empty());
        assert!(shell.state.exiting);
        shell.bus.quit();
    }

    #[test]
    fn mailbox_close_waits_for_composers_and_last_child_finishes_the_process() {
        let (mut shell,_broker) = compose_fixture(false);
        add_composer(&mut shell,"one");
        assert!(shell.composers.hold_owner(egui::ViewportId::ROOT,&mut shell.state));
        shell.advance_close();
        assert!(!shell.state.exiting);
        let window = shell.composers.windows["compose-1"].clone();
        window.lock().unwrap().pending.push(crate::composer::Input {events:vec![UiEvent::CloseCompose],request:None,mixed:false});
        shell.composer_input();
        composer_choice(&mut shell,"compose-1",crate::state::DraftChoice::Keep);
        assert!(shell.state.exiting);
        shell.bus.quit();
    }

    #[test]
    fn root_close_on_a_logic_only_wake_keeps_child_composers_alive() {
        let (mut shell,_broker) = compose_fixture(true);
        add_composer(&mut shell,"owner"); add_composer(&mut shell,"child");
        let ctx = shell.ctx.clone();
        let mut input = egui::RawInput::default();
        input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap().events.push(egui::ViewportEvent::Close);
        let output = ctx.run_ui(input,|_| shell.logic(&ctx));
        assert!(output.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::CancelClose));
        assert!(!shell.exiting && !shell.state.exiting);
        assert!(shell.composers.owner_closing);
        assert_eq!(shell.composers.root.as_deref(),Some("compose-1"));
        assert_eq!(shell.composers.windows.len(),2);
        output.drop_without_applying_deltas();
        shell.bus.quit();
    }

    #[test]
    fn bus_attachment_commands_use_the_named_window_and_focus_default() {
        let (mut shell,_broker) = compose_fixture(false);
        add_composer(&mut shell,"one"); add_composer(&mut shell,"two");
        for (name,path) in [("compose-1","/home/user/one.txt"),("compose-2","/home/user/two.txt")] {
            let window = shell.composers.windows[name].clone();
            window.lock().unwrap().view.compose.as_mut().unwrap().draft.attachments.push(path.into());
        }
        shell.command(1,"email.compose.attachment.remove",r#"{"window":"compose-1","path":"/home/user/one.txt"}"#);
        assert!(shell.composers.windows["compose-1"].lock().unwrap().view.compose.as_ref().unwrap().draft.attachments.is_empty());
        assert_eq!(shell.composers.windows["compose-2"].lock().unwrap().view.compose.as_ref().unwrap().draft.attachments.len(),1);
        shell.command(2,"email.compose.attachment.remove",r#"{"path":"/home/user/two.txt"}"#);
        assert!(shell.composers.windows["compose-2"].lock().unwrap().view.compose.as_ref().unwrap().draft.attachments.is_empty());
        shell.bus.quit();
    }

    fn pending_attachments(
        state: &mut State,
        opening: bool,
        paths: &[PathBuf],
    ) -> crate::work::ComposeInspection {
        let args = if opening {
            serde_json::json!({"to":"ann@example.invalid", "subject":"Files", "attachments":paths})
        } else {
            serde_json::json!({"paths":paths})
        };
        crate::commands::command(
            state,
            &crate::actions::registry(),
            &crate::strings(),
            42,
            if opening {
                "email.compose"
            } else {
                "email.compose.attachment.add"
            },
            &args.to_string(),
        );
        assert!(
            state.take_replies().is_empty(),
            "inspection must precede the reply"
        );
        state
            .take_work()
            .into_iter()
            .find_map(|work| match work {
                Work::InspectComposeAttachments(inspection) => Some(inspection),
                _ => None,
            })
            .expect("queued filesystem inspection")
    }

    fn inspect(inspection: crate::work::ComposeInspection) -> Event {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let event = runtime.block_on(inspect_compose_attachments(inspection));
        runtime.shutdown_timeout(std::time::Duration::from_millis(100));
        event
    }

    #[test]
    fn compose_attachment_replies_follow_background_inspection_and_keep_order() {
        let dir = tempfile::tempdir().unwrap();
        let one = dir.path().join("one.txt");
        let two = dir.path().join("two.txt");
        std::fs::write(&one, b"one").unwrap();
        std::fs::write(&two, b"two").unwrap();
        let mut state = crate::state::tests::connected_many(&["a"]);
        for opening in [true, false] {
            let paths = if opening {
                vec![one.clone()]
            } else {
                vec![one.clone(), two.clone(), one.clone()]
            };
            let pending = pending_attachments(&mut state, opening, &paths);
            assert!(!state.compose_sendable());
            state.apply_ui(UiEvent::SendCompose);
            assert!(
                state.take_work().is_empty(),
                "Send must wait for inspection"
            );
            let event = inspect(pending);
            assert!(
                state.take_replies().is_empty(),
                "a worker cannot reply before state accepts its result"
            );
            state.apply(event);
            let replies = state.take_replies();
            assert_eq!(replies.len(), 1);
            assert_eq!((replies[0].id, replies[0].rc), (42, 0));
            assert_eq!(replies[0].body["compose"], state.info()["compose"]);
            if opening {
                assert_eq!(replies[0].body["composing"], "a");
            }
        }
        assert_eq!(
            state.compose.as_ref().unwrap().draft.attachments,
            [one, two]
        );
    }

    #[test]
    fn late_inspection_never_attaches_to_a_closed_replaced_or_edited_compose() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("one.txt");
        std::fs::write(&path, b"one").unwrap();
        for opening in [true, false] {
            for change in ["close", "replace", "edit"] {
                let mut state = crate::state::tests::connected_many(&["a"]);
                if !opening {
                    state.open_compose(Some("a"), jmap::Composed::default());
                    state.take_work();
                }
                let pending = pending_attachments(&mut state, opening, std::slice::from_ref(&path));
                match change {
                    "close" => state.compose = None,
                    "replace" => {
                        state.open_compose(Some("a"), jmap::Composed::default());
                    }
                    _ => state.apply_ui(UiEvent::ComposeField(
                        crate::state::ComposeField::Subject,
                        "Edited".into(),
                    )),
                }
                state.apply(inspect(pending));
                let replies = state.take_replies();
                assert_eq!(replies.len(), 1);
                assert_eq!(replies[0].id, 42);
                assert_eq!(
                    replies[0].body["error_code"],
                    if change == "close" {
                        code::NOT_FOUND
                    } else {
                        code::STALE
                    }
                );
                if let Some(compose) = &state.compose {
                    assert!(compose.draft.attachments.is_empty());
                    assert!(compose.attachments.is_empty());
                    assert!(compose.attachment_inspections.is_empty());
                }
            }
        }
    }

    #[test]
    fn attachment_batch_failure_is_atomic_and_answers_only_after_inspection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("one.txt");
        std::fs::write(&path, b"one").unwrap();
        let mut state = crate::state::tests::connected_many(&["a"]);
        state.open_compose(Some("a"), jmap::Composed::default());
        state.take_work();
        let revision = state.compose.as_ref().unwrap().revision;
        let pending = pending_attachments(&mut state, false, &[path, dir.path().join("missing")]);
        state.apply(inspect(pending));
        let replies = state.take_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].body["error_code"], code::ATTACHMENT_NOT_FILE);
        let compose = state.compose.as_ref().unwrap();
        assert!(compose.draft.attachments.is_empty());
        assert_eq!(compose.revision, revision);
    }

    #[test]
    fn opening_prefs_errors_use_the_catalogue() {
        use citizen::AppCallError;

        for (error, key) in [
            (
                AppCallError::LaunchFailed("launcher detail".into()),
                "no-launcher",
            ),
            (AppCallError::DidNotAppear, "prefs-silent"),
            (
                AppCallError::Transport(citizen::CallError::from("transport detail")),
                "prefs-silent",
            ),
            (
                AppCallError::Refused {
                    code: "REFUSED".into(),
                    message: "refusal detail".into(),
                    reply: citizen::Reply::default(),
                },
                "prefs-silent",
            ),
        ] {
            let text = prefs_open_error(error);
            assert_eq!(text, crate::label(key));
            assert_ne!(text, key, "the Fluent key must still exist");
            assert!(!text.contains("detail"));
        }
    }

    #[derive(Clone)]
    struct AuditWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for AuditWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn attachment_audit_logs_one_line_with_caller_and_paths_only() {
        for caller in ["window", "local:sender", "mesh:sender@peer"] {
            let bytes = Arc::new(Mutex::new(Vec::new()));
            let writer = bytes.clone();
            let subscriber = tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_writer(move || AuditWriter(writer.clone()))
                .finish();
            let draft = jmap::Composed {
                attachments: vec![PathBuf::from("/home/user/report.pdf")],
                body: "private body".into(),
                subject: "private subject".into(),
                ..jmap::Composed::default()
            };
            tracing::subscriber::with_default(subscriber, || {
                audit_attachments(caller, &draft);
                audit_attachments(caller, &jmap::Composed::default());
            });
            let log = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
            assert_eq!(log.lines().count(), 1, "{log}");
            assert!(log.contains("INFO"), "{log}");
            assert!(log.contains(&format!("caller_key=\"{caller}\"")), "{log}");
            assert!(log.contains("mixed_input=false"), "{log}");
            assert!(log.contains("/home/user/report.pdf"), "{log}");
            assert!(!log.contains("private body") && !log.contains("private subject"));
        }
    }

    #[test]
    fn drive_audit_keeps_request_id_and_caller_for_later_frame_correlation() {
        check_drive_audit(false);
    }

    #[test]
    fn mixed_input_audit_keeps_drive_caller_and_marks_ambiguity() {
        check_drive_audit(true);
    }

    fn check_drive_audit(mixed_input: bool) {
        use egui_kittest::Harness;

        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = bytes.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || AuditWriter(writer.clone()))
            .finish();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        std::fs::write(&path, b"hello").unwrap();
        let mut state = crate::state::tests::connected_many(&["a"]);
        state.open_compose(
            Some("a"),
            jmap::Composed {
                to: jmap::parse_addresses("ann@example.invalid").unwrap(),
                subject: "Files".into(),
                attachments: vec![path],
                ..jmap::Composed::default()
            },
        );
        let registry = actions::registry();
        let strings = crate::strings();
        let theme = Theme::for_context(design::DesignContext::default());
        let palette = editor::Palette::from_theme(&theme);
        let mut h = Harness::builder()
            .with_size(egui::vec2(1180.0, 760.0))
            .build_ui_state(
                move |ui, data: &mut (State, HashMap<u64, SendOrigin>)| {
                    for answer in drive::logic(ui.ctx()) {
                        data.1.remove(&answer.id);
                    }
                    let events = view(ui, &data.0, &registry, &strings, &palette);
                    let origin = driven_origin(ui.ctx(), &data.1);
                    apply_ui_events(&mut data.0, &registry, events, origin);
                },
                (state, HashMap::new()),
            );
        toolkit::install(&h.ctx, &theme);
        drive::install(&h.ctx, APP);
        h.run_steps(2);
        let ctx = h.ctx.clone();
        tracing::subscriber::with_default(subscriber, || {
            assert!(
                request_drive(
                    &ctx,
                    &mut h.state_mut().1,
                    42,
                    "mesh:sender@peer".into(),
                    "ui.click",
                    &json!({"label":label("send")})
                )
                .is_none()
            );
            assert!(h.state().0.compose.as_ref().unwrap().send_origin.is_none());
            // The actual drive plugin replays the click on later UI passes.
            for _ in 0..8 {
                if mixed_input {
                    // Local pointer movement shares the pass with the drive
                    // click without changing the target under the pointer.
                    let at = h
                        .ctx
                        .input(|input| input.pointer.latest_pos())
                        .unwrap_or(egui::pos2(10.0, 20.0));
                    h.input_mut().events.push(egui::Event::PointerMoved(at));
                }
                h.step();
            }
            let (draft, origin) = h
                .state_mut()
                .0
                .take_work()
                .into_iter()
                .find_map(|work| {
                    if let Work::Send { draft, origin, .. } = work {
                        Some((draft, origin))
                    } else {
                        None
                    }
                })
                .expect("the replayed click must queue a send");
            assert_eq!(origin.caller_key, "mesh:sender@peer");
            assert_eq!(origin.drive_request_id, Some(42));
            assert_eq!(origin.mixed_input, mixed_input);
            assert_eq!(
                h.state().0.compose.as_ref().unwrap().send_origin.as_ref(),
                Some(&origin)
            );
            audit_compose_attachments(&origin, &draft);
        });
        let log = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        let lines: Vec<_> = log.lines().collect();
        assert_eq!(lines.len(), 2, "{log}");
        assert!(
            lines[0].contains("request_id=42") && lines[0].contains("ui.click"),
            "{log}"
        );
        assert!(
            lines[1].contains("drive_request_id=Some(42)")
                && lines[1].contains("sending email attachments"),
            "{log}"
        );
        assert!(
            lines[1].contains(&format!("mixed_input={mixed_input}")),
            "{log}"
        );
        for line in lines {
            assert!(line.contains("caller_key=\"mesh:sender@peer\""), "{log}");
            assert!(!line.contains("caller_key=\"window\""), "{log}");
        }
    }

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
    fn a_print_page_is_private_whole_and_old_ones_go() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("email-print");
        let now = std::time::SystemTime::now();
        let first = write_print(&dir, "a.html", "<p>a</p>", now).unwrap();
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "<p>a</p>");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&first), 0o600);
        // Printing again replaces the page; nothing partial is left.
        write_print(&dir, "a.html", "<p>b</p>", now).unwrap();
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "<p>b</p>");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // Two days on, the old page goes as a new one is written.
        let later = now + 2 * PRINT_KEEP;
        let second = write_print(&dir, "b.html", "<p>c</p>", later).unwrap();
        assert!(!first.exists() && second.exists());
        assert!(write_print(&dir, "../x.html", "", now).is_err());
        assert!(write_print(&dir, ".hidden", "", now).is_err());
    }

    #[test]
    fn round5_23_the_close_drain_drops_nothing_that_waits() {
        let command = Event::Delivery(citizen::Delivery::Command {
            id: 9,
            command: "email.ping".into(),
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
