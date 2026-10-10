// SPDX-License-Identifier: MIT OR Apache-2.0
//! The command registry: every action an application offers is a
//! [`Command`] with a stable id, and the menus, shortcuts, palette and Bus
//! surface are generated from the registry. The UI, the `mix` command line
//! and Bus verbs all dispatch by id through [`Registry::execute`], so anything
//! a person can do in an application an agent can do too.
//!
//! A command's handler takes the application's state (`S`) and changes it;
//! work that leaves the process (a Bus call, a file) is queued on the state
//! for the application's effect loop, never performed inside the handler.

use crate::icons::Icon;
use crate::menu::{self, Entry, Menu, Row};
use crate::strings::Strings;
use egui::{KeyboardShortcut, ModifierNames};
use serde::Serialize;
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

/// One action.
pub struct Command<S> {
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
    Disabled(&'static str),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(id) => write!(f, "unknown command {id:?}"),
            Self::Disabled(id) => write!(f, "command {id:?} is disabled"),
        }
    }
}

impl std::error::Error for CommandError {}

/// A command as the Bus and the palette describe it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Described {
    pub id: &'static str,
    pub label: String,
    pub menu: Option<String>,
    pub shortcut: Option<String>,
    pub enabled: bool,
    /// A choice command's tick; `None` for an ordinary command.
    pub checked: Option<bool>,
}

/// Runs a choice command: a closure, so it can carry its choice.
type ChoiceRun<S> = Box<dyn Fn(&mut S)>;

/// Whether a choice command is the current choice.
type ChoiceChecked<S> = Box<dyn Fn(&S) -> bool>;

/// The application's commands, in menu order.
pub struct Registry<S> {
    commands: Vec<Command<S>>,
    /// Fluent key of the menu that opens with a search field (§3.5).
    search_menu: Option<&'static str>,
    /// Choice commands ([`Registry::add_choice`]): their runs and ticks,
    /// by command id. Their [`Command`] entries carry everything else.
    choice_runs: HashMap<&'static str, ChoiceRun<S>>,
    choice_checks: HashMap<&'static str, ChoiceChecked<S>>,
}

impl<S> Default for Registry<S> {
    fn default() -> Self {
        Self { commands: Vec::new(), search_menu: None, choice_runs: HashMap::new(), choice_checks: HashMap::new() }
    }
}

/// Where a choice command sits and what it shows: a [`Command`]'s fields
/// less its `run` and `enabled` (a choice can always be chosen).
#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: Option<&'static str>,
    pub submenu: Option<&'static str>,
    pub group: u8,
}

/// A choice command's own run does nothing: the registry runs its closure.
fn choose<S>(_: &mut S) {}

impl<S> Registry<S> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `command`.
    ///
    /// # Panics
    /// On a duplicate id or shortcut: both are programming errors that would
    /// otherwise make dispatch ambiguous.
    pub fn add(&mut self, command: Command<S>) -> &mut Self {
        assert!(self.get(command.id).is_none(), "duplicate command id {:?}", command.id);
        if let Some(shortcut) = command.shortcut {
            assert!(
                !self.commands.iter().any(|c| c.shortcut == Some(shortcut)),
                "duplicate shortcut on {:?}",
                command.id
            );
        }
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
    pub fn add_choice(&mut self, place: Place, run: impl Fn(&mut S) + 'static, checked: impl Fn(&S) -> bool + 'static) -> &mut Self {
        let Place { id, label, menu, submenu, group } = place;
        self.add(Command { id, label, menu, submenu, group, shortcut: None, icon: None, enabled: always, run: choose });
        self.choice_runs.insert(id, Box::new(run));
        self.choice_checks.insert(id, Box::new(checked));
        self
    }

    /// Whether command `id` is a choice, and if so whether it is ticked.
    pub fn checked(&self, id: &str, state: &S) -> Option<bool> {
        self.choice_checks.get(id).map(|checked| checked(state))
    }

    /// Open the menu `menu` (a Fluent menu key, usually Help) with a field
    /// that searches every command (§3.5).
    pub fn search_menu(&mut self, menu: &'static str) -> &mut Self {
        self.search_menu = Some(menu);
        self
    }

    pub fn get(&self, id: &str) -> Option<&Command<S>> {
        self.commands.iter().find(|c| c.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Command<S>> {
        self.commands.iter()
    }

    /// Run the command `id` against `state`: the one dispatch path for the
    /// UI, the command line and the Bus.
    pub fn execute(&self, id: &str, state: &mut S) -> Result<(), CommandError> {
        let command = self.get(id).ok_or_else(|| CommandError::Unknown(id.to_owned()))?;
        if !(command.enabled)(state) {
            return Err(CommandError::Disabled(command.id));
        }
        match self.choice_runs.get(command.id) {
            Some(run) => run(state),
            None => (command.run)(state),
        }
        Ok(())
    }

    /// Every command with its current enablement, for `<app>.commands`.
    pub fn describe(&self, state: &S, strings: &Strings) -> Vec<Described> {
        self.commands
            .iter()
            .map(|c| Described {
                id: c.id,
                label: label_text(strings, c.label),
                menu: c.menu.map(|m| label_text(strings, m)),
                shortcut: c.shortcut.map(|s| s.format(&ModifierNames::NAMES, false)),
                enabled: (c.enabled)(state),
                checked: self.checked(c.id, state),
            })
            .collect()
    }

    /// Ids of the enabled commands whose shortcut was pressed this frame, in
    /// press order, consuming those key presses. None fire while a menu is
    /// open: the open menu has the keyboard (chrome specification §3.6).
    ///
    /// Modifiers match exactly, unlike egui's `consume_shortcut`, which
    /// ignores extra Shift and Alt: Ctrl+Shift+S never runs the Ctrl+S
    /// command, even when the Ctrl+Shift+S command is disabled.
    pub fn shortcuts(&self, ctx: &egui::Context, state: &S) -> Vec<&'static str> {
        let mut fired = Vec::new();
        if menu::is_open(ctx) {
            return fired;
        }
        ctx.input_mut(|i| {
            i.events.retain(|event| {
                let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { return true };
                let hit = self.commands.iter().find(|c| {
                    c.shortcut.is_some_and(|s| s.logical_key == *key && modifiers.matches_exact(s.modifiers))
                });
                match hit {
                    Some(command) if (command.enabled)(state) => {
                        fired.push(command.id);
                        false
                    }
                    _ => true,
                }
            });
        });
        fired
    }

    /// A menu bar of its own, for an app without the title bar
    /// ([`crate::titlebar`]). Returns the ids of the commands clicked.
    pub fn menu_bar(&self, ui: &mut egui::Ui, state: &S, strings: &Strings) -> Vec<&'static str> {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            self.menus(ui, state, strings)
        })
        .inner
    }

    /// The menus ([`menu::bar`]) drawn into the current left-to-right row.
    /// Returns the ids chosen this frame.
    pub fn menus(&self, ui: &mut egui::Ui, state: &S, strings: &Strings) -> Vec<&'static str> {
        let model = self.model(ui.ctx(), state, strings);
        let search = self.search_menu.and_then(|key| {
            let title = label_text(strings, key);
            let menu = model.iter().position(|m| m.title == title)?;
            Some(menu::Search { menu, hint: crate::strings::own("search-menus"), empty: crate::strings::own("no-matching-commands") })
        });
        menu::bar_with(ui, &model, search.as_ref()).into_iter().collect()
    }

    /// The menu model: one menu per distinct `menu` key in first-use order,
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
        let row = |c: &Command<S>| {
            let shortcut = c.shortcut.map(|s| ctx.format_shortcut(&s));
            let label = label_text(strings, c.label);
            let enabled = (c.enabled)(state);
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
                let in_menu: Vec<&Command<S>> = self.commands.iter().filter(|c| c.menu == Some(key)).collect();
                let mut placed: Vec<&'static str> = Vec::new();
                let mut top = in_menu.iter().filter_map(|c| match c.submenu {
                    None => Some((c.group, row(c))),
                    Some(sub) if !placed.contains(&sub) => {
                        placed.push(sub);
                        let mut children = in_menu.iter().filter(|d| d.submenu == Some(sub)).map(|d| (d.group, row(d)));
                        Some((c.group, Entry::Row(Row::submenu(label_text(strings, sub), grouped(&mut children)))))
                    }
                    Some(_) => None,
                });
                Menu { title: label_text(strings, key), entries: menu::tidy(grouped(&mut top)) }
            })
            .collect()
    }

    /// The enabled commands whose label contains `query` (case-insensitive),
    /// for a command palette.
    pub fn search<'a>(&'a self, query: &str, state: &S, strings: &Strings) -> Vec<&'a Command<S>> {
        let query = query.to_lowercase();
        self.commands
            .iter()
            .filter(|c| (c.enabled)(state) && label_text(strings, c.label).to_lowercase().contains(&query))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    #[derive(Default)]
    struct Counter {
        n: u32,
        locked: bool,
    }

    const FTL: &str = "menu-edit = Edit\ncmd-bump = Bump\ncmd-reset = Reset\n";

    fn registry() -> Registry<Counter> {
        let mut r = Registry::new();
        r.add(Command {
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
        .add(Command {
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
    fn execute_dispatches_by_id_and_honours_enablement() {
        let r = registry();
        let mut s = Counter::default();
        r.execute("count.bump", &mut s).unwrap();
        assert_eq!(s.n, 1);
        s.locked = true;
        assert_eq!(r.execute("count.bump", &mut s), Err(CommandError::Disabled("count.bump")));
        assert_eq!(r.execute("nope", &mut s), Err(CommandError::Unknown("nope".into())));
        r.execute("count.reset", &mut s).unwrap();
        assert_eq!(s.n, 0);
    }

    #[test]
    fn describe_is_localised_and_reports_enablement() {
        let strings = Strings::new(FTL);
        let d = registry().describe(&Counter { locked: true, ..Counter::default() }, &strings);
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
        let ids: Vec<_> = r.search("re", &Counter::default(), &strings).iter().map(|c| c.id).collect();
        assert_eq!(ids, ["count.reset"]);
    }

    #[test]
    fn the_model_groups_rows_and_gathers_submenus() {
        let strings = Strings::new("menu-edit = Edit\nsub = More\n");
        let mut r = registry();
        let add = |r: &mut Registry<Counter>, id, submenu, group| {
            r.add(Command { id, label: id, menu: Some("menu-edit"), submenu, group, shortcut: None, icon: None, enabled: always, run: |_| {} });
        };
        add(&mut r, "a", Some("sub"), 1);
        add(&mut r, "b", None, 1);
        add(&mut r, "c", Some("sub"), 2);
        let model = r.model(&egui::Context::default(), &Counter { locked: true, ..Counter::default() }, &strings);
        let edit = &model[0];
        assert_eq!(edit.title, "Edit");
        let labels: Vec<_> = edit.entries.iter().map(|e| e.row().map(|r| r.label.as_str())).collect();
        assert_eq!(labels, [Some("cmd-bump"), Some("cmd-reset"), None, Some("More"), Some("b")]);
        let bump = edit.entries[0].row().unwrap();
        assert!(!bump.enabled && bump.shortcut.as_deref().is_some_and(|s| s.contains('B')));
        let more = edit.entries[3].row().unwrap();
        assert!(more.is_submenu() && more.enabled);
        let children: Vec<_> = more.children.iter().map(|e| e.row().map(|r| r.label.as_str())).collect();
        assert_eq!(children, [Some("a"), None, Some("c")], "a group change inside the submenu");
    }

    /// Ctrl+S and Ctrl+Shift+S, Ctrl+Shift+S registered last and disabled
    /// when `locked`.
    fn overlapping() -> Registry<Counter> {
        let mut r = Registry::new();
        let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        let save_as = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::S);
        r.add(Command { id: "file.save", label: "x", menu: None, submenu: None, group: 0, shortcut: Some(save), icon: None, enabled: always, run: |_| {} })
            .add(Command {
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
            .map(|&modifiers| egui::Event::Key { key: Key::S, physical_key: None, pressed: true, repeat: false, modifiers })
            .collect();
        let mut output = ctx.run_ui(egui::RawInput { events, ..Default::default() }, |ui| out = r.shortcuts(ui.ctx(), s));
        output.textures_delta.clear();
        out
    }

    #[test]
    fn shortcuts_match_modifiers_exactly_and_in_press_order() {
        let r = overlapping();
        let shift = Modifiers::COMMAND | Modifiers::SHIFT;
        assert_eq!(fired(&r, &Counter::default(), &[shift]), ["file.save_as"]);
        assert_eq!(fired(&r, &Counter::default(), &[Modifiers::COMMAND, shift, Modifiers::COMMAND]), ["file.save", "file.save_as", "file.save"]);
        let locked = Counter { locked: true, ..Counter::default() };
        assert!(fired(&r, &locked, &[shift]).is_empty(), "a disabled specific shortcut never falls back to the simpler one");
        assert!(fired(&r, &Counter::default(), &[Modifiers::COMMAND | Modifiers::ALT]).is_empty());
    }

    #[test]
    #[should_panic(expected = "duplicate command id")]
    fn duplicate_ids_are_refused() {
        let mut r = registry();
        r.add(Command { id: "count.bump", label: "x", menu: None, submenu: None, group: 0, shortcut: None, icon: None, enabled: always, run: |_| {} });
    }
}
