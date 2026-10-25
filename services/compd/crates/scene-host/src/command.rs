// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The host's Bus commands.

use crate::panels::PanelCommand;

/// One `shell.scene.*` request (Scene Editor plan §4.3; Quoin's command set),
/// plus the `shell.scenes.list` inventory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneCommand {
    Load,
    Validate,
    Patch,
    Get,
    Describe,
    Unload,
    Watch,
    /// Measured from the renderer, not the store.
    Layout,
    List,
    /// Map / unmap the dialog seat (Scene Editor plan §4.3 Q2).
    DialogShow,
    DialogHide,
    /// Map / unmap the pointer-only toast seat.
    ToastShow,
    ToastHide,
    /// `shell.ping`: `{service, status: "ok"}`, as Quoin answers it (the
    /// liveness probe loaders and gates wait on).
    Ping,
    /// `shell.info`: the contract and the commands this host serves.
    Info,
    /// `shell.props.get`: `{dialog, panels}` or one path of it.
    PropsGet,
    /// `shell.panel.state {edge}` (Quoin's panel model).
    PanelState,
    /// `shell.panel.mode {edge, mode}`.
    PanelMode,
    /// `shell.panel.page.set {edge, id}`.
    PanelPageSet,
    /// `shell.panel.order {edges}`: conf.mix's declared page order (N2b).
    PanelOrder,
    /// `shell.panel.resize {edge, thickness_px}` (N2b).
    PanelResize,
    /// Agent equivalents of the RMB menu: open {corner}, choose {serial,index}, close {serial}.
    MenuOpen,
    MenuChoose,
    MenuClose,
    /// `shell.panel.{show,hide,toggle,pin,unpin,dock} {edge}`, or with
    /// `corner` set `shell.corner.{show,hide,toggle,pin,unpin} {corner}` on
    /// the edge the corner summons (Quoin: top-left → left, …).
    Panel {
        kind: crate::panels::PanelCommand,
        corner: bool,
    },
    /// A Quoin shell command this host does not serve yet (the settings commands):
    /// refused with `UNKNOWN_COMMAND`, never a silent no-op, and never listed
    /// by `shell.info`.
    NotServed,
}

impl SceneCommand {
    /// Stable protocol discovery; watch and validate are registry queries too.
    pub fn manifest(service: &str) -> Vec<bus::CommandDescriptor> {
        use bus::CommandDescriptor;
        [
            CommandDescriptor::new("HELP", &[], "List every command", true),
            CommandDescriptor::new("app.describe", &[], "Describe the service commands", true),
        ]
        .into_iter()
        .chain(Self::names(service).into_iter().map(|name| {
            let kind = Self::parse(service, &name).expect("catalogued scene command");
            CommandDescriptor::new(&name, &[], &format!("Scene host {}", name.split_once('.').unwrap().1), !kind.mutates())
        }))
        .collect()
    }

    /// The command for a request's `command`: `shell.<command>` (the contract's
    /// spelling, which loaders send whatever the host registered as), or
    /// `<service>.<command>` under an override name.
    pub fn parse(service: &str, command: &str) -> Option<Self> {
        fn under<'a>(command: &'a str, prefix: &str) -> Option<&'a str> {
            command.strip_prefix(prefix)?.strip_prefix('.')
        }
        let kind = under(command, "shell").or_else(|| under(command, service))?;
        Some(match kind {
            "scene.load" => Self::Load,
            "scene.validate" => Self::Validate,
            "scene.patch" => Self::Patch,
            "scene.get" => Self::Get,
            "scene.describe" => Self::Describe,
            "scene.unload" => Self::Unload,
            "scene.watch" => Self::Watch,
            "scene.layout" => Self::Layout,
            "scenes.list" => Self::List,
            "dialog.show" => Self::DialogShow,
            "dialog.hide" => Self::DialogHide,
            "toast.show" => Self::ToastShow,
            "toast.hide" => Self::ToastHide,
            "ping" => Self::Ping,
            "info" => Self::Info,
            "props.get" => Self::PropsGet,
            "panel.state" => Self::PanelState,
            "panel.mode" => Self::PanelMode,
            "panel.page.set" => Self::PanelPageSet,
            "panel.order" => Self::PanelOrder,
            "panel.resize" => Self::PanelResize,
            "corner.menu.open" => Self::MenuOpen,
            "corner.menu.choose" => Self::MenuChoose,
            "corner.menu.close" => Self::MenuClose,
            "panel.show" => Self::Panel {
                kind: PanelCommand::Show,
                corner: false,
            },
            "panel.hide" => Self::Panel {
                kind: PanelCommand::Hide,
                corner: false,
            },
            "panel.toggle" => Self::Panel {
                kind: PanelCommand::Toggle,
                corner: false,
            },
            "panel.pin" => Self::Panel {
                kind: PanelCommand::Pin,
                corner: false,
            },
            "panel.unpin" => Self::Panel {
                kind: PanelCommand::Unpin,
                corner: false,
            },
            "panel.dock" => Self::Panel {
                kind: PanelCommand::Dock,
                corner: false,
            },
            "corner.show" => Self::Panel {
                kind: PanelCommand::Show,
                corner: true,
            },
            "corner.hide" => Self::Panel {
                kind: PanelCommand::Hide,
                corner: true,
            },
            "corner.toggle" => Self::Panel {
                kind: PanelCommand::Toggle,
                corner: true,
            },
            "corner.pin" => Self::Panel {
                kind: PanelCommand::Pin,
                corner: true,
            },
            "corner.unpin" => Self::Panel {
                kind: PanelCommand::Unpin,
                corner: true,
            },
            settings if settings.starts_with("settings.") => Self::NotServed,
            _ => return None,
        })
    }

    /// Commands that can change what is mounted or drawn.
    pub fn mutates(self) -> bool {
        matches!(
            self,
            Self::Load
                | Self::Patch
                | Self::Unload
                | Self::DialogShow
                | Self::DialogHide
                | Self::ToastShow
                | Self::ToastHide
                | Self::PanelMode
                | Self::PanelPageSet
                | Self::PanelOrder
                | Self::PanelResize
                | Self::MenuOpen
                | Self::MenuChoose
                | Self::MenuClose
                | Self::Panel { .. }
        )
    }

    /// Every command name under `service`, for HELP and the tests.
    pub fn names(service: &str) -> Vec<String> {
        [
            "scene.load",
            "scene.validate",
            "scene.patch",
            "scene.get",
            "scene.describe",
            "scene.unload",
            "scene.watch",
            "scene.layout",
            "scenes.list",
            "dialog.show",
            "dialog.hide",
            "toast.show",
            "toast.hide",
            "ping",
            "info",
            "props.get",
            "panel.state",
            "panel.mode",
            "panel.page.set",
            "panel.order",
            "panel.resize",
            "panel.show",
            "panel.hide",
            "panel.toggle",
            "panel.pin",
            "panel.unpin",
            "panel.dock",
            "corner.show",
            "corner.hide",
            "corner.toggle",
            "corner.pin",
            "corner.unpin",
            "corner.menu.open",
            "corner.menu.choose",
            "corner.menu.close",
        ]
        .into_iter()
        .map(|kind| format!("{service}.{kind}"))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse_as_shell_or_under_the_registered_service() {
        for name in SceneCommand::names("shell") {
            assert!(SceneCommand::parse("shell", &name).is_some(), "{name}");
            assert!(
                SceneCommand::parse("compd-scenes", &name).is_some(),
                "{name}"
            );
        }
        for name in SceneCommand::names("compd-scenes") {
            assert!(
                SceneCommand::parse("compd-scenes", &name).is_some(),
                "{name}"
            );
            assert!(SceneCommand::parse("shell", &name).is_none(), "{name}");
        }
        assert_eq!(SceneCommand::parse("shell", "shellx.scene.load"), None);
        assert_eq!(SceneCommand::parse("shell", "shell.panel.nope"), None);
        for kind in ["shell.settings.get", "shell.settings.scheme"] {
            assert_eq!(
                SceneCommand::parse("shell", kind),
                Some(SceneCommand::NotServed),
                "{kind}"
            );
        }
        assert_eq!(
            SceneCommand::parse("shell", "shell.panel.order"),
            Some(SceneCommand::PanelOrder)
        );
        assert_eq!(
            SceneCommand::parse("shell", "shell.corner.show"),
            Some(SceneCommand::Panel {
                kind: PanelCommand::Show,
                corner: true
            })
        );
        assert_eq!(
            SceneCommand::parse("shell", "shell.panel.unpin"),
            Some(SceneCommand::Panel {
                kind: PanelCommand::Unpin,
                corner: false
            })
        );
        assert!(
            !SceneCommand::names("shell")
                .iter()
                .any(|name| name.contains("settings")),
            "shell.info lists only what is served"
        );
    }
}
