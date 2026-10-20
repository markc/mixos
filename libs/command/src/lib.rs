// SPDX-License-Identifier: MIT OR Apache-2.0
//! UI-free command tables and the single dispatch path for an application.
//! Toolkit supplies the egui adapter; engine crates depend only on this crate.

pub mod executed;
pub mod keymap;
pub mod strings;

pub use executed::{Executed, Journal, Origin};
use schemars::Schema;
use serde::Serialize;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
pub use strings::Strings;

/// Label lookup, including any catalogue fallback supplied by a UI adapter.
pub trait Labels {
    fn label(&self, key: &str) -> String;
}

impl Labels for Strings {
    fn label(&self, key: &str) -> String {
        self.get(key)
    }
}

pub fn label_text(strings: &impl Labels, key: &str) -> String {
    strings.label(key)
}

/// The wire verb class. Queries do not enter the executed journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Intent,
    Query,
    Input,
    System,
}

impl Class {
    pub const fn journal(self) -> bool {
        !matches!(self, Self::Query)
    }
}

#[derive(Clone, Copy)]
pub struct ParamSpec {
    pub type_name: &'static str,
    pub schema: fn() -> Schema,
}

/// One action. Handlers deserialize their declared parameter type and return
/// `BadParams` on invalid input; schema reporting does not execute a validator.
pub struct Command<S> {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: Option<&'static str>,
    pub submenu: Option<&'static str>,
    pub group: u8,
    pub shortcut: Option<&'static str>,
    /// The Lucide icon name.
    pub icon: Option<&'static str>,
    pub enabled: fn(&S) -> Result<(), &'static str>,
    pub run: fn(&mut S, &Value) -> Result<Value, Error>,
    pub params: Option<ParamSpec>,
    pub class: Class,
}

pub fn always<S>(_: &S) -> Result<(), &'static str> {
    Ok(())
}

/// Wrap a non-capturing `fn(&mut S)` expression in a nullary intent command.
///
/// ```
/// let c: command::Command<u32> = command::nullary!("count.bump", "bump", |s: &mut u32| *s += 1);
/// let mut r = command::Registry::new();
/// r.add(c);
/// assert_eq!(r.execute("count.bump", &mut 0, serde_json::Value::Null, command::Origin::Bus).unwrap(), serde_json::Value::Null);
/// ```
#[macro_export]
macro_rules! nullary {
    ($id:expr, $label:expr, $run:expr) => {
        $crate::Command {
            id: $id,
            label: $label,
            menu: None,
            submenu: None,
            group: 0,
            shortcut: None,
            icon: None,
            enabled: $crate::always,
            run: |state, _| {
                ($run)(state);
                Ok(::serde_json::Value::Null)
            },
            params: None,
            class: $crate::Class::Intent,
        }
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Unknown(String),
    Disabled {
        id: &'static str,
        reason: &'static str,
    },
    BadParams {
        id: &'static str,
        msg: String,
    },
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(id) => write!(f, "unknown command {id:?}"),
            Self::Disabled { id, reason } => write!(f, "command {id:?} is disabled: {reason}"),
            Self::BadParams { id, msg } => write!(f, "bad parameters for command {id:?}: {msg}"),
            Self::Failed(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for Error {}

/// Authorisation input. Identity is not yet part of the v1 dispatch contract.
pub struct Call<'a> {
    pub id: &'static str,
    pub origin: Origin,
    pub params: &'a Value,
}

pub type Authorize = fn(&Call<'_>) -> Result<(), &'static str>;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Described {
    pub id: &'static str,
    pub label: String,
    pub menu: Option<String>,
    pub shortcut: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub more: Vec<String>,
    pub enabled: bool,
    pub checked: Option<bool>,
    pub reason: Option<&'static str>,
    pub class: Class,
    pub schema: Option<Schema>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    Anywhere,
    Browsing,
}

#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: Option<&'static str>,
    pub submenu: Option<&'static str>,
    pub group: u8,
}

type ChoiceRun<S> = Box<dyn Fn(&mut S)>;
type Checked<S> = Box<dyn Fn(&S) -> bool>;
type Relabel<S> = Box<dyn Fn(&S) -> &'static str>;

// Function pointers stay separate from closures so the legacy shim does not
// impose a new S: 'static bound on its callers.
enum NullaryRun<S> {
    Function(fn(&mut S)),
    Choice(ChoiceRun<S>),
}

pub struct Registry<S> {
    commands: Vec<Command<S>>,
    nullary: HashMap<&'static str, NullaryRun<S>>,
    legacy_enabled: HashMap<&'static str, fn(&S) -> bool>,
    checks: HashMap<&'static str, Checked<S>>,
    scopes: HashMap<&'static str, Scope>,
    labels: HashMap<&'static str, Relabel<S>>,
    bindings: keymap::Bindings,
    journal: RefCell<Journal>,
    pub authorize: Option<Authorize>,
}

impl<S> Default for Registry<S> {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            nullary: HashMap::new(),
            legacy_enabled: HashMap::new(),
            checks: HashMap::new(),
            scopes: HashMap::new(),
            labels: HashMap::new(),
            bindings: keymap::Bindings::default(),
            journal: RefCell::new(Journal::default()),
            authorize: None,
        }
    }
}

impl<S> Registry<S> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Panics on duplicate ids or shortcuts, which would make dispatch ambiguous.
    pub fn add(&mut self, command: Command<S>) -> &mut Self {
        assert!(
            self.get(command.id).is_none(),
            "duplicate command id {:?}",
            command.id
        );
        self.bindings
            .add(command.id, command.shortcut.map(str::to_owned));
        self.commands.push(command);
        self
    }

    /// Compatibility constructor for old bool-enabled, nullary command tables.
    /// Metadata is forced to a nullary intent; execution still uses this registry.
    pub fn add_nullary(
        &mut self,
        mut command: Command<S>,
        enabled: fn(&S) -> bool,
        run: fn(&mut S),
    ) -> &mut Self {
        command.params = None;
        command.class = Class::Intent;
        let id = command.id;
        self.add(command);
        self.nullary.insert(id, NullaryRun::Function(run));
        self.legacy_enabled.insert(id, enabled);
        self
    }

    pub fn add_choice(
        &mut self,
        place: Place,
        run: impl Fn(&mut S) + 'static,
        checked: impl Fn(&S) -> bool + 'static,
    ) -> &mut Self {
        let mut command = nullary!(place.id, place.label, |_: &mut S| {});
        command.menu = place.menu;
        command.submenu = place.submenu;
        command.group = place.group;
        self.add(command);
        self.set_choice(place.id, run, checked)
    }

    /// Install the closure of a registered nullary choice (adapter support).
    pub fn set_choice(
        &mut self,
        id: &str,
        run: impl Fn(&mut S) + 'static,
        checked: impl Fn(&S) -> bool + 'static,
    ) -> &mut Self {
        let id = self.require(id);
        assert!(
            self.get(id).expect("known command").params.is_none(),
            "choice commands are nullary"
        );
        self.nullary.insert(id, NullaryRun::Choice(Box::new(run)));
        self.check(id, checked)
    }

    /// Toggle commands are nullary, like choice commands.
    pub fn add_toggle(
        &mut self,
        command: Command<S>,
        checked: impl Fn(&S) -> bool + 'static,
    ) -> &mut Self {
        assert!(command.params.is_none(), "toggle commands are nullary");
        let id = command.id;
        self.add(command);
        self.check(id, checked)
    }

    pub fn check(&mut self, id: &str, checked: impl Fn(&S) -> bool + 'static) -> &mut Self {
        let id = self.require(id);
        self.checks.insert(id, Box::new(checked));
        self
    }

    fn require(&self, id: &str) -> &'static str {
        self.get(id)
            .unwrap_or_else(|| panic!("no command {id:?}"))
            .id
    }

    pub fn get(&self, id: &str) -> Option<&Command<S>> {
        self.commands.iter().find(|c| c.id == id)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Command<S>> {
        self.commands.iter()
    }

    pub fn scope(&mut self, id: &str, scope: Scope) -> &mut Self {
        let id = self.require(id);
        self.scopes.insert(id, scope);
        self
    }
    pub fn scope_of(&self, id: &str) -> Scope {
        self.scopes.get(id).copied().unwrap_or_default()
    }

    pub fn relabel(&mut self, id: &str, label: impl Fn(&S) -> &'static str + 'static) -> &mut Self {
        let id = self.require(id);
        self.labels.insert(id, Box::new(label));
        self
    }
    pub fn label_of(&self, id: &str, state: &S) -> Option<&'static str> {
        let c = self.get(id)?;
        Some(self.labels.get(id).map_or(c.label, |label| label(state)))
    }
    pub fn checked(&self, id: &str, state: &S) -> Option<bool> {
        self.checks.get(id).map(|f| f(state))
    }

    pub fn enabled(&self, id: &str, state: &S) -> Result<(), &'static str> {
        let c = self.get(id).ok_or("unknown command")?;
        if let Some(enabled) = self.legacy_enabled.get(id) {
            if enabled(state) {
                Ok(())
            } else {
                Err("disabled")
            }
        } else {
            (c.enabled)(state)
        }
    }

    /// Set the built accelerator for a legacy adapter (before rebinding).
    pub fn set_shortcut(&mut self, id: &str, shortcut: Option<String>) {
        let id = self.require(id);
        self.bindings.set_shortcut(id, shortcut);
    }
    pub fn add_shortcuts(
        &mut self,
        id: &str,
        shortcuts: impl IntoIterator<Item = String>,
    ) -> &mut Self {
        let id = self.require(id);
        self.bindings.add_shortcuts(id, shortcuts);
        self
    }
    pub fn shortcuts_of(&self, id: &str) -> Vec<String> {
        self.bindings.shortcuts_of(id)
    }

    /// Whether the current binding came from a valid override, rather than
    /// the built defaults. Adapters use this to retain parsed default chords.
    pub fn shortcuts_overridden(&self, id: &str) -> bool {
        self.bindings.is_overridden(id)
    }

    /// The adapter supplies chord validation/canonicalisation; conflict handling,
    /// defaults and overrides are pure data shared by headless clients.
    pub fn rebind(
        &mut self,
        overrides: &keymap::Overrides,
        parse: impl Fn(&str) -> Option<String>,
    ) -> Vec<String> {
        self.bindings.rebind(overrides, parse)
    }

    pub fn execute(
        &self,
        id: &str,
        state: &mut S,
        params: Value,
        origin: Origin,
    ) -> Result<Value, Error> {
        let c = self.get(id).ok_or_else(|| Error::Unknown(id.to_owned()))?;
        if let Some(authorize) = self.authorize {
            authorize(&Call {
                id: c.id,
                origin,
                params: &params,
            })
            .map_err(|reason| Error::Disabled { id: c.id, reason })?;
        }
        self.enabled(id, state)
            .map_err(|reason| Error::Disabled { id: c.id, reason })?;
        if c.params.is_none()
            && !params.is_null()
            && !params.as_object().is_some_and(|p| p.is_empty())
        {
            return Err(Error::BadParams {
                id: c.id,
                msg: "command takes no parameters".into(),
            });
        }
        let result = match self.nullary.get(id) {
            Some(NullaryRun::Function(run)) => {
                run(state);
                Value::Null
            }
            Some(NullaryRun::Choice(run)) => {
                run(state);
                Value::Null
            }
            None => (c.run)(state, &params)?,
        };
        if c.class.journal() {
            self.journal.borrow_mut().record(c.id, origin);
        }
        Ok(result)
    }

    /// Record an accepted action outside dispatch; known queries are skipped.
    pub fn note(&self, id: &str, origin: Origin) {
        if self.get(id).is_none_or(|c| c.class.journal()) {
            self.journal.borrow_mut().record(id, origin);
        }
    }
    pub fn take_executed(&self) -> (Vec<Executed>, u64) {
        self.journal.borrow_mut().take()
    }

    pub fn describe(&self, state: &S, strings: &impl Labels) -> Vec<Described> {
        self.commands
            .iter()
            .map(|c| {
                let reason = self.enabled(c.id, state).err();
                let mut chords = self.shortcuts_of(c.id).into_iter();
                Described {
                    id: c.id,
                    label: label_text(strings, self.label_of(c.id, state).unwrap_or(c.label)),
                    menu: c.menu.map(|m| label_text(strings, m)),
                    shortcut: chords.next(),
                    more: chords.collect(),
                    enabled: reason.is_none(),
                    checked: self.checked(c.id, state),
                    reason,
                    class: c.class,
                    schema: c.params.map(|p| (p.schema)()),
                }
            })
            .collect()
    }
}
