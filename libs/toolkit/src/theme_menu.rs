// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Theme menu every app can add ([`Registry::theme_menu`]): a "Theme"
//! submenu of choice commands that picks the window's scheme and mode
//! directly, each group ticking its current entry.
//!
//! | group | rows | ids |
//! |---|---|---|
//! | hue schemes | Ocean, Crimson, Stone, Forest, Sunset, Mono | `view.theme.<scheme>` |
//! | chrome schemes | Pro, Studio, Classic | `view.theme.<scheme>` |
//! | | Session theme (the scheme follows the session) | `view.theme.session` |
//! | modes | Light, Dark, Session mode (follows the session) | `view.mode.light`, `view.mode.dark`, `view.mode.session` |
//!
//! The app keeps the choice, `(Option<Scheme>, Option<Mode>)` with `None`
//! following the session, and installs [`crate::Theme::with_choice`] when it
//! changes. Labels come from the toolkit's own catalogue.

use crate::command::{Place, Registry};
use design::{Mode, Scheme};

/// The submenu's label key.
pub const SUBMENU: &str = "toolkit-theme";

/// The command id of choosing `scheme`.
pub const fn scheme_id(scheme: Scheme) -> &'static str {
    match scheme {
        Scheme::Ocean => "view.theme.ocean",
        Scheme::Crimson => "view.theme.crimson",
        Scheme::Stone => "view.theme.stone",
        Scheme::Forest => "view.theme.forest",
        Scheme::Sunset => "view.theme.sunset",
        Scheme::Mono => "view.theme.mono",
        Scheme::Pro => "view.theme.pro",
        Scheme::Studio => "view.theme.studio",
        Scheme::Classic => "view.theme.classic",
    }
}

const fn scheme_label(scheme: Scheme) -> &'static str {
    match scheme {
        Scheme::Ocean => "toolkit-theme-ocean",
        Scheme::Crimson => "toolkit-theme-crimson",
        Scheme::Stone => "toolkit-theme-stone",
        Scheme::Forest => "toolkit-theme-forest",
        Scheme::Sunset => "toolkit-theme-sunset",
        Scheme::Mono => "toolkit-theme-mono",
        Scheme::Pro => "toolkit-theme-pro",
        Scheme::Studio => "toolkit-theme-studio",
        Scheme::Classic => "toolkit-theme-classic",
    }
}

/// The command id of choosing `mode`.
pub const fn mode_id(mode: Mode) -> &'static str {
    match mode {
        Mode::Light => "view.mode.light",
        Mode::Dark => "view.mode.dark",
    }
}

const fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Light => "toolkit-mode-light",
        Mode::Dark => "toolkit-mode-dark",
    }
}

/// Following the session's scheme, and its mode.
pub const SESSION_SCHEME: &str = "view.theme.session";
pub const SESSION_MODE: &str = "view.mode.session";

/// The app's theme choice: `None` on an axis follows the session.
pub type Choice = (Option<Scheme>, Option<Mode>);

impl<S: 'static> Registry<S> {
    /// Add the Theme submenu to `menu` (a Fluent menu key, usually View):
    /// `get` reads the app's choice, `set` stores one.
    pub fn theme_menu(&mut self, menu: &'static str, get: fn(&S) -> Choice, set: fn(&mut S, Option<Scheme>, Option<Mode>)) -> &mut Self {
        let place = |id, label, group| Place { id, label, menu: Some(menu), submenu: Some(SUBMENU), group };
        for scheme in Scheme::ALL {
            let group = if scheme.is_chrome_scheme() { 1 } else { 0 };
            self.add_choice(
                place(scheme_id(scheme), scheme_label(scheme), group),
                move |s| {
                    let (_, mode) = get(s);
                    set(s, Some(scheme), mode);
                },
                move |s| get(s).0 == Some(scheme),
            );
        }
        self.add_choice(
            place(SESSION_SCHEME, "toolkit-theme-session", 2),
            move |s| {
                let (_, mode) = get(s);
                set(s, None, mode);
            },
            move |s| get(s).0.is_none(),
        );
        for mode in Mode::ALL {
            self.add_choice(
                place(mode_id(mode), mode_label(mode), 3),
                move |s| {
                    let (scheme, _) = get(s);
                    set(s, scheme, Some(mode));
                },
                move |s| get(s).1 == Some(mode),
            );
        }
        self.add_choice(
            place(SESSION_MODE, "toolkit-mode-session", 3),
            move |s| {
                let (scheme, _) = get(s);
                set(s, scheme, None);
            },
            move |s| get(s).1.is_none(),
        );
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::Strings;

    #[derive(Default)]
    struct App {
        choice: Choice,
    }

    fn registry() -> Registry<App> {
        let mut r = Registry::new();
        r.theme_menu("menu-view", |a: &App| a.choice, |a, scheme, mode| a.choice = (scheme, mode));
        r
    }

    /// The Theme submenu's rows: (label, tick), separators as `None`.
    fn rows(r: &Registry<App>, app: &App) -> Vec<Option<(String, Option<bool>)>> {
        let strings = Strings::new("menu-view = View\n");
        let model = r.model(&egui::Context::default(), app, &strings);
        let theme = model[0].entries[0].row().expect("the Theme row");
        assert_eq!(theme.label, "Theme");
        theme.children.iter().map(|e| e.row().map(|r| (r.label.clone(), r.checked))).collect()
    }

    fn ticked(rows: &[Option<(String, Option<bool>)>]) -> Vec<String> {
        rows.iter().flatten().filter(|(_, c)| *c == Some(true)).map(|(l, _)| l.clone()).collect()
    }

    #[test]
    fn the_submenu_groups_schemes_session_and_modes_and_ticks_the_session_by_default() {
        let r = registry();
        let rows = rows(&r, &App::default());
        let labels: Vec<_> = rows.iter().map(|r| r.as_ref().map(|(l, _)| l.as_str())).collect();
        assert_eq!(
            labels,
            [
                Some("Ocean"), Some("Crimson"), Some("Stone"), Some("Forest"), Some("Sunset"), Some("Mono"), None,
                Some("Pro"), Some("Studio"), Some("Classic"), None,
                Some("Session theme"), None,
                Some("Light"), Some("Dark"), Some("Session mode"),
            ]
        );
        assert!(rows.iter().flatten().all(|(_, c)| c.is_some()), "every row is a choice");
        assert_eq!(ticked(&rows), ["Session theme", "Session mode"]);
    }

    #[test]
    fn choosing_sets_one_axis_and_the_ticks_follow() {
        let r = registry();
        let mut app = App::default();
        r.execute("view.theme.forest", &mut app).unwrap();
        assert_eq!(app.choice, (Some(Scheme::Forest), None));
        r.execute("view.mode.dark", &mut app).unwrap();
        assert_eq!(app.choice, (Some(Scheme::Forest), Some(Mode::Dark)), "the scheme stays");
        assert_eq!(ticked(&rows(&r, &app)), ["Forest", "Dark"]);
        r.execute(SESSION_SCHEME, &mut app).unwrap();
        assert_eq!(app.choice, (None, Some(Mode::Dark)), "the mode stays");
        r.execute(SESSION_MODE, &mut app).unwrap();
        assert_eq!(app.choice, (None, None));
    }

    #[test]
    fn choices_describe_with_ticks_and_are_found_by_search() {
        let r = registry();
        let strings = Strings::new("menu-view = View\n");
        let app = App { choice: (Some(Scheme::Pro), Some(Mode::Light)) };
        let described = r.describe(&app, &strings);
        let pro = described.iter().find(|d| d.id == "view.theme.pro").unwrap();
        assert_eq!((pro.label.as_str(), pro.checked, pro.menu.as_deref()), ("Pro", Some(true), Some("View")));
        assert_eq!(described.iter().find(|d| d.id == "view.theme.ocean").unwrap().checked, Some(false));
        let found: Vec<_> = r.search("stud", &app, &strings).iter().map(|c| c.id).collect();
        assert_eq!(found, ["view.theme.studio"]);
        let model = r.model(&egui::Context::default(), &app, &strings);
        let results = crate::menu::matching(&model, "forest");
        assert_eq!(results[0].label, "View › Theme › Forest");
        assert_eq!(results[0].checked, Some(false));
    }
}
