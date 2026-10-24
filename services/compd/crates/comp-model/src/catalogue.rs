// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The `comp.*` catalogue: 39 commands in three families and 13 topics.

pub use crate::observation::{
    CORNER_CLICKED_TOPIC_SUFFIX, CORNER_CLICKED_V2_TOPIC_SUFFIX, CORNER_ENTERED_TOPIC_SUFFIX,
    CORNER_LEFT_TOPIC_SUFFIX, FOCUS_TOPIC_SUFFIX, OUTPUT_TOPIC_SUFFIX, PANEL_COMMAND_TOPIC_SUFFIX,
    POINTER_TOPIC_SUFFIX, PROPS_TOPIC_SUFFIX, SESSION_TOPIC_SUFFIX, SURFACE_MAPPED_TOPIC_SUFFIX,
    SURFACE_UNMAPPED_TOPIC_SUFFIX, TOPIC_SUFFIXES, topic_name,
};

/// The service name on KMS.
pub const SERVICE: &str = "comp";
/// The service name nested (`--bus-service` overrides either). Commands stay
/// literal `comp.*` under it.
pub const NESTED_SERVICE: &str = "comp-nested";
/// The service name for the render-node-only virtual output.
pub const HEADLESS_SERVICE: &str = "comp-headless";

/// noded's own props topic; its `services.registered` diffs carry the full
/// registration set, which is how the compositor sees a panel-holder
/// service leave the Bus. Only the local broker may speak on it.
pub const REGISTRY_TOPIC: &str = "noded.props.changed";

/// The read commands answered from a snapshot.
pub const READ_COMMANDS: &[&str] = &[
    "comp.info",
    "comp.session.get",
    "comp.props.get",
    "comp.props.list",
    "comp.props.describe",
    "comp.windows.list",
    "comp.workspaces.list",
];

/// The 17 commands the dispatcher answers directly (not a window or input
/// family member).
pub const DIRECT_COMMANDS: &[&str] = &[
    "comp.capture.frame",
    "comp.ping",
    "comp.props.watch",
    "comp.pointer.watch",
    "comp.props.set",
    "comp.region.select",
    "comp.input.sequence",
    "comp.motion.run",
    "comp.panel.hold",
    "comp.panel.mode",
    "comp.info",
    "comp.session.get",
    "comp.props.get",
    "comp.props.list",
    "comp.props.describe",
    "comp.windows.list",
    "comp.workspaces.list",
];

/// The 17 window-family commands (`{id, generation}`-addressed, plus the
/// workspace switch, count and rename).
pub const WINDOW_COMMANDS: &[&str] = &[
    "comp.window.maximize",
    "comp.window.unmaximize",
    "comp.window.fullscreen",
    "comp.window.unfullscreen",
    "comp.window.minimize",
    "comp.window.restore",
    "comp.window.focus",
    "comp.window.raise",
    "comp.window.close",
    "comp.window.place",
    "comp.window.wait",
    "comp.window.stats",
    "comp.window.stats.reset",
    "comp.workspace.switch",
    "comp.window.send_to_workspace",
    "comp.workspaces.set_count",
    "comp.workspace.rename",
];

/// The 5 single-step input commands (also the only commands a sequence step may
/// name).
pub const INPUT_COMMANDS: &[&str] = &[
    "comp.input.pointer.move",
    "comp.input.pointer.button",
    "comp.input.pointer.scroll",
    "comp.input.key",
    "comp.input.release_all",
];

/// Which family a command belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandFamily {
    Direct,
    Window,
    Input,
}

/// The family of a known `comp.*` command; `None` is `unknown_command`.
pub fn command_family(command: &str) -> Option<CommandFamily> {
    if DIRECT_COMMANDS.contains(&command) {
        Some(CommandFamily::Direct)
    } else if WINDOW_COMMANDS.contains(&command) {
        Some(CommandFamily::Window)
    } else if INPUT_COMMANDS.contains(&command) {
        Some(CommandFamily::Input)
    } else {
        None
    }
}

/// Every command, direct first.
pub fn all_commands() -> impl Iterator<Item = &'static str> {
    DIRECT_COMMANDS
        .iter()
        .chain(WINDOW_COMMANDS)
        .chain(INPUT_COMMANDS)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_surface_is_39_commands_and_13_topics() {
        assert_eq!(DIRECT_COMMANDS.len(), 17);
        assert_eq!(WINDOW_COMMANDS.len(), 17);
        assert_eq!(INPUT_COMMANDS.len(), 5);
        let commands = all_commands().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(commands.len(), 39, "no command is in two families");
        assert!(commands.iter().all(|command| command.starts_with("comp.")));
        assert!(READ_COMMANDS.iter().all(|command| DIRECT_COMMANDS.contains(command)));
        assert_eq!(TOPIC_SUFFIXES.len(), 13);
        assert_eq!(
            TOPIC_SUFFIXES
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            13
        );
        assert_eq!(command_family("comp.window.focus"), Some(CommandFamily::Window));
        assert_eq!(command_family("comp.input.key"), Some(CommandFamily::Input));
        assert_eq!(command_family("comp.ping"), Some(CommandFamily::Direct));
        assert_eq!(command_family("comp.session.get"), Some(CommandFamily::Direct));
        assert_eq!(command_family("comp-nested.panel.hold"), None);
        assert_eq!(
            topic_name(NESTED_SERVICE, FOCUS_TOPIC_SUFFIX),
            "comp-nested.focus.changed"
        );
        assert_eq!(
            topic_name(SERVICE, PROPS_TOPIC_SUFFIX),
            "comp.props.changed"
        );
    }
}
