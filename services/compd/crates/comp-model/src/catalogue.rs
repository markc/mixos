// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The `comp.*` catalogue: 39 verbs in three families and 13 topics.

pub use crate::observation::{
    CORNER_CLICKED_TOPIC_SUFFIX, CORNER_CLICKED_V2_TOPIC_SUFFIX, CORNER_ENTERED_TOPIC_SUFFIX,
    CORNER_LEFT_TOPIC_SUFFIX, FOCUS_TOPIC_SUFFIX, OUTPUT_TOPIC_SUFFIX, PANEL_COMMAND_TOPIC_SUFFIX,
    POINTER_TOPIC_SUFFIX, PROPS_TOPIC_SUFFIX, SESSION_TOPIC_SUFFIX, SURFACE_MAPPED_TOPIC_SUFFIX,
    SURFACE_UNMAPPED_TOPIC_SUFFIX, TOPIC_SUFFIXES, topic_name,
};

/// The service name on KMS.
pub const SERVICE: &str = "comp";
/// The service name nested (`--bus-service` overrides either). Verbs stay
/// literal `comp.*` under it.
pub const NESTED_SERVICE: &str = "comp-nested";

/// noded's own props topic; its `services.registered` diffs carry the full
/// registration set, which is how the compositor sees a panel-holder
/// service leave the Bus. Only the local broker may speak on it.
pub const REGISTRY_TOPIC: &str = "noded.props.changed";

/// The read verbs answered from a snapshot.
pub const READ_VERBS: &[&str] = &[
    "comp.info",
    "comp.session.get",
    "comp.props.get",
    "comp.props.list",
    "comp.props.describe",
    "comp.windows.list",
    "comp.workspaces.list",
];

/// The 17 verbs the dispatcher answers directly (not a window or input
/// family member).
pub const DIRECT_VERBS: &[&str] = &[
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

/// The 17 window-family verbs (`{id, generation}`-addressed, plus the
/// workspace switch, count and rename).
pub const WINDOW_VERBS: &[&str] = &[
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

/// The 5 single-step input verbs (also the only verbs a sequence step may
/// name).
pub const INPUT_VERBS: &[&str] = &[
    "comp.input.pointer.move",
    "comp.input.pointer.button",
    "comp.input.pointer.scroll",
    "comp.input.key",
    "comp.input.release_all",
];

/// Which family a verb belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerbFamily {
    Direct,
    Window,
    Input,
}

/// The family of a known `comp.*` verb; `None` is `unknown_verb`.
pub fn verb_family(verb: &str) -> Option<VerbFamily> {
    if DIRECT_VERBS.contains(&verb) {
        Some(VerbFamily::Direct)
    } else if WINDOW_VERBS.contains(&verb) {
        Some(VerbFamily::Window)
    } else if INPUT_VERBS.contains(&verb) {
        Some(VerbFamily::Input)
    } else {
        None
    }
}

/// Every verb, direct first.
pub fn all_verbs() -> impl Iterator<Item = &'static str> {
    DIRECT_VERBS
        .iter()
        .chain(WINDOW_VERBS)
        .chain(INPUT_VERBS)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_surface_is_39_verbs_and_13_topics() {
        assert_eq!(DIRECT_VERBS.len(), 17);
        assert_eq!(WINDOW_VERBS.len(), 17);
        assert_eq!(INPUT_VERBS.len(), 5);
        let verbs = all_verbs().collect::<std::collections::BTreeSet<_>>();
        assert_eq!(verbs.len(), 39, "no verb is in two families");
        assert!(verbs.iter().all(|verb| verb.starts_with("comp.")));
        assert!(READ_VERBS.iter().all(|verb| DIRECT_VERBS.contains(verb)));
        assert_eq!(TOPIC_SUFFIXES.len(), 13);
        assert_eq!(
            TOPIC_SUFFIXES
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            13
        );
        assert_eq!(verb_family("comp.window.focus"), Some(VerbFamily::Window));
        assert_eq!(verb_family("comp.input.key"), Some(VerbFamily::Input));
        assert_eq!(verb_family("comp.ping"), Some(VerbFamily::Direct));
        assert_eq!(verb_family("comp.session.get"), Some(VerbFamily::Direct));
        assert_eq!(verb_family("comp-nested.panel.hold"), None);
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
