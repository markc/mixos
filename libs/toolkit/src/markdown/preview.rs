// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! One parsed document, kept until its caller's key or revision changes.

use std::sync::Arc;

use super::Document;

/// A Markdown document cached by a caller's key and revision. No UI state
/// is kept here; clones share the parsed document.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview<K> {
    cached: Option<(K, u64, Arc<Document>)>,
}

impl<K> Default for Preview<K> {
    fn default() -> Self {
        Self { cached: None }
    }
}

impl<K> Preview<K> {
    /// The last parsed document, without calling the source or parsing.
    pub fn cached(&self) -> Option<&Arc<Document>> {
        self.cached.as_ref().map(|(_, _, document)| document)
    }

    /// Forget the key, revision and document.
    pub fn clear(&mut self) {
        self.cached = None;
    }
}

impl<K: PartialEq> Preview<K> {
    /// Return the document for `key` and `revision`. Call `source` and
    /// parse only when either changes, or the cache is empty.
    pub fn document<'a>(
        &mut self,
        key: K,
        revision: u64,
        source: impl FnOnce() -> &'a str,
    ) -> Arc<Document> {
        if let Some((cached_key, cached_revision, document)) = &self.cached
            && *cached_key == key
            && *cached_revision == revision
        {
            return Arc::clone(document);
        }
        let document = Arc::new(Document::parse(source()));
        self.cached = Some((key, revision, Arc::clone(&document)));
        document
    }

    /// Draw the cached document, returning the link activated this frame.
    pub fn show<'a>(
        &mut self,
        ui: &mut egui::Ui,
        key: K,
        revision: u64,
        source: impl FnOnce() -> &'a str,
    ) -> Option<String> {
        super::show(ui, &self.document(key, revision, source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Default does not need a default key.
    #[derive(Clone, Debug, PartialEq)]
    struct Key(u8);

    #[test]
    fn the_same_key_and_revision_parse_once() {
        let mut preview = Preview::default();
        let mut calls = 0;
        let first = preview.document(Key(1), 1, || {
            calls += 1;
            "# First"
        });
        let again = preview.document(Key(1), 1, || {
            calls += 1;
            "# Changed"
        });
        assert_eq!(calls, 1);
        assert!(Arc::ptr_eq(&first, &again));
        assert_eq!(*first, Document::parse("# First"));
    }

    #[test]
    fn a_new_revision_parses_again() {
        let mut preview = Preview::default();
        let first = preview.document(Key(1), 1, || "# First");
        let next = preview.document(Key(1), 2, || "# Revised");
        assert!(!Arc::ptr_eq(&first, &next));
        assert_eq!(*next, Document::parse("# Revised"));
    }

    #[test]
    fn a_new_key_parses_again() {
        let mut preview = Preview::default();
        let first = preview.document(Key(1), 1, || "# First");
        let next = preview.document(Key(2), 1, || "# Second");
        assert!(!Arc::ptr_eq(&first, &next));
        assert_eq!(*next, Document::parse("# Second"));
    }

    #[test]
    fn clones_share_the_document_and_clear_forgets_it() {
        let mut preview = Preview::default();
        assert!(preview.cached().is_none());
        let first = preview.document(Key(1), 1, || "# First");
        let mut cloned = preview.clone();
        assert!(Arc::ptr_eq(&first, cloned.cached().unwrap()));
        cloned.clear();
        assert!(cloned.cached().is_none());
        assert!(Arc::ptr_eq(&first, preview.cached().unwrap()));
        let next = cloned.document(Key(1), 1, || "# After clear");
        assert!(!Arc::ptr_eq(&first, &next));
        assert_eq!(*next, Document::parse("# After clear"));
    }
}
