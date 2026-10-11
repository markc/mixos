// SPDX-License-Identifier: MIT OR Apache-2.0
//! compd's user-visible strings, from its Fluent catalogue
//! (`services/compd/i18n/en/compd.ftl`, AGENTS.md §4). A missing key renders
//! as the key itself, so a gap shows instead of going blank.

use toolkit::strings::Strings;

const CATALOGUE: &str = include_str!("../../../i18n/en/compd.ftl");

thread_local! {
    // The compositor's UIs run on its one thread; a bundle per thread keeps
    // Fluent's memoiser out of any lock.
    static STRINGS: Strings = Strings::new(CATALOGUE);
}

/// The string for `key`.
pub fn text(key: &str) -> String {
    STRINGS.with(|strings| strings.get(key))
}

/// The string for `key` with its `{ $name }` arguments filled in.
pub fn with(key: &str, args: &[(&str, &str)]) -> String {
    STRINGS.with(|strings| strings.with(key, args))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_catalogue_parses_and_fills_arguments() {
        assert_eq!(super::text("capture-confirm"), "Confirm");
        assert_eq!(super::with("fps-value", &[("fps", "60")]), "60 FPS");
    }
}
