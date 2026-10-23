// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! screenshot's commands. The menu bar, the toolbar, the shortcuts, Help
//! search and the Bus (`screenshot.commands` / `screenshot.execute`) all come
//! from this registry, so anything a person can do an agent can do by id.
//!
//! No F1–F12 key is bound, bare or with modifiers: those belong to the
//! person, and the toolkit's registry refuses one as a default.
use crate::app::{App, DELAYS, Dialog, SIZES, Tool, WIDTHS};
use crate::capture::Mode;
use annotation::Kind;
use egui::{Key, KeyboardShortcut, Modifiers};
use toolkit::command::Place;
use toolkit::{Command, Icon, Registry};

const fn key(modifiers: Modifiers, key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(modifiers, key))
}

const CTRL: Modifiers = Modifiers::COMMAND;
const CTRL_SHIFT: Modifiers = Modifiers::COMMAND.plus(Modifiers::SHIFT);
const NONE: Modifiers = Modifiers::NONE;

/// Commands that open a dialog: never in a headless instance.
pub fn needs_window(id: &str) -> bool {
    matches!(
        id,
        "file.open" | "file.save_as" | "help.about" | "help.shortcuts" | "help.macros"
    )
}

fn edit_ready(a: &App) -> Result<(), &'static str> {
    if a.editable() {
        Ok(())
    } else if a.modal() {
        Err("dialog open")
    } else if a.busy() || a.quitting {
        Err("busy")
    } else {
        Err("no image")
    }
}

fn editable(a: &App) -> bool {
    a.editable()
}

fn with_document(a: &App) -> bool {
    !a.modal() && a.document.is_some()
}

fn can_capture(a: &App) -> bool {
    a.idle() && a.connected
}

macro_rules! tools {
    ($r:ident; $( $id:literal, $label:literal, $tool:expr, $key:expr, $icon:expr; )*) => {
        $(
            $r.add_toggle(
                Command {
                    id: $id,
                    label: $label,
                    menu: Some("annotate"),
                    submenu: None,
                    group: if matches!($tool, Tool::Select | Tool::Crop) { 0 } else { 1 },
                    shortcut: key(NONE, $key),
                    icon: Some($icon),
                    enabled: with_document,
                    run: |a: &mut App| a.set_tool($tool),
                },
                |a: &App| a.ui.tool == $tool,
            );
        )*
    };
}

/// The registry.
pub fn registry() -> toolkit::command::EngineAdapter<App> {
    let mut r = Registry::new();
    r.add(toolkit::macros::command(
        |a: &App| a.ui.macros_open || (!a.headless && !a.modal()),
        |a: &mut App| a.ui.macros_open = !a.ui.macros_open,
    ));
    // File.
    r.add(Command {
        id: "file.open",
        label: "open",
        menu: Some("file"),
        submenu: None,
        group: 0,
        shortcut: key(CTRL, Key::O),
        icon: Some(Icon::FolderOpen),
        enabled: |a: &App| a.idle() && !a.headless,
        run: App::open_dialog,
    })
    .add(Command {
        id: "file.save",
        label: "save",
        menu: Some("file"),
        submenu: None,
        group: 1,
        shortcut: key(CTRL, Key::S),
        icon: Some(Icon::Save),
        enabled: editable,
        run: App::save,
    })
    .add(Command {
        id: "file.save_as",
        label: "save-as",
        menu: Some("file"),
        submenu: None,
        group: 1,
        shortcut: key(CTRL_SHIFT, Key::S),
        icon: None,
        enabled: |a: &App| a.editable() && !a.headless,
        run: App::save_as,
    })
    .add(Command {
        id: "file.copy",
        label: "copy",
        menu: Some("file"),
        submenu: None,
        group: 1,
        shortcut: key(CTRL, Key::C),
        icon: Some(Icon::Copy),
        enabled: editable,
        run: App::copy,
    })
    .add(Command {
        id: "file.quit",
        label: "quit",
        menu: Some("file"),
        submenu: None,
        group: 2,
        shortcut: key(CTRL, Key::Q),
        icon: None,
        enabled: |a: &App| !a.modal() && !a.quitting,
        run: App::quit,
    });
    // Edit.
    for mut command in annotation::commands::<App>() {
        command.enabled = match command.id {
            "edit.undo" => |a| {
                edit_ready(a)?;
                annotation::commands::can_undo(a)
            },
            "edit.redo" => |a| {
                edit_ready(a)?;
                annotation::commands::can_redo(a)
            },
            _ => edit_ready,
        };
        r.add_engine(command);
    }
    r.add(Command {
        id: "edit.delete_selected",
        label: "delete",
        menu: Some("edit"),
        submenu: None,
        group: 1,
        shortcut: key(NONE, Key::Delete),
        icon: Some(Icon::Trash),
        enabled: |a: &App| a.editable() && a.ui.selected.is_some(),
        run: App::delete_selected,
    })
    .add(Command {
        id: "edit.uncrop",
        label: "uncrop",
        menu: Some("edit"),
        submenu: None,
        group: 1,
        shortcut: None,
        icon: None,
        enabled: |a: &App| a.editable() && a.document.as_ref().is_some_and(|d| d.crop().is_some()),
        run: App::uncrop,
    })
    .add(Command {
        id: "edit.deselect",
        label: "deselect",
        menu: Some("edit"),
        submenu: None,
        group: 1,
        shortcut: None,
        icon: None,
        enabled: |a: &App| a.ui.selected.is_some(),
        run: App::deselect,
    });
    r.add_shortcuts("edit.redo", [KeyboardShortcut::new(CTRL, Key::Y)]);
    r.add_shortcuts(
        "edit.delete_selected",
        [KeyboardShortcut::new(NONE, Key::Backspace)],
    );
    // Capture.
    r.add(Command {
        id: "capture.take",
        label: "take",
        menu: Some("capture"),
        submenu: None,
        group: 0,
        shortcut: key(CTRL, Key::N),
        icon: Some(Icon::Camera),
        enabled: can_capture,
        run: |a: &mut App| a.take(None),
    })
    .add(Command {
        id: "capture.region",
        label: "take-region",
        menu: Some("capture"),
        submenu: None,
        group: 1,
        shortcut: None,
        icon: Some(Icon::SquareDashed),
        enabled: can_capture,
        run: |a: &mut App| a.take(Some(Mode::Region)),
    })
    .add(Command {
        id: "capture.screen",
        label: "take-screen",
        menu: Some("capture"),
        submenu: None,
        group: 1,
        shortcut: None,
        icon: Some(Icon::Monitor),
        enabled: can_capture,
        run: |a: &mut App| a.take(Some(Mode::Screen)),
    })
    .add(Command {
        id: "capture.window",
        label: "take-window",
        menu: Some("capture"),
        submenu: None,
        group: 1,
        shortcut: None,
        icon: Some(Icon::AppWindow),
        enabled: |a: &App| can_capture(a) && a.ui.window.is_some(),
        run: |a: &mut App| a.take(Some(Mode::Window)),
    })
    .add(Command {
        id: "capture.cancel",
        label: "cancel",
        menu: Some("capture"),
        submenu: None,
        group: 2,
        shortcut: key(NONE, Key::Escape),
        icon: Some(Icon::X),
        enabled: App::capturing,
        run: App::cancel,
    });
    for mode in Mode::ALL {
        r.add_choice(
            Place {
                id: mode_id(mode),
                label: mode_label(mode),
                menu: Some("capture"),
                submenu: Some("capture-mode"),
                group: 0,
            },
            move |a: &mut App| a.set_mode(mode),
            move |a: &App| a.ui.mode == mode,
        );
    }
    for delay in DELAYS {
        r.add_choice(
            Place {
                id: delay_id(delay),
                label: delay_label(delay),
                menu: Some("capture"),
                submenu: Some("delay"),
                group: 0,
            },
            move |a: &mut App| a.set_delay(delay),
            move |a: &App| a.ui.delay == delay,
        );
    }
    r.add_toggle(
        Command {
            id: "capture.pointer",
            label: "pointer",
            menu: Some("capture"),
            submenu: None,
            group: 4,
            shortcut: None,
            icon: None,
            enabled: |a: &App| !a.busy() && !a.modal(),
            run: App::toggle_pointer,
        },
        |a: &App| a.ui.cursor,
    )
    .add(Command {
        id: "capture.refresh",
        label: "refresh-windows",
        menu: Some("capture"),
        submenu: None,
        group: 4,
        shortcut: None,
        icon: Some(Icon::RefreshCw),
        enabled: |a: &App| a.connected,
        run: App::refresh,
    });
    // Capture › Output: the output under the pointer (compd's choice), or
    // a named one, stepped through in compd's order; the options bar lists
    // them by name.
    r.add_choice(
        Place {
            id: "capture.output.pointer",
            label: "output-pointer",
            menu: Some("capture"),
            submenu: Some("output"),
            group: 0,
        },
        |a: &mut App| a.choose_output(None),
        |a: &App| a.ui.output.is_none(),
    )
    .add(Command {
        id: "capture.output.next",
        label: "output-next",
        menu: Some("capture"),
        submenu: Some("output"),
        group: 1,
        shortcut: None,
        icon: Some(Icon::Monitor),
        enabled: |a: &App| !a.busy() && !a.modal() && a.outputs.len() > 1,
        run: App::next_output,
    });
    // Annotate: the tools, then the ink, width and text size.
    tools! { r;
        "tool.select", "select", Tool::Select, Key::V, Icon::MousePointer2;
        "tool.crop", "crop", Tool::Crop, Key::C, Icon::Crop;
        "tool.arrow", "arrow", Tool::Draw(Kind::Arrow), Key::A, Icon::MoveUpRight;
        "tool.line", "line", Tool::Draw(Kind::Line), Key::L, Icon::Minus;
        "tool.rectangle", "rectangle", Tool::Draw(Kind::Rectangle), Key::R, Icon::Square;
        "tool.ellipse", "ellipse", Tool::Draw(Kind::Ellipse), Key::E, Icon::Circle;
        "tool.pen", "pen", Tool::Draw(Kind::Pen), Key::P, Icon::Pencil;
        "tool.highlighter", "highlighter", Tool::Draw(Kind::Highlighter), Key::H, Icon::Highlighter;
        "tool.redact", "redact", Tool::Draw(Kind::Redact), Key::X, Icon::EyeOff;
        "tool.text", "text", Tool::Draw(Kind::Text), Key::T, Icon::Type;
        "tool.number", "number", Tool::Draw(Kind::Number), Key::N, Icon::Hash;
    }
    for (index, (name, label, _)) in crate::ink::INKS.into_iter().enumerate() {
        r.add_choice(
            Place {
                id: ink_id(name),
                label,
                menu: Some("annotate"),
                submenu: Some("colour"),
                group: 2,
            },
            move |a: &mut App| {
                let colour = a.inks[index].rgba;
                a.set_colour(colour);
            },
            move |a: &App| a.ui.colour == a.inks[index].rgba,
        );
    }
    for (name, label, width) in WIDTHS {
        r.add_choice(
            Place {
                id: width_id(name),
                label,
                menu: Some("annotate"),
                submenu: Some("width"),
                group: 2,
            },
            move |a: &mut App| a.set_width(width),
            move |a: &App| a.ui.width == width,
        );
    }
    for (name, label, size) in SIZES {
        r.add_choice(
            Place {
                id: size_id(name),
                label,
                menu: Some("annotate"),
                submenu: Some("text-size"),
                group: 2,
            },
            move |a: &mut App| a.set_text_size(size),
            move |a: &App| a.ui.text_size == size,
        );
    }
    // View.
    r.add(Command {
        id: "view.zoom_in",
        label: "zoom-in",
        menu: Some("view"),
        submenu: None,
        group: 0,
        shortcut: key(CTRL, Key::Plus),
        icon: Some(Icon::ZoomIn),
        enabled: with_document,
        run: |a: &mut App| a.zoom_by(1.25),
    })
    .add(Command {
        id: "view.zoom_out",
        label: "zoom-out",
        menu: Some("view"),
        submenu: None,
        group: 0,
        shortcut: key(CTRL, Key::Minus),
        icon: Some(Icon::ZoomOut),
        enabled: with_document,
        run: |a: &mut App| a.zoom_by(1.0 / 1.25),
    })
    .add(Command {
        id: "view.fit",
        label: "fit",
        menu: Some("view"),
        submenu: None,
        group: 0,
        shortcut: key(CTRL, Key::Num0),
        icon: Some(Icon::Scan),
        enabled: with_document,
        run: App::fit,
    });
    r.add_shortcuts("view.zoom_in", [KeyboardShortcut::new(CTRL, Key::Equals)]);
    // View › Theme: any scheme, style, mode and framing, in this window only.
    r.theme_menu("view", |a: &App| a.theme, |a: &mut App, c| a.theme = c);
    // Help.
    r.add(Command {
        id: "help.shortcuts",
        label: "shortcuts",
        menu: Some("help"),
        submenu: None,
        group: 0,
        shortcut: key(CTRL, Key::Slash),
        icon: Some(Icon::Keyboard),
        enabled: |a: &App| a.idle() && !a.headless,
        run: |a: &mut App| a.open(Dialog::Shortcuts),
    })
    .add(Command {
        id: "help.about",
        label: "about",
        menu: Some("help"),
        submenu: None,
        group: 1,
        shortcut: None,
        icon: None,
        enabled: |a: &App| a.idle() && !a.headless,
        run: |a: &mut App| a.open(Dialog::About),
    });
    let mut adapter = toolkit::command::EngineAdapter::new(r.into_registry());
    adapter.search_menu("help");
    adapter
}

pub const fn mode_id(mode: Mode) -> &'static str {
    match mode {
        Mode::Screen => "capture.mode.screen",
        Mode::Window => "capture.mode.window",
        Mode::Region => "capture.mode.region",
    }
}

const fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Screen => "screen",
        Mode::Window => "window",
        Mode::Region => "region",
    }
}

fn delay_id(delay: u32) -> &'static str {
    match delay {
        0 => "capture.delay.0",
        3 => "capture.delay.3",
        5 => "capture.delay.5",
        _ => "capture.delay.10",
    }
}

fn delay_label(delay: u32) -> &'static str {
    match delay {
        0 => "delay-none",
        3 => "delay-3",
        5 => "delay-5",
        _ => "delay-10",
    }
}

pub fn ink_id(name: &str) -> &'static str {
    match name {
        "red" => "annotate.colour.red",
        "amber" => "annotate.colour.amber",
        "green" => "annotate.colour.green",
        "teal" => "annotate.colour.teal",
        "black" => "annotate.colour.black",
        _ => "annotate.colour.white",
    }
}

pub fn width_id(name: &str) -> &'static str {
    match name {
        "thin" => "annotate.width.thin",
        "medium" => "annotate.width.medium",
        "thick" => "annotate.width.thick",
        _ => "annotate.width.heavy",
    }
}

pub fn size_id(name: &str) -> &'static str {
    match name {
        "small" => "annotate.size.small",
        "medium" => "annotate.size.medium",
        "large" => "annotate.size.large",
        _ => "annotate.size.huge",
    }
}

/// The command that picks `tool`.
pub fn tool_id(tool: Tool) -> &'static str {
    match tool {
        Tool::Select => "tool.select",
        Tool::Crop => "tool.crop",
        Tool::Draw(Kind::Arrow) => "tool.arrow",
        Tool::Draw(Kind::Line) => "tool.line",
        Tool::Draw(Kind::Rectangle) => "tool.rectangle",
        Tool::Draw(Kind::Ellipse) => "tool.ellipse",
        Tool::Draw(Kind::Pen) => "tool.pen",
        Tool::Draw(Kind::Highlighter) => "tool.highlighter",
        Tool::Draw(Kind::Redact) => "tool.redact",
        Tool::Draw(Kind::Text) => "tool.text",
        Tool::Draw(Kind::Number) => "tool.number",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toolkit::command::Origin;

    fn app() -> App {
        App::new(crate::label, crate::ink::inks(), false)
    }

    #[test]
    fn every_label_and_menu_is_in_the_catalogue() {
        let strings = crate::strings();
        let known = |key: &str| toolkit::command::label_text(&strings, key) != key;
        for command in registry().registry().iter() {
            assert!(
                known(command.label),
                "{} label {:?}",
                command.id,
                command.label
            );
            assert!(
                command.menu.is_none_or(|m| strings.has(m)),
                "{} menu",
                command.id
            );
            assert!(command.submenu.is_none_or(known), "{} submenu", command.id);
        }
    }

    #[test]
    fn no_function_key_is_bound() {
        let r = registry();
        let function_keys = [
            Key::F1,
            Key::F2,
            Key::F3,
            Key::F4,
            Key::F5,
            Key::F6,
            Key::F7,
            Key::F8,
            Key::F9,
            Key::F10,
            Key::F11,
            Key::F12,
        ];
        for command in r.registry().iter() {
            for shortcut in r.shortcuts_of(command.id) {
                assert!(
                    !function_keys.contains(&shortcut.logical_key),
                    "{} binds a function key",
                    command.id
                );
            }
        }
    }

    #[test]
    fn every_tool_has_its_command() {
        let r = registry();
        let mut a = app();
        a.document = Some(annotation::Document::new(image::RgbaImage::new(4, 4)).unwrap());
        for tool in Tool::ALL {
            r.execute(tool_id(tool), &mut a, serde_json::Value::Null, Origin::Bus)
                .unwrap();
            assert_eq!(a.ui.tool, tool);
            assert_eq!(r.registry().checked(tool_id(tool), &a), Some(true));
        }
    }

    #[test]
    fn choices_set_and_tick_the_ui_state() {
        let r = registry();
        let mut a = app();
        r.execute(
            "capture.mode.window",
            &mut a,
            serde_json::Value::Null,
            Origin::Bus,
        )
        .unwrap();
        r.execute(
            "capture.delay.5",
            &mut a,
            serde_json::Value::Null,
            Origin::Bus,
        )
        .unwrap();
        r.execute(
            "annotate.colour.teal",
            &mut a,
            serde_json::Value::Null,
            Origin::Bus,
        )
        .unwrap();
        r.execute(
            "annotate.width.heavy",
            &mut a,
            serde_json::Value::Null,
            Origin::Bus,
        )
        .unwrap();
        r.execute(
            "annotate.size.large",
            &mut a,
            serde_json::Value::Null,
            Origin::Bus,
        )
        .unwrap();
        r.execute(
            "capture.pointer",
            &mut a,
            serde_json::Value::Null,
            Origin::Bus,
        )
        .unwrap();
        assert_eq!(a.ui.mode, Mode::Window);
        assert_eq!(a.ui.delay, 5);
        assert_eq!(a.ui.colour, a.inks[3].rgba);
        assert_eq!(a.ui.width, 16.0);
        assert_eq!(a.ui.text_size, 40.0);
        assert!(a.ui.cursor);
        for id in [
            "capture.mode.window",
            "capture.delay.5",
            "annotate.colour.teal",
            "annotate.width.heavy",
            "annotate.size.large",
            "capture.pointer",
        ] {
            assert_eq!(r.registry().checked(id, &a), Some(true), "{id}");
        }
        assert_eq!(r.registry().checked("capture.mode.region", &a), Some(false));
    }

    #[test]
    fn a_dialog_disables_everything_but_reading() {
        let r = registry();
        let mut a = app();
        a.document = Some(annotation::Document::new(image::RgbaImage::new(4, 4)).unwrap());
        a.open(Dialog::About);
        for id in [
            "file.save",
            "file.open",
            "capture.take",
            "tool.pen",
            "edit.undo",
            "help.about",
        ] {
            assert!(
                r.execute(id, &mut a, serde_json::Value::Null, Origin::Bus)
                    .is_err(),
                "{id} must be disabled under a dialog"
            );
        }
    }
}
