// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The command registry: every action an application offers is a
//! [`Action`] with a stable id, and the menus, shortcuts, palette and Bus
//! surface are generated from the registry. The UI, the `mix` command line
//! and Bus commands all dispatch by id through [`Registry::execute`], so anything
//! a person can do in an application an agent can do too.
//!
//! A command's handler takes the application's state (`S`) and changes it;
//! work that leaves the process (a Bus call, a file) is queued on the state
//! for the application's effect loop, never performed inside the handler.

use crate::executed::Executed;
pub use crate::executed::Origin;
use crate::icons::Icon;
use crate::menu::{self, Entry, Menu, Row};
use crate::strings::Strings;
pub use ::action::{Action as EngineAction, Registry as EngineRegistry};
pub use ::action::{Call, Class, Described, Error, ParamSpec, Place, Scope};
use egui::KeyboardShortcut;
use std::collections::HashMap;
use std::fmt;

/// The message `key` from the app's catalogue, else from the toolkit's own
/// (the labels of toolkit-made commands, such as the Theme menu's), else
/// the key itself.
pub fn label_text(strings: &Strings, key: &str) -> String {
    if strings.has(key) {
        strings.get(key)
    } else {
        crate::strings::own(key)
    }
}

struct ToolkitLabels<'a>(&'a Strings);
impl ::action::Labels for ToolkitLabels<'_> {
    fn label(&self, key: &str) -> String {
        label_text(self.0, key)
    }
}

/// One legacy nullary action. New engine tables use `command::Command`.
pub struct Action<S> {
    /// Stable dotted id, e.g. `bus.refresh`. Never reused once published.
    pub id: &'static str,
    /// Fluent key of the label shown in menus and the palette.
    pub label: &'static str,
    /// Fluent key of the top-level menu it appears in, or `None` for
    /// palette- and shortcut-only commands.
    pub menu: Option<&'static str>,
    /// Fluent key of a submenu inside `menu`, or `None` for a row of the
    /// menu itself. The submenu row sits where its first command would.
    pub submenu: Option<&'static str>,
    /// The command's group within its menu (or submenu): a separator is
    /// drawn wherever the group changes from one row to the next. Use 0
    /// when the menu has no groups.
    pub group: u8,
    pub shortcut: Option<KeyboardShortcut>,
    pub icon: Option<Icon>,
    /// Whether the command can run in this state.
    pub enabled: fn(&S) -> bool,
    pub run: fn(&mut S),
}

/// A command can always run.
pub fn always<S>(_: &S) -> bool {
    true
}

/// Why [`Registry::execute`] did not run a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    Unknown(String),
    Denied(&'static str),
    Disabled(&'static str),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(id) => write!(f, "unknown command {id:?}"),
            Self::Denied(id) => write!(f, "command {id:?} is denied"),
            Self::Disabled(id) => write!(f, "command {id:?} is disabled"),
        }
    }
}

impl std::error::Error for CommandError {}

/// The application's commands, in menu order.
pub struct Registry<S> {
    commands: Vec<Action<S>>,
    /// Fluent key of the menu that opens with a search field (§3.5).
    search_menu: Option<&'static str>,
    /// The UI-free registry owns dispatch, choices, labels and binding policy.
    core: ::action::Registry<S>,
    /// Parsed accelerator cache, refreshed only when bindings change.
    extras: HashMap<&'static str, Vec<KeyboardShortcut>>,
    /// Original parsed chords retain explicit Ctrl versus platform-command
    /// flags. Their wire names alone cannot express that distinction.
    parsed_defaults: HashMap<&'static str, Vec<KeyboardShortcut>>,
}

impl<S> Default for Registry<S> {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            search_menu: None,
            core: ::action::Registry::new(),
            extras: HashMap::new(),
            parsed_defaults: HashMap::new(),
        }
    }
}

/// Refuse `shortcut` as a default chord of command `id` when it is on one of
/// the person's function keys, F1–F12, whatever its modifiers
/// (docs/docs/decisions/2026-10-17-function-keys-belong-to-the-user.md).
/// Only [`Registry::rebind`], applying the person's own bindings, may put a
/// command on one.
fn default_chord(id: &str, shortcut: KeyboardShortcut) {
    assert!(
        !crate::keymap::user_only(shortcut.logical_key),
        "command {id:?} has a default chord on {}: F1-F12 belong to the user and only \
         their keymap may bind them (docs/docs/decisions/2026-10-17-function-keys-belong-to-the-user.md)",
        crate::keymap::chord_text(&shortcut)
    );
}

/// A choice command's own run does nothing: the registry runs its closure.
fn choose<S>(_: &mut S) {}

impl<S> Registry<S> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Finish a transitional table, retaining its handlers and bindings in
    /// the core registry so an engine table can be added before adaptation.
    pub fn into_registry(self) -> EngineRegistry<S> {
        self.core
    }

    /// Add an engine-owned command while assembling a transitional table.
    /// Only nullary commands get UI rows; all handlers stay in core.
    pub fn add_engine(&mut self, command: EngineAction<S>) -> &mut Self {
        if command.params.is_none() {
            self.commands.push(Action {
                id: command.id,
                label: command.label,
                menu: command.menu,
                submenu: command.submenu,
                group: command.group,
                shortcut: None,
                icon: command
                    .icon
                    .and_then(|name| Icon::ALL.iter().copied().find(|i| i.name() == name)),
                enabled: always,
                run: choose,
            });
        }
        self.core.add(command);
        self.refresh_shortcuts();
        self
    }

    /// Add `command`.
    ///
    /// # Panics
    /// On a duplicate id or shortcut: both are programming errors that would
    /// otherwise make dispatch ambiguous. On a shortcut on F1–F12, with any
    /// modifiers: those keys are the person's ([`default_chord`]).
    pub fn add(&mut self, command: Action<S>) -> &mut Self {
        assert!(
            self.get(command.id).is_none(),
            "duplicate command id {:?}",
            command.id
        );
        if let Some(shortcut) = command.shortcut {
            default_chord(command.id, shortcut);
            assert!(
                !self.taken(shortcut),
                "duplicate shortcut on {:?}",
                command.id
            );
        }
        #[allow(deprecated)] // Transitional nullary wrapper, removed in plan step 7.
        self.core.add_nullary(
            ::action::Action {
                id: command.id,
                label: command.label,
                menu: command.menu,
                submenu: command.submenu,
                group: command.group,
                shortcut: None,
                icon: command.icon.map(Icon::name),
                enabled: ::action::always,
                run: |_, _| Ok(serde_json::Value::Null),
                params: None,
                class: Class::Intent,
            },
            command.enabled,
            command.run,
        );
        self.core.set_shortcut(
            command.id,
            command.shortcut.map(|s| crate::keymap::chord_text(&s)),
        );
        self.parsed_defaults
            .insert(command.id, command.shortcut.into_iter().collect());
        self.commands.push(command);
        self
    }

    /// Add a choice command: one of a group, ticked in its menu while
    /// `checked` holds, running `run` (a closure, so it can carry the
    /// choice). Menus, Help search, [`Registry::describe`] (with its tick)
    /// and [`Registry::execute`] treat it as any other command.
    ///
    /// # Panics
    /// On a duplicate id, as [`Registry::add`].
    pub fn add_choice(
        &mut self,
        place: Place,
        run: impl Fn(&mut S) + 'static,
        checked: impl Fn(&S) -> bool + 'static,
    ) -> &mut Self {
        let Place {
            id,
            label,
            menu,
            submenu,
            group,
        } = place;
        self.add(Action {
            id,
            label,
            menu,
            submenu,
            group,
            shortcut: None,
            icon: None,
            enabled: always,
            run: choose,
        });
        // Replace the temporary nullary entry with its choice closure in core.
        self.core.set_choice(id, run, checked);
        self
    }

    /// Add `command`, ticked in its menu while `checked` holds: an on/off
    /// setting that keeps its own shortcut and enablement (a choice command
    /// has neither).
    ///
    /// # Panics
    /// On a duplicate id or shortcut, as [`Registry::add`].
    pub fn add_toggle(
        &mut self,
        command: Action<S>,
        checked: impl Fn(&S) -> bool + 'static,
    ) -> &mut Self {
        let id = command.id;
        self.add(command);
        self.core.check(id, checked);
        self
    }

    /// Let command `id`'s shortcuts fire only in `scope`.
    ///
    /// # Panics
    /// When there is no command `id`.
    pub fn scope(&mut self, id: &str, scope: Scope) -> &mut Self {
        self.core.scope(id, scope);
        self
    }

    /// Where command `id`'s shortcuts fire.
    pub fn scope_of(&self, id: &str) -> Scope {
        self.core.scope_of(id)
    }

    /// Give command `id` a label that follows the state ("Mark as Read"
    /// while unread, "Mark as Unread" otherwise): `label` returns the
    /// Fluent key. Menus, [`Registry::describe`] and search read it.
    ///
    /// # Panics
    /// When there is no command `id`.
    pub fn relabel(&mut self, id: &str, label: impl Fn(&S) -> &'static str + 'static) -> &mut Self {
        self.core.relabel(id, label);
        self
    }

    /// Action `id`'s label key in `state`.
    pub fn label_of(&self, id: &str, state: &S) -> Option<&'static str> {
        self.core.label_of(id, state)
    }

    /// Whether command `id` is a choice, and if so whether it is ticked.
    pub fn checked(&self, id: &str, state: &S) -> Option<bool> {
        self.core.checked(id, state)
    }

    pub(crate) fn enabled(&self, id: &str, state: &S) -> bool {
        self.core.enabled(id, state).is_ok()
    }

    /// Open the menu `menu` (a Fluent menu key, usually Help) with a field
    /// that searches every command (§3.5).
    pub fn search_menu(&mut self, menu: &'static str) -> &mut Self {
        self.search_menu = Some(menu);
        self
    }

    pub fn get(&self, id: &str) -> Option<&Action<S>> {
        self.commands.iter().find(|c| c.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Action<S>> {
        self.commands.iter()
    }

    /// Whether any command already has `shortcut`, as its own or a further
    /// chord.
    fn taken(&self, shortcut: KeyboardShortcut) -> bool {
        self.commands.iter().any(|c| c.shortcut == Some(shortcut))
            || self.extras.values().flatten().any(|s| *s == shortcut)
    }

    /// Give command `id` further chords that run it too, outside the menus.
    /// A chord another command already has is skipped, so the first command
    /// to claim a chord keeps it.
    ///
    /// # Panics
    /// When there is no command `id`, or on a chord on F1–F12, as
    /// [`Registry::add`].
    pub fn add_shortcuts(
        &mut self,
        id: &str,
        shortcuts: impl IntoIterator<Item = KeyboardShortcut>,
    ) -> &mut Self {
        let id = self
            .get(id)
            .unwrap_or_else(|| panic!("no command {id:?}"))
            .id;
        let shortcuts: Vec<_> = shortcuts
            .into_iter()
            .inspect(|s| default_chord(id, *s))
            .filter(|s| !self.taken(*s))
            .collect();
        self.parsed_defaults
            .entry(id)
            .or_default()
            .extend(&shortcuts);
        self.core
            .add_shortcuts(id, shortcuts.iter().map(crate::keymap::chord_text));
        self.refresh_shortcuts();
        self
    }

    /// Every chord of command `id`: its own shortcut first, then the rest.
    pub fn shortcuts_of(&self, id: &str) -> Vec<KeyboardShortcut> {
        let Some(command) = self.get(id) else {
            return Vec::new();
        };
        command
            .shortcut
            .into_iter()
            .chain(self.extras.get(command.id).into_iter().flatten().copied())
            .collect()
    }

    /// Every command with a chord, as a grid of its label and its chords,
    /// for a Keyboard Shortcuts dialog: it shows the bindings in force.
    pub fn shortcut_grid(&self, ui: &mut egui::Ui, strings: &Strings) {
        let ctx = ui.ctx().clone();
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                egui::Grid::new("shortcut-grid").show(ui, |ui| {
                    for command in &self.commands {
                        let all = self.shortcuts_of(command.id);
                        if all.is_empty() {
                            continue;
                        }
                        let keys: Vec<String> =
                            all.iter().map(|s| ctx.format_shortcut(s)).collect();
                        ui.label(label_text(strings, command.label));
                        ui.label(egui::RichText::new(keys.join(", ")).weak());
                        ui.end_row();
                    }
                });
            });
    }

    /// Apply one app's bindings from the desktop profile, starting from the
    /// chords the registry was built with (so applying an empty map restores
    /// them):
    ///
    /// - each listed command takes exactly its listed chords, the first as
    ///   its menu accelerator; an empty list unbinds it;
    /// - a chord the person bound is taken from any command that had it by
    ///   default; two listed commands claiming one chord leave it with the
    ///   first, by id;
    /// - an unknown command or a chord that is not a key is skipped; a
    ///   list in which no chord is a key keeps the command's defaults.
    ///
    /// Returns a note for each thing skipped, for the app's log.
    pub fn rebind(&mut self, overrides: &crate::keymap::Overrides) -> Vec<String> {
        let notes = self.core.rebind(overrides, |text| {
            crate::keymap::parse_chord(text).map(|s| crate::keymap::chord_text(&s))
        });
        self.refresh_shortcuts();
        notes
    }

    fn refresh_shortcuts(&mut self) {
        self.extras.clear();
        let mut seen = Vec::new();
        for c in &mut self.commands {
            let defaults = self.parsed_defaults.get(c.id);
            let overridden = self.core.shortcuts_overridden(c.id);
            let (own, more) = self.core.bound_of(c.id);
            let has_own = own.is_some();
            let chords: Vec<_> = own
                .into_iter()
                .chain(more)
                .map(|s| {
                    defaults
                        .filter(|_| !overridden)
                        .and_then(|keys| {
                            keys.iter().find(|key| crate::keymap::chord_text(key) == s)
                        })
                        .copied()
                        .or_else(|| crate::keymap::parse_chord(&s))
                        .unwrap_or_else(|| panic!("invalid shortcut {s:?} on {:?}", c.id))
                })
                .collect();
            for chord in &chords {
                assert!(!seen.contains(chord), "duplicate shortcut on {:?}", c.id);
                seen.push(*chord);
            }
            let mut chords = chords.into_iter();
            c.shortcut = if has_own { chords.next() } else { None };
            let more: Vec<_> = chords.collect();
            if !more.is_empty() {
                self.extras.insert(c.id, more);
            }
        }
    }
    /// Run the command `id` against `state`: the one dispatch path for the
    /// UI, the command line and the Bus. `origin` says how it was reached;
    /// an accepted run is recorded for `<app>.action.executed` once the
    /// handler returns, and a refused one is not.
    pub fn execute(&self, id: &str, state: &mut S, origin: Origin) -> Result<(), CommandError> {
        self.core
            .execute(id, state, serde_json::Value::Null, origin)
            .map(|_| ())
            .map_err(|err| match err {
                Error::Unknown(id) => CommandError::Unknown(id),
                Error::Denied { id, .. } => CommandError::Denied(id),
                Error::Disabled { id, .. } => CommandError::Disabled(id),
                Error::BadParams { .. } | Error::Failed(_) => CommandError::Disabled(
                    "legacy nullary dispatch: invalid parameters or handler failure",
                ),
            })
    }

    /// Record a run of `id` that the application performed itself instead
    /// of through [`Registry::execute`] (a toggle that needs the frame's
    /// state, an action the editor or the Bus ran), so it is published like
    /// any other. Record only an accepted run, once.
    pub fn note(&self, id: &str, origin: Origin) {
        self.core.note(id, origin);
    }

    /// The runs recorded since the last call, oldest first, and how many
    /// were dropped before them, for the application's Bus connection to
    /// publish ([`crate::executed`]).
    pub fn take_executed(&self) -> (Vec<Executed>, u64) {
        self.core.take_executed()
    }

    /// Every command with its current enablement, for `<app>.actions`.
    pub fn describe(&self, state: &S, strings: &Strings) -> Vec<Described> {
        self.core.describe(state, &ToolkitLabels(strings))
    }

    /// Ids of the enabled commands whose shortcut was pressed this frame, in
    /// press order, consuming those key presses. None fire while a menu is
    /// open: the open menu has the keyboard (chrome specification §3.6).
    ///
    /// Modifiers match exactly, unlike egui's `consume_shortcut`, which
    /// ignores extra Shift and Alt: Ctrl+Shift+S never runs the Ctrl+S
    /// command, even when the Ctrl+Shift+S command is disabled.
    pub fn shortcuts(&self, ctx: &egui::Context, state: &S) -> Vec<&'static str> {
        crate::macros::keyboard(ctx);
        let mut fired = Vec::new();
        if menu::is_open(ctx) {
            return fired;
        }
        // With no text field taking the keyboard, the clipboard events the
        // platform makes of Ctrl+X, Ctrl+C and Ctrl+V are those key presses.
        let typing = ctx.egui_wants_keyboard_input();
        let clipboard = !typing;
        ctx.input_mut(|i| {
            let held = i.modifiers;
            i.events
                .retain(|event| match self.hit(event, held, clipboard) {
                    // A browsing chord is the text field's while one types.
                    Some(command) if typing && self.scope_of(command.id) == Scope::Browsing => true,
                    Some(command) if self.core.enabled(command.id, state).is_ok() => {
                        fired.push(command.id);
                        false
                    }
                    _ => true,
                });
        });
        fired
    }

    /// Whether [`Self::shortcuts`] takes `event` (with no menu open): a
    /// press of an enabled command's shortcut. An app that orders its own
    /// input around the shortcuts asks this.
    pub fn takes(&self, event: &egui::Event, state: &S) -> bool {
        self.hit(event, egui::Modifiers::NONE, false)
            .is_some_and(|c| self.core.enabled(c.id, state).is_ok())
    }

    /// The command whose shortcut `event` presses, modifiers exactly.
    ///
    /// egui-winit sends no key press for Ctrl+X, Ctrl+C or Ctrl+V (with any
    /// other modifiers): it sends `Cut`, `Copy` or `Paste` instead. With
    /// `clipboard`, those count as the key with the modifiers `held`.
    fn hit(
        &self,
        event: &egui::Event,
        held: egui::Modifiers,
        clipboard: bool,
    ) -> Option<&Action<S>> {
        let (key, modifiers) = match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => (*key, *modifiers),
            egui::Event::Cut if clipboard => (egui::Key::X, held),
            egui::Event::Copy if clipboard => (egui::Key::C, held),
            egui::Event::Paste(_) | egui::Event::PasteImage(_) if clipboard => (egui::Key::V, held),
            _ => return None,
        };
        let pressed =
            |s: &KeyboardShortcut| s.logical_key == key && modifiers.matches_exact(s.modifiers);
        self.commands.iter().find(|c| {
            c.shortcut.as_ref().is_some_and(pressed)
                || self
                    .extras
                    .get(c.id)
                    .is_some_and(|more| more.iter().any(pressed))
        })
    }

    /// A menu bar of its own, for an app without the title bar
    /// ([`crate::titlebar`]). Returns the commands chosen, each with how
    /// (a menu row, or Help search).
    pub fn menu_bar(
        &self,
        ui: &mut egui::Ui,
        state: &S,
        strings: &Strings,
    ) -> Vec<(&'static str, Origin)> {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            self.menus(ui, state, strings)
        })
        .inner
    }

    /// The menus ([`menu::bar`]) drawn into the current left-to-right row.
    /// Returns the commands chosen this frame: from a menu row
    /// ([`Origin::Menu`]) or from Help search's results ([`Origin::Palette`]).
    pub fn menus(
        &self,
        ui: &mut egui::Ui,
        state: &S,
        strings: &Strings,
    ) -> Vec<(&'static str, Origin)> {
        let model = self.model(ui.ctx(), state, strings);
        let search = self.search_menu.and_then(|key| {
            let title = label_text(strings, key);
            let menu = model.iter().position(|m| m.title == title)?;
            Some(menu::Search {
                menu,
                hint: crate::strings::own("search-menus"),
                empty: crate::strings::own("no-matching-commands"),
            })
        });
        menu::bar_picked(ui, &model, search.as_ref())
            .map(|(id, searched)| {
                let origin = if searched {
                    Origin::Palette
                } else {
                    Origin::Menu
                };
                (id, origin)
            })
            .into_iter()
            .collect()
    }

    /// Menu `menu` (a Fluent menu key) as `response`'s right-click menu:
    /// the menu bar's rows, ticks, shortcuts and enablement, generated the
    /// same way. Returns the command chosen this frame (run it with
    /// [`Origin::Menu`]).
    pub fn context_menu(
        &self,
        response: &egui::Response,
        menu: &str,
        state: &S,
        strings: &Strings,
    ) -> Option<&'static str> {
        self.context_popup(egui::Popup::context_menu(response), menu, state, strings)
    }

    /// Menu `menu` as the right-click menu of the area `rect` of `ui`: a
    /// secondary click anywhere in it opens the menu, whichever widget
    /// (a selectable label, a link) the click landed on. For a pane full of
    /// text that has no single response. `id` names the menu's memory.
    pub fn context_menu_over(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        id: egui::Id,
        menu: &str,
        state: &S,
        strings: &Strings,
    ) -> Option<&'static str> {
        let layer = ui.layer_id();
        let ctx = ui.ctx().clone();
        // The layer check reads the context: outside the input lock.
        let at = ui
            .input(|i| {
                i.pointer
                    .secondary_clicked()
                    .then(|| i.pointer.interact_pos())
            })
            .flatten();
        let opened = at.is_some_and(|at| rect.contains(at) && ctx.layer_id_at(at) == Some(layer));
        let popup = egui::Popup::new(id, ctx, egui::PopupAnchor::PointerFixed, layer)
            .kind(egui::PopupKind::Menu)
            .layout(egui::Layout::top_down_justified(egui::Align::Min))
            .style(egui::containers::menu::menu_style)
            .open_memory(opened.then_some(egui::SetOpenCommand::Bool(true)));
        self.context_popup(popup, menu, state, strings)
    }

    fn context_popup(
        &self,
        popup: egui::Popup<'_>,
        menu: &str,
        state: &S,
        strings: &Strings,
    ) -> Option<&'static str> {
        let mut chosen = None;
        popup.show(|ui| {
            let title = label_text(strings, menu);
            let model = self.model(ui.ctx(), state, strings);
            if let Some(menu) = model.iter().find(|m| m.title == title) {
                chosen = context_entries(ui, &menu.entries);
            }
        });
        chosen
    }

    /// The menu model: first-use order, with Help always last,
    /// rows in registry order, a submenu row where a submenu's first command
    /// is, and a separator wherever the group changes.
    pub fn model(&self, ctx: &egui::Context, state: &S, strings: &Strings) -> Vec<Menu> {
        let mut keys: Vec<&'static str> = Vec::new();
        for command in &self.commands {
            if let Some(key) = command.menu
                && !keys.contains(&key)
            {
                keys.push(key);
            }
        }
        keys.sort_by_key(|key| *key == "help");
        let row = |c: &Action<S>| {
            let shortcut = c.shortcut.map(|s| ctx.format_shortcut(&s));
            let label = label_text(strings, self.label_of(c.id, state).unwrap_or(c.label));
            let enabled = self.core.enabled(c.id, state).is_ok();
            Entry::Row(match self.checked(c.id, state) {
                Some(checked) => Row::choice(c.id, label, shortcut, enabled, checked),
                None => Row::command(c.id, label, shortcut, enabled),
            })
        };
        // Rows with a separator at each change of group.
        let grouped = |commands: &mut dyn Iterator<Item = (u8, Entry)>| {
            let mut entries = Vec::new();
            let mut last = None;
            for (group, entry) in commands {
                if last.is_some_and(|g| g != group) {
                    entries.push(Entry::Separator);
                }
                last = Some(group);
                entries.push(entry);
            }
            entries
        };
        keys.into_iter()
            .map(|key| {
                let in_menu: Vec<&Action<S>> = self
                    .commands
                    .iter()
                    .filter(|c| c.menu == Some(key))
                    .collect();
                let mut placed: Vec<&'static str> = Vec::new();
                let mut top = in_menu.iter().filter_map(|c| match c.submenu {
                    None => Some((c.group, row(c))),
                    Some(sub) if !placed.contains(&sub) => {
                        placed.push(sub);
                        let mut children = in_menu
                            .iter()
                            .filter(|d| d.submenu == Some(sub))
                            .map(|d| (d.group, row(d)));
                        Some((
                            c.group,
                            Entry::Row(Row::submenu(
                                label_text(strings, sub),
                                grouped(&mut children),
                            )),
                        ))
                    }
                    Some(_) => None,
                });
                Menu {
                    title: label_text(strings, key),
                    entries: menu::tidy(grouped(&mut top)),
                }
            })
            .collect()
    }

    /// The enabled commands whose label contains `query` (case-insensitive),
    /// for a command palette.
    pub fn search<'a>(&'a self, query: &str, state: &S, strings: &Strings) -> Vec<&'a Action<S>> {
        let query = query.to_lowercase();
        self.commands
            .iter()
            .filter(|c| {
                self.core.enabled(c.id, state).is_ok()
                    && label_text(strings, self.label_of(c.id, state).unwrap_or(c.label))
                        .to_lowercase()
                        .contains(&query)
            })
            .collect()
    }
}

/// A context menu's `entries` as egui menu buttons; the command chosen.
fn context_entries(ui: &mut egui::Ui, entries: &[Entry]) -> Option<&'static str> {
    // Rows flush against each other, as the menu bar's: a long menu (the
    // email Message menu) keeps its height, and hover highlights touch.
    // Separators keep their own spacing.
    ui.spacing_mut().item_spacing.y = 0.0;
    let mut chosen = None;
    for entry in entries {
        match entry {
            Entry::Separator => {
                ui.separator();
            }
            Entry::Row(row) if row.is_submenu() => {
                ui.add_enabled_ui(row.enabled, |ui| {
                    ui.menu_button(row.label.as_str(), |ui| {
                        if let Some(id) = context_entries(ui, &row.children) {
                            chosen = Some(id);
                        }
                    });
                });
            }
            Entry::Row(row) => {
                let label = match row.checked {
                    Some(true) => format!("{} {}", menu::TICK, row.label),
                    _ => row.label.clone(),
                };
                let mut button = egui::Button::new(label);
                if let Some(shortcut) = &row.shortcut {
                    button = button.shortcut_text(shortcut.as_str());
                }
                if ui.add_enabled(row.enabled, button).clicked() {
                    chosen = row.id;
                    ui.close();
                }
            }
        }
    }
    chosen
}

/// The egui view of an engine-owned command table. Shortcuts and icons are
/// resolved once on construction and after rebinding. Dispatch and enablement
/// always use the original engine registry; no handlers live in this adapter.
/// Commands with `params: Some(_)` are excluded from menus, shortcuts and the
/// palette, which dispatch nullary actions. They remain describable and can be
/// run through `execute` with explicit parameters.
pub struct EngineAdapter<S> {
    pub(crate) ui: Registry<S>,
}

impl<S> EngineAdapter<S> {
    pub fn new(core: EngineRegistry<S>) -> Self {
        // Private UI rows use the legacy metadata shape so both table formats
        // share the same rendering and exact-modifier input code. The dummy
        // callbacks are never exposed or dispatched: core owns every handler.
        let commands = core
            .iter()
            .filter(|c| c.params.is_none())
            .map(|c| Action {
                id: c.id,
                label: c.label,
                menu: c.menu,
                submenu: c.submenu,
                group: c.group,
                shortcut: None,
                icon: c
                    .icon
                    .and_then(|name| Icon::ALL.iter().copied().find(|i| i.name() == name)),
                enabled: always,
                run: choose,
            })
            .collect();
        let mut ui = Registry {
            commands,
            search_menu: None,
            core,
            extras: HashMap::new(),
            parsed_defaults: HashMap::new(),
        };
        ui.refresh_shortcuts();
        Self { ui }
    }

    pub fn registry(&self) -> &EngineRegistry<S> {
        &self.ui.core
    }
    pub fn into_registry(self) -> EngineRegistry<S> {
        self.ui.core
    }
    pub fn execute(
        &self,
        id: &str,
        state: &mut S,
        params: serde_json::Value,
        origin: Origin,
    ) -> Result<serde_json::Value, Error> {
        self.ui.core.execute(id, state, params, origin)
    }
    pub fn note(&self, id: &str, origin: Origin) {
        self.ui.note(id, origin);
    }
    pub fn take_executed(&self) -> (Vec<Executed>, u64) {
        self.ui.take_executed()
    }
    pub fn describe(&self, state: &S, strings: &Strings) -> Vec<Described> {
        self.ui.describe(state, strings)
    }
    pub fn search<'a>(
        &'a self,
        query: &str,
        state: &S,
        strings: &Strings,
    ) -> Vec<&'a EngineAction<S>> {
        self.ui
            .search(query, state, strings)
            .into_iter()
            .filter_map(|c| self.ui.core.get(c.id))
            .collect()
    }
    pub fn icon(&self, id: &str) -> Option<Icon> {
        self.ui.get(id).and_then(|c| c.icon)
    }
    /// Label and parsed accelerator, using the same metadata as menus.
    pub fn tooltip(&self, ctx: &egui::Context, id: &str, strings: &Strings) -> Option<String> {
        self.ui
            .get(id)
            .map(|c| crate::tooltip::for_command(ctx, c, strings))
    }
    pub fn search_menu(&mut self, menu: &'static str) -> &mut Self {
        self.ui.search_menu(menu);
        self
    }
    pub fn rebind(&mut self, overrides: &crate::keymap::Overrides) -> Vec<String> {
        self.ui.rebind(overrides)
    }
    pub fn shortcuts_of(&self, id: &str) -> Vec<KeyboardShortcut> {
        self.ui.shortcuts_of(id)
    }
    pub fn shortcuts(&self, ctx: &egui::Context, state: &S) -> Vec<&'static str> {
        self.ui.shortcuts(ctx, state)
    }
    pub fn takes(&self, event: &egui::Event, state: &S) -> bool {
        self.ui.takes(event, state)
    }
    pub fn shortcut_grid(&self, ui: &mut egui::Ui, strings: &Strings) {
        self.ui.shortcut_grid(ui, strings);
    }
    pub fn model(&self, ctx: &egui::Context, state: &S, strings: &Strings) -> Vec<Menu> {
        self.ui.model(ctx, state, strings)
    }
    pub fn menu_bar(
        &self,
        ui: &mut egui::Ui,
        state: &S,
        strings: &Strings,
    ) -> Vec<(&'static str, Origin)> {
        self.ui.menu_bar(ui, state, strings)
    }
    pub fn menus(
        &self,
        ui: &mut egui::Ui,
        state: &S,
        strings: &Strings,
    ) -> Vec<(&'static str, Origin)> {
        self.ui.menus(ui, state, strings)
    }
    pub fn context_menu(
        &self,
        response: &egui::Response,
        menu: &str,
        state: &S,
        strings: &Strings,
    ) -> Option<&'static str> {
        self.ui.context_menu(response, menu, state, strings)
    }
    #[allow(clippy::too_many_arguments)] // Mirrors the legacy context-menu API.
    pub fn context_menu_over(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        id: egui::Id,
        menu: &str,
        state: &S,
        strings: &Strings,
    ) -> Option<&'static str> {
        self.ui
            .context_menu_over(ui, rect, id, menu, state, strings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Counter {
        n: u32,
        locked: bool,
    }

    const FTL: &str = "menu-edit = Edit\ncmd-bump = Bump\ncmd-reset = Reset\n";

    #[test]
    fn help_is_last_without_reordering_other_menus() {
        let mut registry = Registry::new();
        for (id, menu) in [
            ("h", "help"),
            ("f", "file"),
            ("v", "view"),
            ("h2", "help"),
            ("g", "go"),
        ] {
            registry.add(Action {
                id,
                label: id,
                menu: Some(menu),
                submenu: None,
                group: 0,
                shortcut: None,
                icon: None,
                enabled: |_: &()| true,
                run: |_| {},
            });
        }
        let model = registry.model(
            &egui::Context::default(),
            &(),
            &Strings::new("help = Help\nfile = File\nview = View\ngo = Go\n"),
        );
        assert_eq!(
            model.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(),
            ["File", "View", "Go", "Help"]
        );
        assert_eq!(model[3].entries.len(), 2);
    }

    #[test]
    fn extras_without_an_accelerator_keep_description_and_menu_shape() {
        let mut r = registry();
        let chord = KeyboardShortcut::new(Modifiers::COMMAND, Key::U);
        r.add_shortcuts("count.reset", [chord]);
        r.rebind(&BTreeMap::new());
        let state = Counter::default();
        let strings = Strings::new(FTL);
        let row = serde_json::to_value(&r.describe(&state, &strings)[1]).unwrap();
        assert_eq!(row["shortcut"], serde_json::Value::Null);
        assert_eq!(row["more"], serde_json::json!(["Ctrl+U"]));
        assert!(
            r.model(&egui::Context::default(), &state, &strings)[0].entries[1]
                .row()
                .unwrap()
                .shortcut
                .is_none()
        );
        let press = egui::Event::Key {
            key: Key::U,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: chord.modifiers,
        };
        assert!(r.takes(&press, &state));
        assert_eq!(fired_key(&r, &state, chord), ["count.reset"]);
    }

    #[test]
    fn legacy_execute_dispatches_and_returns_errors_directly() {
        let mut r = registry();
        let mut state = Counter::default();
        r.execute("count.bump", &mut state, Origin::Bus).unwrap();
        assert_eq!(state.n, 1);
        state.locked = true;
        assert_eq!(
            r.execute("count.bump", &mut state, Origin::Bus),
            Err(CommandError::Disabled("count.bump"))
        );
        assert_eq!(
            r.execute("absent", &mut state, Origin::Bus),
            Err(CommandError::Unknown("absent".into()))
        );
        let mut failed = ::action::nullary!("failed", "Failed", |_: &mut Counter| {});
        failed.run = |_, _| Err(Error::Failed("handler failed".into()));
        r.core.add(failed);
        assert!(
            matches!(r.execute("failed", &mut state, Origin::Bus), Err(CommandError::Disabled(message)) if message.contains("handler failure"))
        );
        let mut bad = ::action::nullary!("bad", "Bad", |_: &mut Counter| {});
        bad.run = |_, _| {
            Err(Error::BadParams {
                id: "bad",
                msg: "invalid".into(),
            })
        };
        r.core.add(bad);
        assert!(
            matches!(r.execute("bad", &mut state, Origin::Bus), Err(CommandError::Disabled(message)) if message.contains("invalid parameters"))
        );
        r.core.authorize = Some(|_| Err("refused"));
        for locked in [false, true] {
            state.locked = locked;
            assert_eq!(
                r.execute("count.bump", &mut state, Origin::Bus),
                Err(CommandError::Denied("count.bump"))
            );
        }
    }

    #[test]
    fn typed_engine_commands_execute_but_are_absent_from_nullary_ui() {
        let mut core = EngineRegistry::new();
        let mut c = ::action::nullary!("count.set", "Set", |_: &mut Counter| {});
        c.params = Some(ParamSpec {
            type_name: "Count",
            schema: || serde_json::from_value(serde_json::json!({"type": "object"})).unwrap(),
        });
        c.menu = Some("menu-edit");
        c.shortcut = Some("Ctrl+B");
        c.run = |s, params| {
            let n: u32 =
                serde_json::from_value(params["n"].clone()).map_err(|e| Error::BadParams {
                    id: "count.set",
                    msg: e.to_string(),
                })?;
            s.n = n;
            Ok(serde_json::json!({"n": n}))
        };
        core.add(c);
        let ui = EngineAdapter::new(core);
        let mut state = Counter::default();
        let strings = Strings::new(FTL);
        assert!(
            ui.model(&egui::Context::default(), &state, &strings)
                .is_empty()
        );
        assert!(ui.search("", &state, &strings).is_empty());
        assert!(ui.shortcuts_of("count.set").is_empty());
        let press = egui::Event::Key {
            key: Key::B,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        };
        assert!(!ui.takes(&press, &state));
        assert!(ui.describe(&state, &strings)[0].schema.is_some());
        assert!(matches!(
            ui.execute(
                "count.set",
                &mut state,
                serde_json::Value::Null,
                Origin::Bus
            ),
            Err(Error::BadParams { .. })
        ));
        assert_eq!(
            ui.execute(
                "count.set",
                &mut state,
                serde_json::json!({"n": 42}),
                Origin::Bus
            )
            .unwrap(),
            serde_json::json!({"n": 42})
        );
        assert_eq!(state.n, 42);
        assert_eq!(ui.take_executed().0.len(), 1);
    }

    #[test]
    fn engine_adapter_reads_canonical_modifier_order() {
        let mut core = EngineRegistry::new();
        let mut c = ::action::nullary!("one", "One", |_: &mut Counter| {});
        c.shortcut = Some("Alt+Shift+Ctrl+B");
        core.add(c);
        core.add_shortcuts("one", ["Ctrl+Shift+Alt+B".into()]);
        let ui = EngineAdapter::new(core);
        assert_eq!(
            ui.shortcuts_of("one"),
            [KeyboardShortcut::new(
                Modifiers::COMMAND | Modifiers::SHIFT | Modifiers::ALT,
                Key::B
            )]
        );
        assert_eq!(
            ui.describe(&Counter::default(), &Strings::new(FTL))[0]
                .shortcut
                .as_deref(),
            Some("Ctrl+Alt+Shift+B")
        );
    }

    #[test]
    #[should_panic(
        expected = "duplicate shortcut \"Ctrl+B\" on \"command\" (chord text decides duplicates)"
    )]
    fn ctrl_and_command_duplicates_are_decided_by_chord_text() {
        let mut r: Registry<Counter> = Registry::new();
        for (id, modifiers) in [("ctrl", Modifiers::CTRL), ("command", Modifiers::COMMAND)] {
            r.add(Action {
                id,
                label: id,
                menu: None,
                submenu: None,
                group: 0,
                shortcut: Some(KeyboardShortcut::new(modifiers, Key::B)),
                icon: None,
                enabled: always,
                run: |_| {},
            });
        }
    }

    #[test]
    fn legacy_describe_golden_uses_stored_chord_grammar() {
        let mut r: Registry<Counter> = Registry::new();
        for (id, key) in [
            ("left", Key::ArrowLeft),
            ("right", Key::ArrowRight),
            ("up", Key::ArrowUp),
            ("down", Key::ArrowDown),
            ("equals", Key::Equals),
            ("minus", Key::Minus),
        ] {
            r.add(Action {
                id,
                label: id,
                menu: None,
                submenu: None,
                group: 0,
                shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND, key)),
                icon: None,
                enabled: always,
                run: |_| {},
            });
        }
        r.add_shortcuts(
            "equals",
            [KeyboardShortcut::new(Modifiers::ALT, Key::ArrowLeft)],
        );
        let rows =
            serde_json::to_value(r.describe(&Counter::default(), &Strings::new(FTL))).unwrap();
        let golden = [
            "Ctrl+ArrowLeft",
            "Ctrl+ArrowRight",
            "Ctrl+ArrowUp",
            "Ctrl+ArrowDown",
            "Ctrl+=",
            "Ctrl+-",
        ];
        for (row, shortcut) in rows.as_array().unwrap().iter().zip(golden) {
            let id = row["id"].as_str().unwrap();
            let mut expected = serde_json::json!({"id": id, "label": id, "menu": null, "shortcut": shortcut,
                "enabled": true, "checked": null, "reason": null, "class": "intent", "schema": null});
            if id == "equals" {
                expected["more"] = serde_json::json!(["Alt+ArrowLeft"]);
            }
            assert_eq!(row, &expected);
        }
    }

    #[test]
    fn explicit_ctrl_defaults_survive_extras_rebinding_and_restoration() {
        let raw = KeyboardShortcut::new(Modifiers::CTRL, Key::B);
        let mut r: Registry<Counter> = Registry::new();
        r.add(Action {
            id: "raw",
            label: "cmd-bump",
            menu: None,
            submenu: None,
            group: 0,
            shortcut: Some(raw),
            icon: None,
            enabled: always,
            run: |_| {},
        });
        r.add_shortcuts("raw", [KeyboardShortcut::new(Modifiers::CTRL, Key::U)]);
        assert_eq!(r.shortcuts_of("raw")[0], raw);
        let press = egui::Event::Key {
            key: Key::B,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::CTRL,
        };
        assert!(r.takes(&press, &Counter::default()));
        r.rebind(&BTreeMap::new());
        assert_eq!(r.shortcuts_of("raw")[0], raw);
        assert!(r.takes(&press, &Counter::default()));
        r.rebind(&[("raw".to_owned(), vec!["Ctrl+B".to_owned()])].into());
        assert_eq!(r.shortcuts_of("raw")[0].modifiers, Modifiers::COMMAND);
        r.rebind(&BTreeMap::new());
        assert_eq!(r.shortcuts_of("raw")[0], raw);
        assert!(r.takes(&press, &Counter::default()));
    }

    #[test]
    fn engine_owned_tables_use_the_same_input_rendering_and_dispatch() {
        let mut core = EngineRegistry::new();
        let mut c = ::action::nullary!("count.bump", "cmd-bump", |s: &mut Counter| s.n += 1);
        c.shortcut = Some("Ctrl+Shift+B");
        c.icon = Some("plus"); // Unshipped icon names have no image.
        c.menu = Some("menu-edit");
        c.enabled = |s| if s.locked { Err("locked") } else { Ok(()) };
        core.add(c);
        let ui = EngineAdapter::new(core);
        let mut s = Counter::default();
        let key = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::B);
        assert_eq!(ui.shortcuts_of("count.bump"), [key]);
        let press = egui::Event::Key {
            key: Key::B,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: key.modifiers,
        };
        assert!(ui.takes(&press, &s));
        assert_eq!(
            ui.search("bump", &s, &Strings::new(FTL))[0].id,
            "count.bump"
        );
        assert_eq!(ui.icon("count.bump"), None);
        assert_eq!(
            ui.execute(
                "count.bump",
                &mut s,
                serde_json::Value::Null,
                Origin::Shortcut
            )
            .unwrap(),
            serde_json::Value::Null
        );
        assert_eq!(s.n, 1);
        assert_eq!(ui.take_executed().0[0].origin, Origin::Shortcut);
        s.locked = true;
        assert!(!ui.takes(&press, &s));
        assert_eq!(
            ui.describe(&s, &Strings::new(FTL))[0].reason,
            Some("locked")
        );
        assert!(
            !ui.model(&egui::Context::default(), &s, &Strings::new(FTL))[0].entries[0]
                .row()
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn legacy_description_fields_use_canonical_chords() {
        let r = registry();
        let row =
            serde_json::to_value(&r.describe(&Counter::default(), &Strings::new(FTL))[0]).unwrap();
        assert_eq!(
            row,
            serde_json::json!({
                "id": "count.bump", "label": "Bump", "menu": "Edit",
                "shortcut": "Ctrl+B",
                "enabled": true, "checked": null, "reason": null, "class": "intent", "schema": null
            })
        );
    }

    fn registry() -> Registry<Counter> {
        let mut r = Registry::new();
        r.add(Action {
            id: "count.bump",
            label: "cmd-bump",
            menu: Some("menu-edit"),
            submenu: None,
            group: 0,
            shortcut: Some(KeyboardShortcut::new(Modifiers::COMMAND, Key::B)),
            icon: None,
            enabled: |s: &Counter| !s.locked,
            run: |s| s.n += 1,
        })
        .add(Action {
            id: "count.reset",
            label: "cmd-reset",
            menu: Some("menu-edit"),
            submenu: None,
            group: 0,
            shortcut: None,
            icon: None,
            enabled: always,
            run: |s| s.n = 0,
        });
        r
    }

    #[test]
    fn accepted_runs_are_recorded_in_order_and_refusals_are_not() {
        let r = registry();
        let mut s = Counter::default();
        r.execute("count.bump", &mut s, Origin::Menu).unwrap();
        assert!(r.execute("nope", &mut s, Origin::Bus).is_err());
        s.locked = true;
        assert!(r.execute("count.bump", &mut s, Origin::Bus).is_err());
        r.execute("count.reset", &mut s, Origin::Bus).unwrap();
        r.note("view.mode", Origin::Ui);
        let (runs, lost) = r.take_executed();
        let got: Vec<(&str, Origin, u64)> = runs
            .iter()
            .map(|e| (e.id.as_str(), e.origin, e.seq))
            .collect();
        assert_eq!(
            got,
            vec![
                ("count.bump", Origin::Menu, 1),
                ("count.reset", Origin::Bus, 2),
                ("view.mode", Origin::Ui, 3)
            ]
        );
        assert_eq!(lost, 0);
        assert_eq!(r.take_executed(), (Vec::new(), 0));
    }

    #[test]
    fn execute_dispatches_by_id_and_honours_enablement() {
        let r = registry();
        let mut s = Counter::default();
        r.execute("count.bump", &mut s, crate::action::Origin::Bus)
            .unwrap();
        assert_eq!(s.n, 1);
        s.locked = true;
        assert_eq!(
            r.execute("count.bump", &mut s, crate::action::Origin::Bus),
            Err(CommandError::Disabled("count.bump"))
        );
        assert_eq!(
            r.execute("nope", &mut s, crate::action::Origin::Bus),
            Err(CommandError::Unknown("nope".into()))
        );
        r.execute("count.reset", &mut s, crate::action::Origin::Bus)
            .unwrap();
        assert_eq!(s.n, 0);
    }

    #[test]
    fn describe_is_localised_and_reports_enablement() {
        let strings = Strings::new(FTL);
        let d = registry().describe(
            &Counter {
                locked: true,
                ..Counter::default()
            },
            &strings,
        );
        assert_eq!(d[0].label, "Bump");
        assert_eq!(d[0].menu.as_deref(), Some("Edit"));
        assert!(!d[0].enabled);
        assert!(d[0].shortcut.as_deref().is_some_and(|s| s.contains('B')));
        assert!(d[1].enabled);
    }

    #[test]
    fn search_finds_enabled_commands_by_label() {
        let strings = Strings::new(FTL);
        let r = registry();
        let ids: Vec<_> = r
            .search("re", &Counter::default(), &strings)
            .iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, ["count.reset"]);
    }

    #[test]
    fn the_model_groups_rows_and_gathers_submenus() {
        let strings = Strings::new("menu-edit = Edit\nsub = More\n");
        let mut r = registry();
        let add = |r: &mut Registry<Counter>, id, submenu, group| {
            r.add(Action {
                id,
                label: id,
                menu: Some("menu-edit"),
                submenu,
                group,
                shortcut: None,
                icon: None,
                enabled: always,
                run: |_| {},
            });
        };
        add(&mut r, "a", Some("sub"), 1);
        add(&mut r, "b", None, 1);
        add(&mut r, "c", Some("sub"), 2);
        let model = r.model(
            &egui::Context::default(),
            &Counter {
                locked: true,
                ..Counter::default()
            },
            &strings,
        );
        let edit = &model[0];
        assert_eq!(edit.title, "Edit");
        let labels: Vec<_> = edit
            .entries
            .iter()
            .map(|e| e.row().map(|r| r.label.as_str()))
            .collect();
        assert_eq!(
            labels,
            [
                Some("cmd-bump"),
                Some("cmd-reset"),
                None,
                Some("More"),
                Some("b")
            ]
        );
        let bump = edit.entries[0].row().unwrap();
        assert!(!bump.enabled && bump.shortcut.as_deref().is_some_and(|s| s.contains('B')));
        let more = edit.entries[3].row().unwrap();
        assert!(more.is_submenu() && more.enabled);
        let children: Vec<_> = more
            .children
            .iter()
            .map(|e| e.row().map(|r| r.label.as_str()))
            .collect();
        assert_eq!(
            children,
            [Some("a"), None, Some("c")],
            "a group change inside the submenu"
        );
    }

    /// Ctrl+S and Ctrl+Shift+S, Ctrl+Shift+S registered last and disabled
    /// when `locked`.
    fn overlapping() -> Registry<Counter> {
        let mut r = Registry::new();
        let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        let save_as = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::S);
        r.add(Action {
            id: "file.save",
            label: "x",
            menu: None,
            submenu: None,
            group: 0,
            shortcut: Some(save),
            icon: None,
            enabled: always,
            run: |_| {},
        })
        .add(Action {
            id: "file.save_as",
            label: "y",
            menu: None,
            submenu: None,
            group: 0,
            shortcut: Some(save_as),
            icon: None,
            enabled: |s: &Counter| !s.locked,
            run: |_| {},
        });
        r
    }

    fn fired(r: &Registry<Counter>, s: &Counter, presses: &[Modifiers]) -> Vec<&'static str> {
        let ctx = egui::Context::default();
        let mut out = Vec::new();
        let events = presses
            .iter()
            .map(|&modifiers| egui::Event::Key {
                key: Key::S,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            })
            .collect();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| out = r.shortcuts(ui.ctx(), s),
        );
        output.textures_delta.clear();
        out
    }

    #[test]
    fn shortcuts_match_modifiers_exactly_and_in_press_order() {
        let r = overlapping();
        let shift = Modifiers::COMMAND | Modifiers::SHIFT;
        assert_eq!(fired(&r, &Counter::default(), &[shift]), ["file.save_as"]);
        assert_eq!(
            fired(
                &r,
                &Counter::default(),
                &[Modifiers::COMMAND, shift, Modifiers::COMMAND]
            ),
            ["file.save", "file.save_as", "file.save"]
        );
        let locked = Counter {
            locked: true,
            ..Counter::default()
        };
        assert!(
            fired(&r, &locked, &[shift]).is_empty(),
            "a disabled specific shortcut never falls back to the simpler one"
        );
        assert!(
            fired(
                &r,
                &Counter::default(),
                &[Modifiers::COMMAND | Modifiers::ALT]
            )
            .is_empty()
        );
    }

    #[test]
    fn rebinding_takes_moves_unbinds_and_restores() {
        let ctrl = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
        let mut r = registry();
        r.add_shortcuts("count.bump", [ctrl(Key::U), ctrl(Key::B)]);
        assert_eq!(
            r.shortcuts_of("count.bump"),
            [ctrl(Key::B), ctrl(Key::U)],
            "a chord already bound is skipped"
        );
        let s = Counter::default();
        assert_eq!(
            fired_key(&r, &s, ctrl(Key::U)),
            ["count.bump"],
            "a further chord runs it"
        );
        let overrides = |pairs: &[(&str, &[&str])]| {
            pairs
                .iter()
                .map(|(id, chords)| {
                    (
                        (*id).to_owned(),
                        chords.iter().map(|c| (*c).to_owned()).collect(),
                    )
                })
                .collect::<crate::keymap::Overrides>()
        };
        // Reset takes Ctrl+B; Bump keeps Ctrl+U, promoted to its accelerator.
        let notes = r.rebind(&overrides(&[("count.reset", &["Ctrl+B", "F9"])]));
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            r.shortcuts_of("count.reset"),
            [
                ctrl(Key::B),
                KeyboardShortcut::new(Modifiers::NONE, Key::F9)
            ]
        );
        assert_eq!(r.get("count.bump").unwrap().shortcut, Some(ctrl(Key::U)));
        assert_eq!(fired_key(&r, &s, ctrl(Key::B)), ["count.reset"]);
        let d = r.describe(&s, &Strings::new(FTL));
        assert_eq!(d[1].more.len(), 1, "F9 is reported");
        // Unbinding, unknown commands and keys, a chord claimed twice.
        let notes = r.rebind(&overrides(&[
            ("count.bump", &[]),
            ("count.reset", &["Ctrl+Nope", "Ctrl+B"]),
            ("nope", &["F1"]),
            ("count.zzz", &["F2"]),
        ]));
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(r.shortcuts_of("count.bump").is_empty());
        assert_eq!(r.shortcuts_of("count.reset"), [ctrl(Key::B)]);
        // An empty profile entry restores the built chords.
        assert!(r.rebind(&BTreeMap::new()).is_empty());
        assert_eq!(r.shortcuts_of("count.bump"), [ctrl(Key::B), ctrl(Key::U)]);
        assert!(r.shortcuts_of("count.reset").is_empty());
        // A list with no key the app knows (a typo) keeps the defaults; the
        // same chord spelled twice is noted as listed twice.
        let notes = r.rebind(&overrides(&[
            ("count.bump", &["Nope"]),
            ("count.reset", &["Ctrl+R", "Ctrl+r"]),
        ]));
        assert_eq!(r.shortcuts_of("count.bump"), [ctrl(Key::B), ctrl(Key::U)]);
        assert_eq!(r.shortcuts_of("count.reset"), [ctrl(Key::R)]);
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(notes[2].contains("listed twice"), "{notes:?}");
    }

    /// egui-winit turns Ctrl+C, Ctrl+X and Ctrl+V into clipboard events;
    /// with no text field focused they run what those keys are bound to.
    #[test]
    fn clipboard_events_run_their_key_bindings() {
        let mut r: Registry<Counter> = Registry::new();
        let bind = |id, modifiers, key| Action {
            id,
            label: "x",
            menu: None,
            submenu: None,
            group: 0,
            shortcut: Some(KeyboardShortcut::new(modifiers, key)),
            icon: None,
            enabled: always,
            run: |_| {},
        };
        r.add(bind("paste", Modifiers::COMMAND, Key::V));
        r.add(bind("copy", Modifiers::COMMAND, Key::C)).add(bind(
            "cut.more",
            Modifiers::COMMAND | Modifiers::SHIFT,
            Key::X,
        ));
        let fire = |held: Modifiers, event: egui::Event| {
            let ctx = egui::Context::default();
            let mut out = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::ModifiersChanged(held), event],
                    ..Default::default()
                },
                |ui| out = r.shortcuts(ui.ctx(), &Counter::default()),
            );
            output.textures_delta.clear();
            out
        };
        assert_eq!(fire(Modifiers::COMMAND, egui::Event::Copy), ["copy"]);
        assert_eq!(
            fire(Modifiers::COMMAND | Modifiers::SHIFT, egui::Event::Cut),
            ["cut.more"]
        );
        assert!(fire(Modifiers::COMMAND, egui::Event::Cut).is_empty());
        assert_eq!(
            fire(Modifiers::COMMAND, egui::Event::Paste("p".into())),
            ["paste"]
        );
        assert_eq!(
            fire(
                Modifiers::COMMAND,
                egui::Event::PasteImage(std::sync::Arc::new(egui::ColorImage::filled(
                    [1, 1],
                    egui::Color32::WHITE
                )))
            ),
            ["paste"]
        );
    }

    fn fired_key(
        r: &Registry<Counter>,
        s: &Counter,
        shortcut: KeyboardShortcut,
    ) -> Vec<&'static str> {
        let ctx = egui::Context::default();
        let mut out = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: shortcut.logical_key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: shortcut.modifiers,
                }],
                ..Default::default()
            },
            |ui| out = r.shortcuts(ui.ctx(), s),
        );
        output.textures_delta.clear();
        out
    }

    /// A command on `shortcut` by default.
    fn on(id: &'static str, shortcut: KeyboardShortcut) -> Action<Counter> {
        Action {
            id,
            label: "x",
            menu: None,
            submenu: None,
            group: 0,
            shortcut: Some(shortcut),
            icon: None,
            enabled: always,
            run: |_| {},
        }
    }

    #[test]
    #[should_panic(expected = "F1-F12 belong to the user")]
    fn a_default_bare_function_key_is_refused() {
        let f1 = KeyboardShortcut::new(Modifiers::NONE, Key::F1);
        Registry::new().add(on("help.shortcuts", f1));
    }

    #[test]
    #[should_panic(expected = "F1-F12 belong to the user")]
    fn a_default_ctrl_function_key_is_refused() {
        let ctrl_f5 = KeyboardShortcut::new(Modifiers::COMMAND, Key::F5);
        Registry::new().add(on("view.refresh", ctrl_f5));
    }

    #[test]
    #[should_panic(expected = "F1-F12 belong to the user")]
    fn a_default_shift_function_key_is_refused() {
        let shift_f3 = KeyboardShortcut::new(Modifiers::SHIFT, Key::F3);
        Registry::new().add(on("find.prev", shift_f3));
    }

    #[test]
    #[should_panic(expected = "F1-F12 belong to the user")]
    fn a_further_default_function_key_is_refused() {
        let alt_f12 = KeyboardShortcut::new(Modifiers::ALT, Key::F12);
        registry().add_shortcuts("count.bump", [alt_f12]);
    }

    #[test]
    fn the_person_may_bind_a_function_key() {
        let mut r = registry();
        let notes = r.rebind(&BTreeMap::from([(
            "count.bump".to_owned(),
            vec!["F5".to_owned(), "Ctrl+Shift+F11".to_owned()],
        )]));
        assert!(notes.is_empty(), "{notes:?}");
        let f5 = KeyboardShortcut::new(Modifiers::NONE, Key::F5);
        assert_eq!(
            r.shortcuts_of("count.bump"),
            [
                f5,
                KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::F11)
            ]
        );
        assert_eq!(fired_key(&r, &Counter::default(), f5), ["count.bump"]);
        // Restoring the defaults leaves no function key bound.
        assert!(r.rebind(&BTreeMap::new()).is_empty());
        assert!(
            r.iter()
                .flat_map(|c| r.shortcuts_of(c.id))
                .all(|s| !crate::keymap::user_only(s.logical_key))
        );
    }

    #[test]
    fn a_browsing_chord_waits_while_a_text_field_has_the_keyboard() {
        let bare = KeyboardShortcut::new(Modifiers::NONE, Key::S);
        let mut r = registry();
        r.add(Action {
            id: "count.bare",
            label: "cmd-bump",
            menu: None,
            submenu: None,
            group: 0,
            shortcut: Some(bare),
            icon: None,
            enabled: always,
            run: |s| s.n += 10,
        });
        r.scope("count.bare", Scope::Browsing);
        assert_eq!(r.scope_of("count.bare"), Scope::Browsing);
        assert_eq!(r.scope_of("count.bump"), Scope::Anywhere);
        let s = Counter::default();
        assert_eq!(fired_key(&r, &s, bare), ["count.bare"], "nothing focused");
        // The same presses with a text field focused.
        let ctx = egui::Context::default();
        let field = egui::Id::unique("field");
        let mut text = String::new();
        let mut frame = |events: Vec<egui::Event>| {
            let mut out = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    out = r.shortcuts(ui.ctx(), &s);
                    ui.add(egui::TextEdit::singleline(&mut text).id(field));
                },
            );
            output.textures_delta.clear();
            out
        };
        frame(Vec::new());
        ctx.memory_mut(|m| m.request_focus(field));
        frame(Vec::new());
        let press = |shortcut: KeyboardShortcut| egui::Event::Key {
            key: shortcut.logical_key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: shortcut.modifiers,
        };
        assert!(frame(vec![press(bare)]).is_empty(), "the field keeps S");
        assert_eq!(
            frame(vec![press(KeyboardShortcut::new(
                Modifiers::COMMAND,
                Key::B
            ))]),
            ["count.bump"],
            "an anywhere chord still fires"
        );
    }

    #[test]
    fn a_relabelled_command_reads_its_state_everywhere() {
        let mut r = registry();
        r.relabel("count.reset", |s: &Counter| {
            if s.n == 0 { "cmd-bump" } else { "cmd-reset" }
        });
        let strings = Strings::new(FTL);
        let ctx = egui::Context::default();
        let label = |s: &Counter| {
            let model = r.model(&ctx, s, &strings);
            model[0].entries[1].row().unwrap().label.clone()
        };
        assert_eq!(label(&Counter::default()), "Bump");
        let counted = Counter {
            n: 3,
            ..Counter::default()
        };
        assert_eq!(label(&counted), "Reset");
        let described = r.describe(&counted, &strings);
        assert_eq!(described[1].label, "Reset");
        assert_eq!(
            r.label_of("count.reset", &Counter::default()),
            Some("cmd-bump")
        );
        assert_eq!(r.label_of("count.bump", &counted), Some("cmd-bump"));
    }

    #[test]
    #[should_panic(expected = "duplicate command id")]
    fn duplicate_ids_are_refused() {
        let mut r = registry();
        r.add(Action {
            id: "count.bump",
            label: "x",
            menu: None,
            submenu: None,
            group: 0,
            shortcut: None,
            icon: None,
            enabled: always,
            run: |_| {},
        });
    }
}
