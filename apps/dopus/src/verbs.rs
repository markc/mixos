// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The `dopus` Bus port, schema `dopus.v1`: the verbs, every request and
//! reply shape, and the one place a command becomes replies and effects,
//! shared by the window and [`crate::headless`] so the two cannot drift.
//!
//! Every verb is open to local and mesh callers with no authorisation gate.
//! Success is rc 0; a refusal is rc 10 with a [`Refusal`] body.
//!
//! `dopus.action` serves the navigation, view, pane, selection and theme
//! actions. The file operations (`file.open`, `file.new-folder`,
//! `file.rename`, `file.copy-other-pane`, `file.move-other-pane`,
//! `file.delete`) run from the keyboard, the menus and the dialogs, and
//! every `file.*` id is refused over the Bus with [`code::FORBIDDEN`]
//! before [`apply_action_in`] runs. `dopus.open` navigates the two panes.
//!
//! The window also serves the toolkit's drive verbs (`dopus.window`,
//! `dopus.ui.click` and the rest, [`toolkit::drive`]); `dopus.describe`
//! lists them.

use std::path::{Path, PathBuf};

use dopus_core::{DopusCore, PaneId};
use serde::{Deserialize, Serialize};

pub const SERVICE: &str = "dopus";
pub const SCHEMA: &str = "dopus.v1";

/// `(verb, read_only)` in manifest order.
pub const VERBS: &[(&str, bool)] = &[
    ("dopus.ping", true),
    ("dopus.describe", true),
    ("dopus.info", true),
    ("dopus.state", true),
    ("dopus.action", false),
    ("dopus.actions.list", true),
    ("dopus.theme.set", false),
    ("dopus.open", false),
    ("dopus.quit", false),
];

/// Refusal codes (`error_code`).
pub mod code {
    pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
    pub const NOT_FOUND: &str = "NOT_FOUND";
    pub const CONFLICT: &str = "CONFLICT";
    pub const UNAVAILABLE: &str = "UNAVAILABLE";
    pub const INTERNAL: &str = "INTERNAL";
    pub const UNKNOWN_VERB: &str = "UNKNOWN_VERB";
    /// The file operations are not served over the Bus.
    pub const FORBIDDEN: &str = "FORBIDDEN";
}

/// Every refusal body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub error_code: String,
    pub message: String,
    pub reason: Option<String>,
}

// ── ping / describe / info ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingReply {
    pub pong: bool,
    pub service: String,
    pub schema: String,
    pub pid: u32,
    pub headless: bool,
}

/// The `app.describe` control surface (ctk-app-control.v0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescribeReply {
    pub contract: String,
    pub app: String,
    pub title: String,
    pub view: String,
    pub engine: String,
    pub version: String,
    pub description: String,
    pub controls: Vec<serde_json::Value>,
    pub verbs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfoReply {
    pub version: String,
    pub git_sha: String,
    pub build_time: String,
    pub headless: bool,
    pub panes: usize,
    /// The same rows as `dopus.state.panes`.
    pub pane_states: Vec<PaneState>,
    pub config_path: Option<String>,
}

// ── state ────────────────────────────────────────────────────────────────────

/// One pane's state (`pane` 0-based).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneState {
    pub pane: u8,
    pub path: String,
    pub active: bool,
    pub show_hidden: bool,
    /// Thumbnails on (Show Previews).
    #[serde(default)]
    pub previews: bool,
    /// The selection's thumbnail in the shared cache, once made.
    #[serde(default)]
    pub preview: Option<String>,
    /// `name | size | modified`.
    pub sort: String,
    pub ascending: bool,
    pub selected: Option<String>,
    /// All selected paths in visible row order; `selected` is the focused one.
    #[serde(default)]
    pub selected_paths: Vec<String>,
    pub rows: usize,
    /// Relative or absolute, as the status line renders it.
    pub status: String,
    /// Footer text: root totals, a loading ellipsis or a root listing error.
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateReply {
    pub places: dopus_core::config::SidebarConfig,
    pub properties: dopus_core::config::SidebarConfig,
    pub panes: Vec<PaneState>,
    pub theme_scheme: String,
    pub theme_mode: String,
    #[serde(default)]
    pub appearance: AppearanceState,
}

/// What the window draws with; headless leaves these empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppearanceState {
    pub icons: String,
    pub asset_set: Option<String>,
    pub font_ui: String,
    pub font_mono: String,
    #[serde(default)]
    pub font_ui_weight: u16,
    #[serde(default)]
    pub icon_weight: Option<u16>,
}

// ── action / actions.list ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionReq {
    pub id: String,
    #[serde(default)]
    pub pane: PaneTarget,
    #[serde(default)]
    pub args: Option<serde_json::Value>,
}

/// Which pane an action targets; omitted is the active pane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneTarget {
    Left,
    Right,
    #[default]
    Active,
}

impl PaneTarget {
    fn resolve(self, core: &DopusCore) -> PaneId {
        match self {
            Self::Left => PaneId::Left,
            Self::Right => PaneId::Right,
            Self::Active => core.active(),
        }
    }
}

impl<'de> Deserialize<'de> for PaneTarget {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Target {
            Name(String),
            Index(u8),
        }
        match Target::deserialize(deserializer)? {
            Target::Name(name) => match name.as_str() {
                "left" => Ok(Self::Left),
                "right" => Ok(Self::Right),
                "active" => Ok(Self::Active),
                _ => Err(serde::de::Error::custom(
                    "pane must be left, right, active, 0 or 1",
                )),
            },
            Target::Index(0) => Ok(Self::Left),
            Target::Index(1) => Ok(Self::Right),
            Target::Index(_) => Err(serde::de::Error::custom("pane index must be 0 or 1")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionReply {
    pub id: String,
    pub ok: bool,
    pub result: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRow {
    pub id: String,
    pub label: String,
    pub keys: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionsReply {
    pub actions: Vec<ActionRow>,
}

// ── theme ────────────────────────────────────────────────────────────────────

/// `scheme`/`mode` by name; `null` leaves it as it is. This window only,
/// not persisted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeSetReq {
    #[serde(default)]
    pub scheme: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeSetReply {
    pub scheme: String,
    pub mode: String,
}

// ── open ─────────────────────────────────────────────────────────────────────

/// The single-instance forward and the open verb: the first path
/// navigates the left pane, the second the right, extras are logged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenReq {
    #[serde(default)]
    pub paths: Vec<String>,
    /// With a target, exactly one path, and the active pane stays.
    #[serde(default)]
    pub pane: Option<PaneTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenReply {
    pub accepted: usize,
    /// True when at least one path landed in a pane.
    pub opened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuitReply {
    pub quitting: bool,
}

// ── serving ──────────────────────────────────────────────────────────────────

/// What the caller tells the served verbs about itself.
pub struct ServerMeta {
    pub service: String,
    pub headless: bool,
    /// The window can take a focus or sidebar action now (no dialog, not
    /// closing). Headless always refuses them.
    pub window_available: bool,
    pub config_path: Option<String>,
    /// The scheme and mode the window draws; empty headless.
    pub theme_scheme: String,
    pub theme_mode: String,
    pub appearance: AppearanceState,
    /// The `dopus.actions.list` table, built from the effective keymap.
    pub actions: Vec<ActionRow>,
    /// Extra verbs `dopus.describe` lists (the window's drive verbs).
    pub extra_verbs: Vec<String>,
}

/// One served command's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Served {
    /// Toggle a sidebar, then reply to `id`.
    ToggleSidebar {
        id: u64,
        sidebar: dopus_core::config::Sidebar,
        action: String,
    },
    /// Reply `(rc, body)` to command `id`.
    Reply { id: u64, rc: u8, body: String },
    /// Apply the theme selection, then reply with the scheme and mode shown.
    ThemeSet {
        id: u64,
        scheme: Option<String>,
        mode: Option<String>,
    },
    /// A `theme.*` action, then reply as `dopus.action` does.
    ThemeAction {
        id: u64,
        action: ThemeAction,
        name: String,
    },
    /// Focus the pane's location bar, then reply.
    LocationFocus {
        id: u64,
        pane: PaneId,
        action: String,
    },
    /// Quit (the reply went out before this).
    Quit { id: u64 },
}

impl Served {
    pub fn reply_json<T: serde::Serialize>(id: u64, value: &T) -> Self {
        Self::Reply {
            id,
            rc: 0,
            body: serde_json::to_string(value).unwrap_or_else(|_| "{}".into()),
        }
    }

    pub fn refusal(id: u64, refusal: Refusal) -> Self {
        Self::Reply {
            id,
            rc: 10,
            body: serde_json::to_string(&refusal)
                .unwrap_or_else(|_| format!("{{\"error_code\":\"{}\"}}", code::INTERNAL)),
        }
    }

    /// Whether this answer accepts the command: anything but a refusal.
    pub fn accepts(&self) -> bool {
        !matches!(self, Self::Reply { rc, .. } if *rc != 0)
    }

    pub fn error(id: u64, error_code: &str, message: String) -> Self {
        Self::refusal(
            id,
            Refusal {
                error_code: error_code.to_owned(),
                message,
                reason: None,
            },
        )
    }
}

/// The action a `dopus.action` call ran, when `served` accepted it: what
/// the window and headless publish on `dopus.command.executed`.
pub fn accepted_action(command: &Command, served: &[Served]) -> Option<String> {
    if command.verb != "dopus.action" || !served.iter().all(Served::accepts) {
        return None;
    }
    serde_json::from_str::<ActionReq>(&command.body)
        .ok()
        .map(|req| req.id)
}

/// The action table: `(id, label)`. `file.*` rows run from the keyboard and
/// menus only; their `enabled` flags follow the core ([`apply_availability`]).
pub const ACTIONS: &[(&str, &str)] = &[
    ("view.toggle-places", "Show or hide Places"),
    ("view.toggle-properties", "Show or hide Properties"),
    ("location.focus", "Focus the location bar"),
    ("file.open", "Open the selection"),
    ("file.new-folder", "New folder"),
    ("file.rename", "Rename the selection"),
    (
        "file.copy-other-pane",
        "Copy the selection to the other pane",
    ),
    (
        "file.move-other-pane",
        "Move the selection to the other pane",
    ),
    ("file.delete", "Delete the selection"),
    ("nav.back", "Go back"),
    ("nav.forward", "Go forward"),
    ("nav.parent", "Go to parent folder"),
    ("nav.home", "Go to home folder"),
    ("nav.switch-pane", "Switch active pane"),
    ("view.refresh", "Refresh"),
    ("view.toggle-hidden", "Toggle hidden files"),
    ("view.toggle-previews", "Toggle previews"),
    ("view.sort-name", "Sort by name"),
    ("view.sort-size", "Sort by size"),
    ("view.sort-modified", "Sort by modified"),
    ("selection.next", "Select next"),
    ("selection.previous", "Select previous"),
    ("selection.first", "Select first"),
    ("selection.last", "Select last"),
    ("theme.mode-toggle", "Toggle light/dark mode"),
    ("theme.scheme-ocean", "Scheme: Ocean"),
    ("theme.scheme-crimson", "Scheme: Crimson"),
    ("theme.scheme-stone", "Scheme: Stone"),
    ("theme.scheme-forest", "Scheme: Forest"),
    ("theme.scheme-sunset", "Scheme: Sunset"),
    ("theme.scheme-mono", "Scheme: Mono"),
    ("app.quit", "Quit dopus"),
];

/// Action ids that exist in the wider file-manager vocabulary but that
/// dopus does not offer: refused UNAVAILABLE, not INVALID_ARGUMENT.
const KNOWN_ELSEWHERE: &[&str] = &["place.open"];

/// The theme selection a `theme.*` action performs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeAction {
    /// A `theme.scheme-*` action: the scheme by its `dopus.theme.set` name.
    Scheme(String),
    /// `theme.mode-toggle`: light or dark, against what the window shows.
    ModeToggle,
}

fn theme_action(action: &str) -> Option<ThemeAction> {
    if action == "theme.mode-toggle" {
        return Some(ThemeAction::ModeToggle);
    }
    let scheme = action.strip_prefix("theme.scheme-")?;
    ["ocean", "crimson", "stone", "forest", "sunset", "mono"]
        .contains(&scheme)
        .then(|| ThemeAction::Scheme(scheme.to_owned()))
}

/// What applying one action does. A `dopus.action` call is a keystroke a
/// remote caller pressed: the Bus and the keyboard share this.
#[derive(Debug, Clone, PartialEq)]
pub enum Applied {
    ToggleSidebar(dopus_core::config::Sidebar),
    Done,
    /// The window focuses this pane's location bar; headless refuses.
    LocationFocus(PaneId),
    /// A `theme.*` action, for the window; headless refuses.
    Theme(ThemeAction),
    Quit,
}

/// An action the current state cannot take: refused with a reason, never a
/// silent nothing.
fn gated(message: &str) -> Refusal {
    Refusal {
        error_code: code::UNAVAILABLE.to_owned(),
        message: message.to_owned(),
        reason: Some("not_offered".to_owned()),
    }
}

/// Whether the core already holds a New folder or Rename reservation (its
/// `begin_*` calls return silently then): refused up front instead.
fn name_edit_pending(core: &DopusCore) -> bool {
    core.outstanding_reservations().iter().any(|(_, kind)| {
        matches!(
            kind,
            dopus_core::ReservationKind::NewFolder | dopus_core::ReservationKind::Rename
        )
    })
}

/// Apply one action to the active pane.
pub fn apply_action(action: &str, core: &mut DopusCore) -> Result<Applied, Refusal> {
    let pane = core.active();
    apply_action_in(action, core, pane)
}

/// Apply one action to `pane`. Global actions (switch pane, theme, quit)
/// keep their meaning. Each `file.*` arm checks the core's availability
/// first and refuses as the action table shows it disabled.
pub fn apply_action_in(
    action: &str,
    core: &mut DopusCore,
    pane: PaneId,
) -> Result<Applied, Refusal> {
    use dopus_core::SortColumn;
    let done = Ok(Applied::Done);
    let availability = core.availability();
    let busy = || gated("A file operation is still running");
    let selection = |a: &dopus_core::AvailabilitySnapshot| {
        if a.has_selection {
            Ok(())
        } else {
            Err(gated("Nothing is selected"))
        }
    };
    match action {
        "view.toggle-places" => Ok(Applied::ToggleSidebar(dopus_core::config::Sidebar::Places)),
        "view.toggle-properties" => Ok(Applied::ToggleSidebar(
            dopus_core::config::Sidebar::Properties,
        )),
        "location.focus" => Ok(Applied::LocationFocus(pane)),
        "file.open" => {
            // A folder opens in place; a file becomes `OpenFile` (law 4).
            selection(&availability)?;
            core.open_selection();
            done
        }
        "file.new-folder" => {
            if availability.operation_running {
                return Err(busy());
            }
            if name_edit_pending(core) {
                return Err(gated("A name edit is already pending"));
            }
            core.begin_new_folder();
            done
        }
        "file.rename" => {
            selection(&availability)?;
            if availability.selection_count != 1 {
                return Err(gated("Select one item to rename"));
            }
            if availability.operation_running {
                return Err(busy());
            }
            if name_edit_pending(core) {
                return Err(gated("A name edit is already pending"));
            }
            core.begin_rename();
            done
        }
        "file.copy-other-pane" | "file.move-other-pane" | "file.delete" => {
            selection(&availability)?;
            if availability.operation_running {
                return Err(busy());
            }
            match action {
                "file.copy-other-pane" => core.copy_selection_to_other_pane(),
                "file.move-other-pane" => core.move_selection_to_other_pane(),
                _ => core.delete_selection(),
            }
            done
        }
        "nav.back" => {
            core.go_back_in(pane);
            done
        }
        "nav.forward" => {
            core.go_forward_in(pane);
            done
        }
        "nav.parent" => {
            core.go_parent_in(pane);
            done
        }
        "nav.home" => {
            core.go_home_in(pane);
            done
        }
        "nav.switch-pane" => {
            core.switch_pane();
            done
        }
        "view.refresh" => {
            core.refresh_in(pane);
            done
        }
        "view.toggle-hidden" => {
            core.toggle_hidden_in(pane);
            done
        }
        "view.toggle-previews" => {
            core.toggle_previews_in(pane);
            done
        }
        // Law 5: a column switch sorts ascending; the core toggles a same
        // column itself.
        "view.sort-name" => {
            core.set_sort_in(pane, SortColumn::Name, true);
            done
        }
        "view.sort-size" => {
            core.set_sort_in(pane, SortColumn::Size, true);
            done
        }
        "view.sort-modified" => {
            core.set_sort_in(pane, SortColumn::Modified, true);
            done
        }
        "selection.next" => {
            core.select_relative(pane, 1);
            done
        }
        "selection.previous" => {
            core.select_relative(pane, -1);
            done
        }
        "selection.first" => {
            core.select_edge(pane, false);
            done
        }
        "selection.last" => {
            core.select_edge(pane, true);
            done
        }
        "app.quit" => Ok(Applied::Quit),
        other => {
            if let Some(theme) = theme_action(other) {
                return Ok(Applied::Theme(theme));
            }
            let known = KNOWN_ELSEWHERE.contains(&other);
            Err(Refusal {
                error_code: if known {
                    code::UNAVAILABLE
                } else {
                    code::INVALID_ARGUMENT
                }
                .to_owned(),
                message: if known {
                    format!("{other} is not implemented by dopus")
                } else {
                    format!("{other} is not a dopus action")
                },
                reason: known.then(|| "not_implemented".to_owned()),
            })
        }
    }
}

/// Whether `action` is in the table.
pub fn is_action(action: &str) -> bool {
    ACTIONS.iter().any(|(id, _)| *id == action)
}

/// Refresh the table's `enabled` flags from the core: a `file.*` row the
/// current state cannot take reads disabled, as [`apply_action`] refuses it.
pub fn apply_availability(
    actions: &mut [ActionRow],
    availability: &dopus_core::AvailabilitySnapshot,
) {
    for row in actions {
        let selection = availability.has_selection;
        let idle = !availability.operation_running;
        row.enabled = match row.id.as_str() {
            "file.open" => selection,
            "file.new-folder" => idle,
            "file.rename" => selection && idle && availability.selection_count == 1,
            "file.copy-other-pane" | "file.move-other-pane" | "file.delete" => selection && idle,
            _ => true,
        };
    }
}

/// Why `what` was refused to the Bus, whether asked by `dopus.action` or by
/// drive input (`dopus.ui.*`).
pub fn keyboard_only(what: &str) -> String {
    format!("{what} is keyboard-only — the Bus never mutates the filesystem through a file manager")
}

/// `dopus.actions.list`'s table: the actions with the chords `keys` gives
/// each (the effective ones in the window, the defaults headless).
pub fn action_table(keys: impl Fn(&str) -> Vec<String>) -> Vec<ActionRow> {
    ACTIONS
        .iter()
        .map(|(action, label)| ActionRow {
            id: (*action).to_owned(),
            label: (*label).to_owned(),
            keys: keys(action),
            enabled: true,
        })
        .collect()
}

/// How long navigation waits on the stat that tells a file from a folder.
const STAT_WAIT: std::time::Duration = std::time::Duration::from_millis(250);
/// The most of those stats outstanding at once.
const MAX_STATS: usize = 4;

/// A file path navigated as a pane directory lands on its parent:
/// `/etc/passwd` opens `/etc`.
///
/// The stat runs off the calling (UI) thread and is waited on for at most
/// [`STAT_WAIT`]: a path on a stalled mount must not freeze the window.
/// Past that the path is taken as given, and the listing worker reports
/// what it is. A stat that never returns keeps its thread; at most
/// [`MAX_STATS`] are outstanding, and past that the path is taken as given
/// at once, so repeated opens of a stalled mount cannot pile up threads.
pub fn navigable(path: PathBuf) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static STATS: AtomicUsize = AtomicUsize::new(0);
    if STATS.fetch_add(1, Ordering::SeqCst) >= MAX_STATS {
        STATS.fetch_sub(1, Ordering::SeqCst);
        return path;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let probe = path.clone();
    let spawned = std::thread::Builder::new()
        .name("dopus-stat".into())
        .spawn(move || {
            let _ = tx.send(std::fs::metadata(&probe).map(|m| m.is_dir()).ok());
            STATS.fetch_sub(1, Ordering::SeqCst);
        });
    let is_dir = match spawned {
        Ok(_) => rx.recv_timeout(STAT_WAIT).ok().flatten(),
        Err(_) => {
            STATS.fetch_sub(1, Ordering::SeqCst);
            None
        }
    };
    if is_dir == Some(false) {
        return path.parent().map(Path::to_path_buf).unwrap_or(path);
    }
    path
}

/// Apply `dopus.open` paths: the first navigates the left pane, the second
/// the right, extras are logged and ignored. Relative paths resolve against
/// this process's working directory; a file lands on its parent.
pub fn apply_open_paths(core: &mut DopusCore, paths: &[String]) {
    for (index, raw) in paths.iter().enumerate() {
        let pane = match index {
            0 => PaneId::Left,
            1 => PaneId::Right,
            _ => {
                tracing::info!("dopus.open: ignoring extra path {} ({raw})", index + 1);
                continue;
            }
        };
        core.navigate(pane, open_path(raw));
    }
}

fn open_path(raw: &str) -> PathBuf {
    let expanded = crate::dirs::expand_tilde(raw);
    let absolute = if expanded.is_relative() {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(expanded),
            Err(_) => expanded,
        }
    } else {
        expanded
    };
    navigable(absolute)
}

/// One pane's `dopus.state` row.
pub fn pane_state(core: &DopusCore, pane_id: PaneId) -> PaneState {
    let pane = core.pane(pane_id);
    PaneState {
        pane: pane_id.index() as u8,
        path: dopus_core::sanitise_display_path(&pane.path),
        active: core.active() == pane_id,
        show_hidden: pane.show_hidden,
        previews: pane.previews,
        preview: match core.preview(pane_id) {
            Some(dopus_core::thumbs::Preview::Ready(path)) => {
                Some(dopus_core::sanitise_display_path(&path))
            }
            _ => None,
        },
        sort: match pane.sort {
            dopus_core::SortColumn::Name => "name",
            dopus_core::SortColumn::Size => "size",
            dopus_core::SortColumn::Modified => "modified",
        }
        .to_owned(),
        ascending: pane.ascending,
        selected: pane
            .selected
            .as_ref()
            .map(|p| dopus_core::sanitise_display_path(p)),
        selected_paths: core
            .selected_paths(pane_id)
            .iter()
            .map(|path| dopus_core::sanitise_display_path(path))
            .collect(),
        rows: core.visible_rows(pane_id).len(),
        status: pane.status.clone(),
        summary: pane.footer_summary(),
    }
}

/// One `dopus.*` command from the Bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: u64,
    pub verb: String,
    pub body: String,
}

fn window_refusal(id: u64, meta: &ServerMeta, message: &str) -> Served {
    Served::refusal(
        id,
        Refusal {
            error_code: code::UNAVAILABLE.to_owned(),
            message: message.to_owned(),
            reason: Some(
                if meta.headless {
                    "headless"
                } else {
                    "window_busy"
                }
                .to_owned(),
            ),
        },
    )
}

/// Serve one Bus command. Every arm ends in an answer.
pub fn serve_command(
    command: &Command,
    core: &mut DopusCore,
    meta: &ServerMeta,
    info: &buildinfo::BuildInfo,
) -> Vec<Served> {
    let id = command.id;
    let window = !meta.headless && meta.window_available;
    match command.verb.as_str() {
        "dopus.ping" => vec![Served::reply_json(
            id,
            &PingReply {
                pong: true,
                service: meta.service.clone(),
                schema: SCHEMA.to_owned(),
                pid: std::process::id(),
                headless: meta.headless,
            },
        )],
        "dopus.describe" => vec![Served::reply_json(
            id,
            &DescribeReply {
                contract: "ctk-app-control.v0".to_owned(),
                app: "dopus".to_owned(),
                title: "MixOS DOpus".to_owned(),
                view: "dopus".to_owned(),
                engine: "egui".to_owned(),
                version: info.version.to_owned(),
                description: "the MixOS twin-pane file manager: Places and Properties panels; \
                              file operations from the keyboard, menus and dialogs; file.* is \
                              not served over the Bus"
                    .to_owned(),
                controls: Vec::new(),
                verbs: VERBS
                    .iter()
                    .map(|(verb, _)| (*verb).to_owned())
                    .chain(meta.extra_verbs.iter().cloned())
                    .collect(),
            },
        )],
        "dopus.info" => vec![Served::reply_json(
            id,
            &InfoReply {
                version: info.version.to_owned(),
                git_sha: info.git_sha.to_owned(),
                build_time: info.build_time.to_owned(),
                headless: meta.headless,
                panes: 2,
                pane_states: vec![
                    pane_state(core, PaneId::Left),
                    pane_state(core, PaneId::Right),
                ],
                config_path: meta.config_path.clone(),
            },
        )],
        "dopus.state" => vec![Served::reply_json(
            id,
            &StateReply {
                places: core.sidebar(dopus_core::config::Sidebar::Places),
                properties: core.sidebar(dopus_core::config::Sidebar::Properties),
                panes: vec![
                    pane_state(core, PaneId::Left),
                    pane_state(core, PaneId::Right),
                ],
                theme_scheme: meta.theme_scheme.clone(),
                theme_mode: meta.theme_mode.clone(),
                appearance: meta.appearance.clone(),
            },
        )],
        "dopus.action" => {
            let req = match serde_json::from_str::<ActionReq>(&command.body) {
                Ok(req) => req,
                Err(error) => {
                    return vec![Served::error(
                        id,
                        code::INVALID_ARGUMENT,
                        format!("body: {error}"),
                    )];
                }
            };
            if req.id.starts_with("file.") {
                return vec![Served::error(id, code::FORBIDDEN, keyboard_only(&req.id))];
            }
            let pane = req.pane.resolve(core);
            match apply_action_in(&req.id, core, pane) {
                Ok(Applied::ToggleSidebar(_)) if !window => vec![window_refusal(
                    id,
                    meta,
                    "sidebar toggles need an available window",
                )],
                Ok(Applied::ToggleSidebar(sidebar)) => vec![Served::ToggleSidebar {
                    id,
                    sidebar,
                    action: req.id,
                }],
                Ok(Applied::LocationFocus(_)) if !window => vec![window_refusal(
                    id,
                    meta,
                    "location focus needs an available window editor",
                )],
                Ok(Applied::LocationFocus(pane)) => vec![Served::LocationFocus {
                    id,
                    pane,
                    action: req.id,
                }],
                Ok(Applied::Done) => vec![Served::reply_json(
                    id,
                    &ActionReply {
                        id: req.id,
                        ok: true,
                        result: None,
                    },
                )],
                Ok(Applied::Theme(_)) if meta.headless => vec![Served::refusal(
                    id,
                    Refusal {
                        error_code: code::UNAVAILABLE.to_owned(),
                        message: "theme selection needs the windowed app (headless paints nothing)"
                            .to_owned(),
                        reason: Some("headless".to_owned()),
                    },
                )],
                Ok(Applied::Theme(theme)) => vec![Served::ThemeAction {
                    id,
                    action: theme,
                    name: req.id,
                }],
                Ok(Applied::Quit) => vec![
                    Served::reply_json(id, &QuitReply { quitting: true }),
                    Served::Quit { id },
                ],
                Err(refusal) => vec![Served::refusal(id, refusal)],
            }
        }
        "dopus.actions.list" => {
            let mut actions = meta.actions.clone();
            apply_availability(&mut actions, &core.availability());
            for row in &mut actions {
                if [
                    "location.focus",
                    "view.toggle-places",
                    "view.toggle-properties",
                ]
                .contains(&row.id.as_str())
                {
                    row.enabled = window;
                }
            }
            vec![Served::reply_json(id, &ActionsReply { actions })]
        }
        "dopus.theme.set" => {
            if meta.headless {
                return vec![Served::refusal(
                    id,
                    Refusal {
                        error_code: code::UNAVAILABLE.to_owned(),
                        message: "theme selection needs the windowed app (headless paints nothing)"
                            .to_owned(),
                        reason: Some("headless".to_owned()),
                    },
                )];
            }
            match serde_json::from_str::<ThemeSetReq>(&command.body) {
                Ok(req) => vec![Served::ThemeSet {
                    id,
                    scheme: req.scheme,
                    mode: req.mode,
                }],
                Err(error) => vec![Served::error(
                    id,
                    code::INVALID_ARGUMENT,
                    format!("body: {error}"),
                )],
            }
        }
        "dopus.open" => match serde_json::from_str::<OpenReq>(&command.body) {
            Ok(req) => {
                if let Some(target) = req.pane {
                    if req.paths.len() != 1 {
                        return vec![Served::error(
                            id,
                            code::INVALID_ARGUMENT,
                            "pane-targeted open requires exactly one path".to_owned(),
                        )];
                    }
                    let pane = target.resolve(core);
                    core.navigate(pane, open_path(&req.paths[0]));
                } else {
                    apply_open_paths(core, &req.paths);
                }
                vec![Served::reply_json(
                    id,
                    &OpenReply {
                        accepted: req.paths.len(),
                        opened: !req.paths.is_empty(),
                    },
                )]
            }
            Err(error) => vec![Served::error(
                id,
                code::INVALID_ARGUMENT,
                format!("body: {error}"),
            )],
        },
        "dopus.quit" => vec![
            Served::reply_json(id, &QuitReply { quitting: true }),
            Served::Quit { id },
        ],
        other => vec![Served::error(
            id,
            code::UNKNOWN_VERB,
            format!("{other} is not a dopus verb (schema {SCHEMA})"),
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dopus_core::{CoreEvent, DOpusConfig, FileEntry};

    fn meta(headless: bool) -> ServerMeta {
        ServerMeta {
            service: SERVICE.into(),
            headless,
            window_available: !headless,
            config_path: None,
            theme_scheme: String::new(),
            theme_mode: String::new(),
            appearance: AppearanceState::default(),
            actions: action_table(crate::keymap::default_keys),
            extra_verbs: Vec::new(),
        }
    }

    fn core() -> (tempfile::TempDir, DopusCore) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = DOpusConfig::default();
        config.left.path = dir.path().to_owned();
        config.right.path = dir.path().to_owned();
        let (core, _rx) = DopusCore::new(config, None);
        (dir, core)
    }

    fn serve(core: &mut DopusCore, headless: bool, verb: &str, body: &str) -> Vec<Served> {
        let command = Command {
            id: 1,
            verb: verb.into(),
            body: body.into(),
        };
        serve_command(&command, core, &meta(headless), &buildinfo::build_info!())
    }

    fn refusal(served: &[Served]) -> Option<String> {
        served.iter().find_map(|s| match s {
            Served::Reply { rc: 10, body, .. } => serde_json::from_str::<Refusal>(body)
                .ok()
                .map(|r| r.error_code),
            _ => None,
        })
    }

    #[test]
    fn every_action_is_served_or_refused_by_rule() {
        for (action, _) in ACTIONS {
            let (_dir, mut core) = core();
            let served = serve(
                &mut core,
                true,
                "dopus.action",
                &serde_json::json!({ "id": action }).to_string(),
            );
            let code = refusal(&served);
            if action.starts_with("file.") {
                assert_eq!(code.as_deref(), Some(code::FORBIDDEN), "{action}");
            } else if action.starts_with("theme.")
                || [
                    "location.focus",
                    "view.toggle-places",
                    "view.toggle-properties",
                ]
                .contains(action)
            {
                assert_eq!(
                    code.as_deref(),
                    Some(code::UNAVAILABLE),
                    "{action} headless"
                );
            } else {
                assert_eq!(code, None, "{action}: {served:?}");
            }
        }
    }

    #[test]
    fn unknown_ids_and_verbs_are_refused() {
        let (_dir, mut core) = core();
        let bad = serve(&mut core, false, "dopus.action", r#"{"id":"nope.nothing"}"#);
        assert_eq!(refusal(&bad).as_deref(), Some(code::INVALID_ARGUMENT));
        let elsewhere = serve(&mut core, false, "dopus.action", r#"{"id":"place.open"}"#);
        assert_eq!(refusal(&elsewhere).as_deref(), Some(code::UNAVAILABLE));
        let verb = serve(&mut core, false, "dopus.nope", "{}");
        assert_eq!(refusal(&verb).as_deref(), Some(code::UNKNOWN_VERB));
        let pane = serve(
            &mut core,
            false,
            "dopus.action",
            r#"{"id":"nav.home","pane":3}"#,
        );
        assert_eq!(refusal(&pane).as_deref(), Some(code::INVALID_ARGUMENT));
    }

    /// What dopus.command.executed carries for a Bus call: the action of an
    /// accepted dopus.action, never a refused one or another verb.
    #[test]
    fn only_accepted_actions_are_executed_runs() {
        let (_dir, mut core) = core();
        let ran = |core: &mut DopusCore, headless: bool, verb: &str, body: &str| {
            let command = Command {
                id: 1,
                verb: verb.into(),
                body: body.into(),
            };
            let served = serve_command(&command, core, &meta(headless), &buildinfo::build_info!());
            accepted_action(&command, &served)
        };
        let hidden = r#"{"id":"view.toggle-hidden"}"#;
        assert_eq!(
            ran(&mut core, true, "dopus.action", hidden).as_deref(),
            Some("view.toggle-hidden")
        );
        // A file action, a sidebar headless (no window), another verb.
        for (verb, body) in [
            ("dopus.action", r#"{"id":"file.delete"}"#),
            ("dopus.action", r#"{"id":"view.toggle-places"}"#),
            ("dopus.state", "{}"),
        ] {
            assert_eq!(ran(&mut core, true, verb, body), None, "{verb} {body}");
        }
    }

    #[test]
    fn the_window_gets_effects_headless_gets_refusals() {
        let (_dir, mut core) = core();
        let served = serve(
            &mut core,
            false,
            "dopus.action",
            r#"{"id":"theme.scheme-forest"}"#,
        );
        assert!(matches!(
            &served[..],
            [Served::ThemeAction { action: ThemeAction::Scheme(s), .. }] if s == "forest"
        ));
        let served = serve(&mut core, false, "dopus.theme.set", r#"{"mode":"light"}"#);
        assert!(matches!(&served[..], [Served::ThemeSet { .. }]));
        let headless = serve(&mut core, true, "dopus.theme.set", r#"{"mode":"light"}"#);
        assert_eq!(refusal(&headless).as_deref(), Some(code::UNAVAILABLE));
        let quit = serve(&mut core, true, "dopus.quit", "{}");
        assert!(matches!(
            &quit[..],
            [Served::Reply { rc: 0, .. }, Served::Quit { .. }]
        ));
    }

    #[test]
    fn open_targets_panes() {
        let (dir, mut core) = core();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let body = serde_json::json!({ "paths": [sub], "pane": "right" }).to_string();
        let served = serve(&mut core, true, "dopus.open", &body);
        assert!(matches!(&served[..], [Served::Reply { rc: 0, .. }]));
        assert_eq!(core.pane(PaneId::Right).path, sub);
        let two = serde_json::json!({ "paths": ["/a", "/b"], "pane": "left" }).to_string();
        let served = serve(&mut core, true, "dopus.open", &two);
        assert_eq!(refusal(&served).as_deref(), Some(code::INVALID_ARGUMENT));
        let file = dir.path().join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(navigable(file), dir.path());
    }

    #[test]
    fn pane_summaries_follow_their_root_listing() {
        let (dir, mut core) = core();
        core.on_event(CoreEvent::ListingArrived {
            pane: PaneId::Left,
            generation: core.pane(PaneId::Left).generation,
            path: dir.path().to_owned(),
            root: true,
            result: Ok(vec![
                FileEntry {
                    path: dir.path().join("folder"),
                    name: "folder".into(),
                    is_dir: true,
                    size: None,
                    child_count: Some(99),
                    modified: None,
                },
                FileEntry {
                    path: dir.path().join("file"),
                    name: "file".into(),
                    is_dir: false,
                    size: Some(1536),
                    child_count: None,
                    modified: None,
                },
            ]),
        });
        let state = pane_state(&core, PaneId::Left);
        assert_eq!(state.summary, "1 folder, 1 file (1.5 KiB)");
        assert_eq!(state.rows, 2);
    }

    #[test]
    fn the_table_carries_chords_and_availability() {
        let table = action_table(crate::keymap::default_keys);
        let parent = table.iter().find(|r| r.id == "nav.parent").unwrap();
        assert_eq!(parent.keys, ["Backspace", "Alt+ArrowUp"]);
        assert!(ACTIONS.iter().all(|(id, _)| is_action(id)));
        let (_dir, mut core) = core();
        let served = serve(&mut core, true, "dopus.actions.list", "{}");
        let Served::Reply { body, .. } = &served[0] else {
            panic!()
        };
        let reply: ActionsReply = serde_json::from_str(body).unwrap();
        let open = reply.actions.iter().find(|r| r.id == "file.open").unwrap();
        assert!(!open.enabled, "nothing selected");
        let focus = reply
            .actions
            .iter()
            .find(|r| r.id == "location.focus")
            .unwrap();
        assert!(!focus.enabled, "headless");
    }
}
