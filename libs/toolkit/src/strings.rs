// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fluent strings, re-exported from the UI-free command crate.
pub use ::action::Strings;

/// The toolkit's own catalogue: the strings its widgets show.
const TOOLKIT: &str = include_str!("../i18n/en/toolkit.ftl");

thread_local! {
    static OWN: Strings = Strings::new(TOOLKIT);
}

/// The toolkit message `key`.
pub(crate) fn own(key: &str) -> String {
    OWN.with(|s| s.get(key))
}

/// The toolkit message `key` with named arguments.
pub(crate) fn own_with(key: &str, args: &[(&str, &str)]) -> String {
    OWN.with(|s| s.with(key, args))
}
