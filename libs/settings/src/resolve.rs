// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Resolution. [`values`] is key space v2's resolver over the registry's
//! sparse values; the design compile it shares with the format-4 path
//! ([`crate::v1`]) lives here. The v1 functions are re-exported below for
//! their present callers only (settingsd's store, and tests in Prefs and
//! portald); S6 removes the re-exports with [`crate::v1`].
use crate::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub mod values;

pub use crate::v1::{describe, patch, resolve, resolve_with_embedded, resolve_with_embedded_and_accents};

pub const APPS: &[&str] = &["ced", "dopus", "term", "cap", "workshop", "scene-editor"];

pub(crate) fn error(path: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("validation_failed", path, message)
}


/// The accent each context draws, as [`design::resolved_accent`] gives it for
/// that context's design. A context whose design has no accent is absent.
///
/// Kept beside the effective map, never inside [`Effective`]: effective values
/// are sealed into saved profiles (`effective_digest`), so the accent must not
/// change their shape.
pub type Accents = BTreeMap<String, design::SrgbColour>;

/// The last design source parsed, by its digest. A profile's source rarely
/// changes (the embedded one never does), so an apply reuses the parse.
static PARSED: Mutex<Option<(String, Arc<design::DesignSourceDocument>)>> = Mutex::new(None);

pub(crate) fn parsed(source: &str) -> Result<Arc<design::DesignSourceDocument>, Vec<Diagnostic>> {
    let source_id = crate::source_digest(source);
    let mut cache = PARSED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((digest, doc)) = cache.as_ref()
        && *digest == source_id
    {
        return Ok(doc.clone());
    }
    let doc = Arc::new(
        design::parse_design_source(design::SourceIdentity::new(&source_id), source)
            .map_err(|e| vec![error("appearance.source", e.to_string())])?,
    );
    *cache = Some((source_id, doc.clone()));
    Ok(doc)
}

/// Whether `source` parses and compiles for the desktop context: the
/// registry's kind check for `appearance.source`. Resolution still compiles
/// every context before a profile is accepted. The error is the first
/// diagnostic's message.
pub(crate) fn source_compiles(source: &str) -> Result<(), String> {
    let first = |messages: Vec<String>| messages.into_iter().next().unwrap_or_default();
    let doc = parsed(source).map_err(|d| first(d.into_iter().map(|d| d.message).collect()))?;
    match design::compile_design(&doc, design::DesignContext::default()) {
        design::DesignCompileResult::Success(_) => Ok(()),
        design::DesignCompileResult::Fatal(failure) => Err(first(
            failure.diagnostics.into_iter().map(|d| d.message).collect(),
        )),
    }
}

/// The source uses the app axis: it orders it or a modifier block names an
/// app. Only then can the requested app change a compile.
pub(crate) fn claims_app_axis(doc: &design::DesignSourceDocument) -> bool {
    doc.v1.resolution_order.contains(&design::ModifierAxis::App)
        || doc
            .v1
            .modifiers
            .iter()
            .any(|block| block.when.contains_key(&design::ModifierAxis::App))
}

pub(crate) type Compiled = (design::DesignReadProjection, Option<design::SrgbColour>);

/// Compile `context`'s design, sharing work across contexts. A source that
/// does not use the app axis compiles every app exactly as no app
/// (`app_without_a_modifier_compiles_as_none`), so those share one compile.
/// A failure is compiled again under the context asked for: its diagnostics
/// are exactly what that context's own compile reports.
pub(crate) fn compile_context(
    doc: &design::DesignSourceDocument,
    context: design::DesignContext,
    app_axis: bool,
    compiled: &mut BTreeMap<design::DesignContext, Compiled>,
) -> Result<Compiled, Vec<design::DesignDiagnostic>> {
    let mut key = context.clone();
    if !app_axis {
        key.app = None;
    }
    if let Some(done) = compiled.get(&key) {
        return Ok(done.clone());
    }
    match design::compile_design(doc, key.clone()) {
        design::DesignCompileResult::Success(success) => {
            let done = (
                success.candidate.read_projection(),
                design::resolved_accent(&success.candidate),
            );
            compiled.insert(key, done.clone());
            Ok(done)
        }
        design::DesignCompileResult::Fatal(failure) if key == context => Err(failure.diagnostics),
        // The context's own compile decides, as it did before sharing.
        design::DesignCompileResult::Fatal(_) => match design::compile_design(doc, context) {
            design::DesignCompileResult::Fatal(failure) => Err(failure.diagnostics),
            design::DesignCompileResult::Success(success) => Ok((
                success.candidate.read_projection(),
                design::resolved_accent(&success.candidate),
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The premise of the shared compile: with the embedded source, every
    /// built-in app compiles to exactly what no app does.
    #[test]
    fn app_without_a_modifier_compiles_as_none() {
        let doc = parsed(design::EMBEDDED_DEFAULT_SOURCE).unwrap();
        let compile = |app: Option<&str>| match design::compile_design(
            &doc,
            design::DesignContext {
                app: app.map(str::to_owned),
                ..Default::default()
            },
        ) {
            design::DesignCompileResult::Success(success) => (
                serde_json::to_string(&success.candidate.read_projection()).unwrap(),
                design::resolved_accent(&success.candidate),
            ),
            design::DesignCompileResult::Fatal(_) => panic!("the embedded source compiles"),
        };
        assert!(
            !claims_app_axis(&doc),
            "the embedded source now uses the app axis: apps no longer share a compile"
        );
        let none = compile(None);
        for app in APPS {
            assert!(compile(Some(app)) == none, "{app}");
        }
    }

    /// The embedded source, made to claim the app axis (naming `term`) with
    /// a broken pair in the ocean/dark modifier: a fatal compile whose
    /// diagnostics carry the requested app's coordinate.
    fn app_axis_fatal_source() -> design::DesignSourceDocument {
        let mut doc = (*parsed(design::EMBEDDED_DEFAULT_SOURCE).unwrap()).clone();
        doc.v1.resolution_order.push(design::ModifierAxis::App);
        doc.v1
            .modifiers
            .push(serde_json::from_value(json!({"when":{"app":"term"}})).unwrap());
        let ocean_dark = doc
            .v1
            .modifiers
            .iter_mut()
            .find(|b| {
                b.when
                    .get(&design::ModifierAxis::Scheme)
                    .map(String::as_str)
                    == Some("ocean")
                    && b.when.get(&design::ModifierAxis::Mode).map(String::as_str) == Some("dark")
            })
            .expect("an ocean/dark modifier");
        ocean_dark.semantics.pairs.insert(
            "base".into(),
            serde_json::from_value(json!({
                "surface":"missing.surface","foreground":"palette.foreground.default"
            }))
            .unwrap(),
        );
        doc
    }

    #[test]
    fn a_fatal_compile_reports_the_requested_context_s_own_diagnostics() {
        let doc = app_axis_fatal_source();
        assert!(claims_app_axis(&doc));
        let context = design::DesignContext {
            app: Some("workshop".into()),
            ..Default::default()
        };
        let own = match design::compile_design(&doc, context.clone()) {
            design::DesignCompileResult::Fatal(failure) => failure.diagnostics,
            design::DesignCompileResult::Success(_) => panic!("the broken source fails"),
        };
        assert!(
            own.iter().any(|d| d.path.contains("app=workshop")),
            "the fixture reaches the requested app: {own:#?}"
        );
        // Sharing is off for a source that uses the app axis…
        let shared = compile_context(&doc, context.clone(), true, &mut BTreeMap::new());
        assert_eq!(shared.unwrap_err(), own);
        // …and even a forced shared compile reports the context's own failure.
        let forced = compile_context(&doc, context, false, &mut BTreeMap::new());
        assert_eq!(forced.unwrap_err(), own);
    }
}
