// SPDX-License-Identifier: MIT OR Apache-2.0
//! The shared fixture of the title-bar, menu and chrome tests: a desktop
//! editor's menu bar (ten menus; File with a disabled row, a submenu and
//! groups), drawn by the real title bar, with every chosen command recorded.
//! Included by path from each test crate that uses it.

#![allow(dead_code, reason = "each test crate uses part of the fixture")]

use design::{DesignContext, Mode, Scheme};
use egui::{Key, KeyboardShortcut, Modifiers};
use egui_kittest::{Harness, HarnessBuilder};
use toolkit::command::{Command, Registry, always};
use toolkit::{Icon, Strings, Theme, titlebar};

pub const FTL: &str = "\
menu-file = File
menu-edit = Edit
menu-image = Image
menu-layer = Layer
menu-type = Type
menu-select = Select
menu-filter = Filter
menu-view = View
menu-window = Window
menu-help = Help
recent = Open Recent
new = New…
new-clipboard = New from Clipboard
open = Open…
open-as = Open As…
clear-recent = Clear Recent Files
close = Close
close-all = Close All
close-others = Close Others
save = Save
save-as = Save As…
undo = Undo
redo = Redo
size = Image Size…
duplicate = Duplicate Layer
text = Text Options
all = Select All
blur = Blur
zoom = Zoom In
tile = Tile Windows
about = About
";

/// The title the fixture's window shows.
pub const TITLE: &str = "Untitled";

/// What the fixture's commands did.
#[derive(Default)]
pub struct Fixture {
    pub ran: Vec<&'static str>,
}

fn never(_: &Fixture) -> bool {
    false
}

const fn key(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

const CTRL: Modifiers = Modifiers::COMMAND;
const CTRL_ALT: Modifiers = Modifiers::COMMAND.plus(Modifiers::ALT);

macro_rules! commands {
    ($registry:ident: $($id:literal $label:literal $menu:literal $submenu:expr, $group:literal, $shortcut:expr, $enabled:expr;)+) => {
        $($registry.add(Command {
            id: $id,
            label: $label,
            menu: Some($menu),
            submenu: $submenu,
            group: $group,
            shortcut: $shortcut,
            icon: None,
            enabled: $enabled,
            run: |s: &mut Fixture| s.ran.push($id),
        });)+
    };
}

pub fn registry() -> Registry<Fixture> {
    let mut r = Registry::new();
    commands! { r:
        "file.new" "new" "menu-file" None, 0, key(CTRL, Key::N), always;
        "file.new-clipboard" "new-clipboard" "menu-file" None, 0, None, never;
        "file.open" "open" "menu-file" None, 0, key(CTRL, Key::O), always;
        "file.open-as" "open-as" "menu-file" None, 0, key(CTRL_ALT.plus(Modifiers::SHIFT), Key::O), always;
        "file.clear-recent" "clear-recent" "menu-file" Some("recent"), 0, None, always;
        "file.close" "close" "menu-file" None, 1, key(CTRL, Key::W), always;
        "file.close-all" "close-all" "menu-file" None, 1, key(CTRL_ALT, Key::W), always;
        "file.close-others" "close-others" "menu-file" None, 1, key(CTRL_ALT, Key::P), always;
        "file.save" "save" "menu-file" None, 2, key(CTRL, Key::S), always;
        "file.save-as" "save-as" "menu-file" None, 2, key(CTRL.plus(Modifiers::SHIFT), Key::S), always;
        "edit.undo" "undo" "menu-edit" None, 0, key(CTRL, Key::Z), always;
        "edit.redo" "redo" "menu-edit" None, 0, key(CTRL.plus(Modifiers::SHIFT), Key::Z), never;
        "image.size" "size" "menu-image" None, 0, None, always;
        "layer.duplicate" "duplicate" "menu-layer" None, 0, None, always;
        "type.options" "text" "menu-type" None, 0, None, always;
        "select.all" "all" "menu-select" None, 0, key(CTRL, Key::A), always;
        "filter.blur" "blur" "menu-filter" None, 0, None, always;
        "view.zoom-in" "zoom" "menu-view" None, 0, None, always;
        "window.tile" "tile" "menu-window" None, 0, None, always;
        "help.about" "about" "menu-help" None, 0, None, always;
    }
    r
}

/// The theme for one chrome scheme and mode.
pub fn theme(scheme: Scheme, mode: Mode) -> Theme {
    Theme::for_context(DesignContext { scheme, mode, ..DesignContext::default() })
}

/// The five chrome themes of the chrome specification, by its theme id.
pub const CHROME_THEMES: [(&str, Scheme, Mode); 5] = [
    ("pro", Scheme::Pro, Mode::Dark),
    ("proMedium", Scheme::Pro, Mode::Light),
    ("studio", Scheme::Studio, Mode::Dark),
    ("studioLight", Scheme::Studio, Mode::Light),
    ("classic", Scheme::Classic, Mode::Light),
];

/// A window of the fixture: global shortcuts, then the title bar, then an
/// empty body and the resize edges, every chosen command executed.
pub fn harness(builder: HarnessBuilder<Fixture>, theme: &Theme) -> Harness<'static, Fixture> {
    let registry = registry();
    let strings = Strings::new(FTL);
    let stroke = toolkit::icons::stroke_width(theme);
    let mut harness = builder.build_ui_state(
        move |ui, state: &mut Fixture| {
            let mut fired = registry.shortcuts(ui.ctx(), state);
            fired.extend(titlebar::show(ui, TITLE, Some(Icon::Square), stroke, &registry, state, &strings));
            egui::CentralPanel::default().show(ui, |_| {});
            titlebar::edges(ui);
            for id in fired {
                let _ = registry.execute(id, state);
            }
        },
        Fixture::default(),
    );
    toolkit::install(&harness.ctx, theme);
    harness.run();
    harness
}
