// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Theme menu every app can add ([`Registry::theme_menu`]): a "Theme"
//! submenu of choice commands that picks the window's scheme, style, mode
//! and framing directly, each group ticking its current entry.
//!
//! | group | rows | ids |
//! |---|---|---|
//! | hue schemes | Ocean, Crimson, Stone, Forest, Sunset, Mono | `view.theme.<scheme>` |
//! | chrome schemes | Pro, Studio, Classic, Adwaita, Solarized | `view.theme.<scheme>` |
//! | | Session theme (the scheme follows the session) | `view.theme.session` |
//! | modes | Light, Dark, Session mode (follows the session) | `view.mode.light`, `view.mode.dark`, `view.mode.session` |
//! | styles | Scheme's own style, Plain, Pro, Studio, Classic and GNOME style, Session style (follows the session) | `view.style.own`, `view.style.<style>`, `view.style.session` |
//! | framing | App title bar (CSD), System title bar (SSD), Captions left, Captions right | `view.decorations.csd`, `view.decorations.ssd`, `view.captions.left`, `view.captions.right` |
//!
//! The app keeps the [`Choice`], `None` on an axis following the session,
//! and installs [`crate::Theme::with_choice`] when it changes. The framing
//! group has no session row: with no choice on the axis, its defaults tick
//! (CSD, captions right). Labels come
//! from the toolkit's own catalogue.

use crate::command::{Place, Registry};
pub use crate::theme::Choice;
use design::{CaptionSide, Decorations, Mode, Scheme, Style};

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
        Scheme::Adwaita => "view.theme.adwaita",
        Scheme::Solarized => "view.theme.solarized",
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
        Scheme::Adwaita => "toolkit-theme-adwaita",
        Scheme::Solarized => "toolkit-theme-solarized",
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

/// The scheme's own style, chosen.
pub const OWN_STYLE: &str = "view.style.own";

/// Following the session's style.
pub const SESSION_STYLE: &str = "view.style.session";

/// The command id of choosing `style`.
pub const fn style_id(style: Style) -> &'static str {
    match style {
        Style::Plain => "view.style.plain",
        Style::Pro => "view.style.pro",
        Style::Studio => "view.style.studio",
        Style::Classic => "view.style.classic",
        Style::Gnome => "view.style.gnome",
    }
}

const fn style_label(style: Style) -> &'static str {
    match style {
        Style::Plain => "toolkit-style-plain",
        Style::Pro => "toolkit-style-pro",
        Style::Studio => "toolkit-style-studio",
        Style::Classic => "toolkit-style-classic",
        Style::Gnome => "toolkit-style-gnome",
    }
}

/// The command id of choosing `decorations`.
pub const fn decorations_id(decorations: Decorations) -> &'static str {
    match decorations {
        Decorations::Client => "view.decorations.csd",
        Decorations::Server => "view.decorations.ssd",
    }
}

const fn decorations_label(decorations: Decorations) -> &'static str {
    match decorations {
        Decorations::Client => "toolkit-decorations-csd",
        Decorations::Server => "toolkit-decorations-ssd",
    }
}

/// The command id of choosing the caption side.
pub const fn captions_id(side: CaptionSide) -> &'static str {
    match side {
        CaptionSide::Left => "view.captions.left",
        CaptionSide::Right => "view.captions.right",
    }
}

const fn captions_label(side: CaptionSide) -> &'static str {
    match side {
        CaptionSide::Left => "toolkit-captions-left",
        CaptionSide::Right => "toolkit-captions-right",
    }
}

impl<S: 'static> Registry<S> {
    /// Add the Theme submenu to `menu` (a Fluent menu key, usually View):
    /// `get` reads the app's choice, `set` stores one.
    pub fn theme_menu(
        &mut self,
        menu: &'static str,
        get: fn(&S) -> Choice,
        set: fn(&mut S, Choice),
    ) -> &mut Self {
        let place = |id, label, group| Place {
            id,
            label,
            menu: Some(menu),
            submenu: Some(SUBMENU),
            group,
        };
        let choose = move |s: &mut S, change: fn(&mut Choice)| {
            let mut choice = get(s);
            change(&mut choice);
            set(s, choice);
        };
        for scheme in Scheme::ALL {
            let group = if scheme.is_chrome_scheme() { 1 } else { 0 };
            self.add_choice(
                place(scheme_id(scheme), scheme_label(scheme), group),
                move |s| {
                    let mut choice = get(s);
                    choice.scheme = Some(scheme);
                    set(s, choice);
                },
                move |s| get(s).scheme == Some(scheme),
            );
        }
        self.add_choice(
            place(SESSION_SCHEME, "toolkit-theme-session", 2),
            move |s| choose(s, |c| c.scheme = None),
            move |s| get(s).scheme.is_none(),
        );
        for mode in Mode::ALL {
            self.add_choice(
                place(mode_id(mode), mode_label(mode), 3),
                move |s| {
                    let mut choice = get(s);
                    choice.mode = Some(mode);
                    set(s, choice);
                },
                move |s| get(s).mode == Some(mode),
            );
        }
        self.add_choice(
            place(SESSION_MODE, "toolkit-mode-session", 3),
            move |s| choose(s, |c| c.mode = None),
            move |s| get(s).mode.is_none(),
        );
        self.add_choice(
            place(OWN_STYLE, "toolkit-style-own", 4),
            move |s| choose(s, |c| c.style = Some(None)),
            move |s| get(s).style == Some(None),
        );
        for style in Style::ALL {
            self.add_choice(
                place(style_id(style), style_label(style), 4),
                move |s| {
                    let mut choice = get(s);
                    choice.style = Some(Some(style));
                    set(s, choice);
                },
                move |s| get(s).style == Some(Some(style)),
            );
        }
        self.add_choice(
            place(SESSION_STYLE, "toolkit-style-session", 4),
            move |s| choose(s, |c| c.style = None),
            move |s| get(s).style.is_none(),
        );
        for decorations in Decorations::ALL {
            self.add_choice(
                place(
                    decorations_id(decorations),
                    decorations_label(decorations),
                    5,
                ),
                move |s| {
                    let mut choice = get(s);
                    choice.decorations = Some(decorations);
                    set(s, choice);
                },
                move |s| get(s).decorations.unwrap_or_default() == decorations,
            );
        }
        for side in [CaptionSide::Left, CaptionSide::Right] {
            self.add_choice(
                place(captions_id(side), captions_label(side), 5),
                move |s| {
                    let mut choice = get(s);
                    choice.captions = Some(side);
                    set(s, choice);
                },
                move |s| get(s).captions.unwrap_or_default() == side,
            );
        }
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
        r.theme_menu(
            "menu-view",
            |a: &App| a.choice,
            |a, choice| a.choice = choice,
        );
        r
    }

    /// The Theme submenu's rows: (label, tick), separators as `None`.
    fn rows(r: &Registry<App>, app: &App) -> Vec<Option<(String, Option<bool>)>> {
        let strings = Strings::new("menu-view = View\n");
        let model = r.model(&egui::Context::default(), app, &strings);
        let theme = model[0].entries[0].row().expect("the Theme row");
        assert_eq!(theme.label, "Theme");
        theme
            .children
            .iter()
            .map(|e| e.row().map(|r| (r.label.clone(), r.checked)))
            .collect()
    }

    fn ticked(rows: &[Option<(String, Option<bool>)>]) -> Vec<String> {
        rows.iter()
            .flatten()
            .filter(|(_, c)| *c == Some(true))
            .map(|(l, _)| l.clone())
            .collect()
    }

    #[test]
    fn the_submenu_groups_schemes_modes_styles_and_framing_and_ticks_the_defaults() {
        let r = registry();
        let rows = rows(&r, &App::default());
        let labels: Vec<_> = rows
            .iter()
            .map(|r| r.as_ref().map(|(l, _)| l.as_str()))
            .collect();
        assert_eq!(
            labels,
            [
                Some("Ocean"),
                Some("Crimson"),
                Some("Stone"),
                Some("Forest"),
                Some("Sunset"),
                Some("Mono"),
                None,
                Some("Pro"),
                Some("Studio"),
                Some("Classic"),
                Some("Adwaita"),
                Some("Solarized"),
                None,
                Some("Session theme"),
                None,
                Some("Light"),
                Some("Dark"),
                Some("Session mode"),
                None,
                Some("Scheme's own style"),
                Some("Plain style"),
                Some("Pro style"),
                Some("Studio style"),
                Some("Classic style"),
                Some("GNOME style"),
                Some("Session style"),
                None,
                Some("App title bar (CSD)"),
                Some("System title bar (SSD)"),
                Some("Captions left"),
                Some("Captions right"),
            ]
        );
        assert!(
            rows.iter().flatten().all(|(_, c)| c.is_some()),
            "every row is a choice"
        );
        assert_eq!(
            ticked(&rows),
            [
                "Session theme",
                "Session mode",
                "Session style",
                "App title bar (CSD)",
                "Captions right"
            ]
        );
    }

    #[test]
    fn choosing_sets_one_axis_and_the_ticks_follow() {
        let r = registry();
        let mut app = App::default();
        r.execute("view.theme.forest", &mut app).unwrap();
        assert_eq!(
            app.choice,
            Choice {
                scheme: Some(Scheme::Forest),
                ..Choice::default()
            }
        );
        r.execute("view.mode.dark", &mut app).unwrap();
        r.execute("view.style.studio", &mut app).unwrap();
        r.execute("view.decorations.ssd", &mut app).unwrap();
        r.execute("view.captions.left", &mut app).unwrap();
        assert_eq!(
            app.choice,
            Choice {
                scheme: Some(Scheme::Forest),
                style: Some(Some(Style::Studio)),
                mode: Some(Mode::Dark),
                decorations: Some(Decorations::Server),
                captions: Some(CaptionSide::Left),
            },
            "each axis stays"
        );
        assert_eq!(
            ticked(&rows(&r, &app)),
            [
                "Forest",
                "Dark",
                "Studio style",
                "System title bar (SSD)",
                "Captions left"
            ]
        );
        r.execute(OWN_STYLE, &mut app).unwrap();
        assert_eq!(
            app.choice.style,
            Some(None),
            "the scheme's own style, chosen"
        );
        assert_eq!(ticked(&rows(&r, &app))[2], "Scheme's own style");
        r.execute(SESSION_SCHEME, &mut app).unwrap();
        r.execute(SESSION_MODE, &mut app).unwrap();
        assert_eq!((app.choice.scheme, app.choice.mode), (None, None));
        assert_eq!(
            app.choice.decorations,
            Some(Decorations::Server),
            "framing stays"
        );
        r.execute(SESSION_STYLE, &mut app).unwrap();
        assert_eq!(app.choice.style, None, "the session's style again");
    }

    /// After any choice, the session rows return the window to the whole
    /// session theme: a custom session design, not the embedded one.
    #[test]
    fn the_session_rows_restore_a_custom_session_theme() {
        let custom = design::EMBEDDED_DEFAULT_SOURCE.replacen(
            "\"chrome.shade.shadow\": { color_space: \"oklch\", l: 0.0, c: 0.0, h: 0.0, alpha: 0.2 }",
            "\"chrome.shade.shadow\": { color_space: \"oklch\", l: 0.0, c: 0.0, h: 0.0, alpha: 0.3 }",
            1,
        );
        let session = crate::Theme::from_source("custom", &custom).unwrap();
        let embedded = crate::Theme::for_context(session.context().clone());
        assert_ne!(
            session.dictionary(),
            embedded.dictionary(),
            "a custom design"
        );
        let r = registry();
        let mut app = App::default();
        for id in [
            "view.theme.forest",
            "view.style.pro",
            "view.mode.dark",
            "view.decorations.ssd",
            "view.captions.left",
        ] {
            r.execute(id, &mut app).unwrap();
        }
        assert_ne!(
            session.with_choice(&app.choice).dictionary(),
            session.dictionary()
        );
        for id in [
            SESSION_SCHEME,
            SESSION_STYLE,
            SESSION_MODE,
            "view.decorations.csd",
            "view.captions.right",
        ] {
            r.execute(id, &mut app).unwrap();
        }
        assert_eq!(app.choice.style, None);
        let back = session.with_choice(&app.choice);
        assert_eq!(
            back.dictionary(),
            session.dictionary(),
            "the custom session design itself"
        );
        assert_eq!(
            (back.decorations(), back.captions()),
            (Decorations::Client, CaptionSide::Right)
        );
    }

    #[test]
    fn choices_describe_with_ticks_and_are_found_by_search() {
        let r = registry();
        let strings = Strings::new("menu-view = View\n");
        let app = App {
            choice: Choice {
                scheme: Some(Scheme::Pro),
                mode: Some(Mode::Light),
                ..Choice::default()
            },
        };
        let described = r.describe(&app, &strings);
        let pro = described.iter().find(|d| d.id == "view.theme.pro").unwrap();
        assert_eq!(
            (pro.label.as_str(), pro.checked, pro.menu.as_deref()),
            ("Pro", Some(true), Some("View"))
        );
        assert_eq!(
            described
                .iter()
                .find(|d| d.id == "view.theme.ocean")
                .unwrap()
                .checked,
            Some(false)
        );
        let found: Vec<_> = r
            .search("stud", &app, &strings)
            .iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(found, ["view.theme.studio", "view.style.studio"]);
        let model = r.model(&egui::Context::default(), &app, &strings);
        let results = crate::menu::matching(&model, "forest");
        assert_eq!(results[0].label, "View › Theme › Forest");
        assert_eq!(results[0].checked, Some(false));
        let results = crate::menu::matching(&model, "ssd");
        assert_eq!(results[0].label, "View › Theme › System title bar (SSD)");
    }
}
