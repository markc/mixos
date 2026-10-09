// SPDX-License-Identifier: MIT OR Apache-2.0
//! User-visible strings from a Fluent catalogue (AGENTS.md §4: no hard-coded
//! UI strings). An application embeds `i18n/en/<app>.ftl` and looks keys up
//! through [`Strings`]; a missing key renders as the key itself, so a gap is
//! visible rather than blank.

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};

/// One loaded catalogue.
pub struct Strings {
    bundle: FluentBundle<FluentResource>,
}

impl Strings {
    /// The English catalogue `source`.
    ///
    /// # Panics
    /// On a catalogue that does not parse or repeats a key: catalogues are
    /// embedded, so that is a build defect.
    pub fn new(source: &str) -> Self {
        let resource = FluentResource::try_new(source.to_owned()).expect("valid Fluent catalogue");
        let mut bundle = FluentBundle::new(vec!["en".parse().expect("locale")]);
        // Isolation marks would leak into egui labels as visible glyphs.
        bundle.set_use_isolating(false);
        bundle.add_resource(resource).expect("unique catalogue keys");
        Self { bundle }
    }

    /// The message `key`, or `key` itself when the catalogue lacks it.
    pub fn get(&self, key: &str) -> String {
        self.format(key, None)
    }

    /// The message `key` with named arguments.
    pub fn with(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut fluent = FluentArgs::new();
        for (name, value) in args {
            fluent.set(*name, FluentValue::from(*value));
        }
        self.format(key, Some(&fluent))
    }

    fn format(&self, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        let Some(pattern) = self.bundle.get_message(key).and_then(|m| m.value()) else {
            return key.to_owned();
        };
        let mut errors = Vec::new();
        self.bundle.format_pattern(pattern, args, &mut errors).into_owned()
    }

    /// Whether the catalogue defines `key`.
    pub fn has(&self, key: &str) -> bool {
        self.bundle.has_message(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_formats_and_falls_back_to_the_key() {
        let s = Strings::new("hello = Hello\ngreet = Hello, { $name }!\n");
        assert_eq!(s.get("hello"), "Hello");
        assert_eq!(s.with("greet", &[("name", "Mix")]), "Hello, Mix!");
        assert_eq!(s.get("missing"), "missing");
        assert!(s.has("hello") && !s.has("missing"));
    }
}
