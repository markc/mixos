// SPDX-License-Identifier: MIT OR Apache-2.0
//! An in-process document: the text editor without a daemon.
//!
//! [`Document`] owns an [`edit::buffer::Buffer`] (the same buffer the edit
//! service keeps), the editing model and the highlighter, and applies the
//! widget's [`Event`]s to them. Undo groups typing the way the edit service
//! does. Open and save keep the file's byte-order mark, line endings and
//! permissions; save writes a fresh temporary file beside the real file
//! (through a symbolic link, to its target) and renames it into place.
//!
//! ```ignore
//! let mut doc = editor::local::Document::open("notes.md")?;
//! let palette = editor::Palette::from_theme(&theme);
//! let view = editor::View::prose();
//! doc.show(ui, &palette, &view);
//! if let Some(refusal) = doc.take_refusal() { /* tell the user */ }
//! if doc.is_dirty() { /* offer to save */ }
//! ```

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use edit::buffer::{Applied, Buffer, Cas, Eol, FileMeta, LaneSel, OpSpec, TxnRequest};
use edit::error::{CoreError, reason};
use edit::origin::{Origin, OriginKind, Via};
use edit::pos::RangeSpec;
use edit::text::Text;
use editor_model::highlight::Highlight;
use editor_model::model::{EditCfg, EditorModel, line_comment_for};
use editor_model::types::{DeltaKind, ViewDelta};

use crate::{Doc, EditCommand, Event, Output, Palette, Selection, View};

/// Process-unique document identities.
static NEXT_IDENTITY: AtomicU64 = AtomicU64::new(1);

/// The origin local edits are recorded under.
const LOCAL_LABEL: &str = "editor";

/// Lines scanned for the file's own indentation.
const INDENT_SCAN_LINES: usize = 1000;

/// Why a document could not be opened, edited or saved.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// Not UTF-8, too large, or another refusal of the buffer.
    Text(CoreError),
    /// Save without a path.
    NoPath,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Text(e) => f.write_str(&e.message),
            Self::NoPath => f.write_str("the document has no file yet"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<CoreError> for Error {
    fn from(e: CoreError) -> Self {
        Self::Text(e)
    }
}

pub struct Document {
    buffer: Buffer,
    meta: FileMeta,
    path: Option<PathBuf>,
    language: String,
    /// Tab inserts a tab: Makefiles, Go, and files already indented with tabs.
    indent_tabs: bool,
    model: EditorModel,
    highlight: Highlight,
    identity: u64,
    view_gen: u64,
    origin: Origin,
    /// The last edit the buffer refused, for the owner to report.
    refusal: Option<Error>,
}

impl Document {
    /// A new, unsaved document holding `text`, highlighted as `language`
    /// (an edit-service language id such as `markdown`, `rust` or `text`).
    pub fn new(text: &str, language: &str) -> Result<Self, Error> {
        let (buffer, meta) = Buffer::from_bytes(text.as_bytes())?;
        let mut doc = Self::with_buffer(buffer, meta, None, language);
        doc.buffer.mark_saved();
        Ok(doc)
    }

    /// Open `path`. The language comes from its name and first line.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let bytes = fs::read(path)?;
        let (mut buffer, meta) = Buffer::from_bytes(&bytes)?;
        buffer.mark_saved();
        let first_line =
            String::from_utf8_lossy(bytes.split(|b| *b == b'\n').next().unwrap_or_default());
        let language = edit::lang::detect(Some(path), &first_line);
        Ok(Self::with_buffer(
            buffer,
            meta,
            Some(path.to_path_buf()),
            language,
        ))
    }

    fn with_buffer(buffer: Buffer, meta: FileMeta, path: Option<PathBuf>, language: &str) -> Self {
        let indent_tabs = matches!(language, "go" | "makefile")
            || path.as_deref().is_some_and(is_makefile)
            || indents_with_tabs(buffer.text());
        Self {
            highlight: Highlight::for_language(language, path.as_deref()),
            buffer,
            meta,
            path,
            language: language.to_string(),
            indent_tabs,
            model: EditorModel::default(),
            identity: NEXT_IDENTITY.fetch_add(1, Ordering::Relaxed),
            view_gen: 0,
            origin: Origin::new(OriginKind::Human, LOCAL_LABEL),
            refusal: None,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    pub fn text(&self) -> &Text {
        self.buffer.text()
    }

    /// The whole text as a string.
    pub fn contents(&self) -> String {
        let mut s = String::with_capacity(self.buffer.len());
        self.buffer.read(0..self.buffer.len(), &mut s);
        s
    }

    pub fn model(&self) -> &EditorModel {
        &self.model
    }

    /// The model, for the owner to set a selection or scroll directly.
    pub fn model_mut(&mut self) -> &mut EditorModel {
        &mut self.model
    }

    pub fn selection(&self) -> Selection {
        self.model.sel
    }

    /// Changed since it was opened or last saved.
    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }

    /// The last edit the buffer refused since this was last called (a paste
    /// over the 1 MiB request limit, for one).
    pub fn take_refusal(&mut self) -> Option<Error> {
        self.refusal.take()
    }

    /// The borrowed view the widget draws.
    pub fn doc(&self) -> Doc<'_> {
        Doc {
            text: self.buffer.text(),
            identity: self.identity,
            revision: self.buffer.rev(),
            model: &self.model,
            highlight: Some(&self.highlight),
            diagnostics: &[],
        }
    }

    /// Show the editor in `ui` and apply what it asks for. The widget's
    /// state (scroll, drag, composition) is keyed by this document.
    pub fn show(&mut self, ui: &mut egui::Ui, palette: &Palette, view: &View) -> Output {
        self.show_as(ui, self.identity, palette, view)
    }

    /// [`Self::show`] in a pane with its own id, for an editor that shows one
    /// document after another: the pane notices the switch, so input meant
    /// for the previous document never reaches this one.
    pub fn show_as(
        &mut self,
        ui: &mut egui::Ui,
        id_salt: impl std::hash::Hash + std::fmt::Debug,
        palette: &Palette,
        view: &View,
    ) -> Output {
        let out = crate::show(ui, id_salt, &self.doc(), palette, view);
        for e in &out.events {
            self.apply(e, view);
        }
        out
    }

    /// Apply one widget event. A refusal is kept for [`Self::take_refusal`].
    pub fn apply(&mut self, event: &Event, view: &View) {
        let result = match event {
            Event::Command(c) => self.command(c.clone(), view),
            Event::Scrolled(s) => {
                self.model.scroll = *s;
                Ok(())
            }
            Event::Preedit(p) => {
                self.model.set_preedit(!p.is_empty());
                Ok(())
            }
            Event::Undo => self.undo(),
            Event::Redo => self.redo(),
            Event::Focus(_) | Event::Layout(_) => Ok(()),
        };
        if let Err(e) = result {
            self.refusal = Some(e);
        }
    }

    fn cfg(&self, view: &View) -> EditCfg {
        EditCfg {
            measure: view.measure(),
            insert_spaces: !self.indent_tabs,
            eol: if self.meta.eol == Eol::Crlf {
                "\r\n"
            } else {
                "\n"
            },
            line_comment: line_comment_for(&self.language),
        }
    }

    /// Run one editing or motion command.
    pub fn command(&mut self, c: EditCommand, view: &View) -> Result<(), Error> {
        let cfg = self.cfg(view);
        let Some(edit) = self.model.command(self.buffer.text(), &cfg, c) else {
            return Ok(());
        };
        let ops = edit
            .items
            .into_iter()
            .map(|(r, text)| OpSpec::Replace {
                range: RangeSpec::Offsets([r.start, r.end]),
                text,
            })
            .collect();
        let req = TxnRequest {
            ops,
            cas: Cas::Latest,
            coalesce: edit.coalesce,
            cursor: None,
            op_id: None,
        };
        let applied = self.buffer.apply(req, &self.origin, via(), now_ms())?;
        self.delta(&applied, DeltaKind::Local);
        self.model.sel = edit.caret_after;
        Ok(())
    }

    /// Undo the last group of edits; nothing to undo is not an error.
    pub fn undo(&mut self) -> Result<(), Error> {
        match self
            .buffer
            .undo(LaneSel::Own, &self.origin, via(), now_ms())
        {
            Ok(applied) => {
                self.delta(&applied, DeltaKind::Undo);
                self.caret_after_history(&applied);
                Ok(())
            }
            Err(e) if e.reason == Some(reason::NOTHING_TO_UNDO) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Redo the last undone group; nothing to redo is not an error.
    pub fn redo(&mut self) -> Result<(), Error> {
        match self
            .buffer
            .redo(LaneSel::Own, &self.origin, via(), now_ms())
        {
            Ok(applied) => {
                self.delta(&applied, DeltaKind::Redo);
                self.caret_after_history(&applied);
                Ok(())
            }
            Err(e) if e.reason == Some(reason::NOTHING_TO_REDO) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Replace the whole text, as a reload from disk or a regenerated
    /// document: only the changed middle is replaced, it is recorded as a
    /// reload (Undo does not take it back), and it has no size limit beyond
    /// the buffer's.
    pub fn set_contents(&mut self, text: &str) -> Result<(), Error> {
        if let Some(applied) = self.buffer.reload_minimal(text, via(), now_ms())? {
            self.delta(&applied, DeltaKind::Reload);
            self.model.clamp(self.buffer.text());
        }
        Ok(())
    }

    fn delta(&mut self, applied: &Applied, kind: DeltaKind) {
        self.view_gen += 1;
        // No origin: the model marks other origins' changes, and every edit
        // here is the user's own.
        let d = ViewDelta {
            edits: applied.edits.clone(),
            origin: None,
            kind,
            rev: applied.rev,
            view_gen: self.view_gen,
        };
        self.model.apply_delta(&d);
        self.highlight.apply_delta(self.buffer.text(), &d);
    }

    /// After undo or redo the caret goes to the end of the last span it put
    /// back, or where it removed text.
    fn caret_after_history(&mut self, applied: &Applied) {
        let at = applied
            .changed
            .last()
            .map(|r| r.end)
            .or_else(|| applied.edits.last().map(|e| e.offset))
            .unwrap_or(self.model.sel.head)
            .min(self.buffer.len());
        self.model.sel = Selection {
            anchor: at,
            head: at,
        };
    }

    /// Write the document to its path.
    pub fn save(&mut self) -> Result<(), Error> {
        let path = self.path.clone().ok_or(Error::NoPath)?;
        self.save_as(path)
    }

    /// Write the document to `path`, which becomes its path. A symbolic link
    /// stays a link: its target is written.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<(), Error> {
        let path = path.as_ref();
        let target = match fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => fs::canonicalize(path)?,
            _ => path.to_path_buf(),
        };
        write_replacing(&target, &self.buffer.to_bytes(&self.meta))?;
        self.buffer.mark_saved();
        if self.path.as_deref() != Some(path) {
            self.path = Some(path.to_path_buf());
        }
        Ok(())
    }
}

/// Write `bytes` to a new temporary file beside `target` (created
/// exclusively, so nothing existing is followed or truncated), give it
/// `target`'s permissions, flush it, and rename it over `target`. A failure
/// removes the temporary file and leaves `target` as it was.
fn write_replacing(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let dir = target
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let permissions = fs::metadata(target).ok().map(|m| m.permissions());
    let (tmp, mut file) = (0..1000)
        .find_map(|n| {
            let mut tmp_name = OsString::from(".");
            tmp_name.push(name);
            tmp_name.push(format!(".{}.{n}.saving", std::process::id()));
            let tmp = dir.join(tmp_name);
            match OpenOptions::new().write(true).create_new(true).open(&tmp) {
                Ok(f) => Some(Ok((tmp, f))),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => None,
                Err(e) => Some(Err(e)),
            }
        })
        .unwrap_or_else(|| {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "no free temporary file name",
            ))
        })?;
    let written = (|| {
        file.write_all(bytes)?;
        if let Some(p) = permissions {
            file.set_permissions(p)?;
        }
        file.sync_all()?;
        fs::rename(&tmp, target)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

fn is_makefile(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(name, "Makefile" | "makefile" | "GNUmakefile")
        || path.extension().is_some_and(|e| e == "mk")
}

/// The first indented line among the first [`INDENT_SCAN_LINES`] starts with
/// a tab.
fn indents_with_tabs(text: &Text) -> bool {
    for line in 1..=text.line_count().min(INDENT_SCAN_LINES) {
        let Some(start) = text.line_start(line) else {
            break;
        };
        match text.chunk_at(start).first() {
            Some(b'\t') => return true,
            Some(b' ') => return false,
            _ => {}
        }
    }
    false
}

fn via() -> Via {
    Via {
        from: None,
        broker_origin: "local".into(),
        broker_peer: None,
        broker_service: None,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Motion;

    fn view() -> View {
        View::default()
    }

    fn run(doc: &mut Document, c: EditCommand) {
        doc.command(c, &view()).unwrap();
    }

    fn typed(doc: &mut Document, s: &str) {
        for c in s.chars() {
            run(doc, EditCommand::Insert(c.to_string()));
        }
    }

    fn to(offset: usize) -> EditCommand {
        EditCommand::Move {
            to: Motion::To(offset),
            extend: false,
        }
    }

    #[test]
    fn typing_undo_and_redo() {
        let mut doc = Document::new("", "text").unwrap();
        assert!(!doc.is_dirty());
        typed(&mut doc, "hello");
        assert_eq!(doc.contents(), "hello");
        assert_eq!(doc.selection().head, 5);
        assert!(doc.is_dirty());
        run(&mut doc, EditCommand::Newline);
        typed(&mut doc, "world");
        assert_eq!(doc.contents(), "hello\nworld");
        doc.undo().unwrap();
        assert_ne!(
            doc.contents(),
            "hello\nworld",
            "undo takes the last group back"
        );
        while !doc.contents().is_empty() {
            let before = doc.contents();
            doc.undo().unwrap();
            assert_ne!(
                doc.contents(),
                before,
                "every undo changes something until empty"
            );
        }
        doc.undo().unwrap();
        assert!(
            doc.take_refusal().is_none(),
            "nothing to undo is no refusal"
        );
        doc.redo().unwrap();
        assert!(!doc.contents().is_empty());
        assert!(doc.selection().head <= doc.text().len());
    }

    #[test]
    fn selections_and_deletes() {
        let mut doc = Document::new("one two three", "text").unwrap();
        run(&mut doc, EditCommand::SelectWord(5));
        assert_eq!(doc.selection(), Selection { anchor: 4, head: 7 });
        run(&mut doc, EditCommand::Delete);
        assert_eq!(doc.contents(), "one  three");
        run(
            &mut doc,
            EditCommand::Move {
                to: Motion::DocEnd,
                extend: false,
            },
        );
        run(&mut doc, EditCommand::DeleteWordLeft);
        assert_eq!(doc.contents(), "one  ");
    }

    #[test]
    fn refusals_are_kept_for_the_owner() {
        let mut doc = Document::new("", "text").unwrap();
        let huge = "x".repeat(edit::limits::MAX_REQUEST_TEXT_BYTES + 1);
        doc.apply(&Event::Command(EditCommand::Insert(huge.clone())), &view());
        assert!(matches!(doc.take_refusal(), Some(Error::Text(_))));
        assert_eq!(doc.contents(), "");
        doc.set_contents(&huge).unwrap();
        assert_eq!(
            doc.contents().len(),
            huge.len(),
            "a reload has no request limit"
        );
    }

    #[test]
    fn tabs_follow_makefiles_and_the_file_itself() {
        let dir = tempfile::tempdir().unwrap();
        let make = dir.path().join("Makefile");
        fs::write(&make, "all:\n").unwrap();
        let mut doc = Document::open(&make).unwrap();
        run(&mut doc, to(5));
        run(&mut doc, EditCommand::Tab);
        assert_eq!(doc.contents(), "all:\n\t");
        let mut doc = Document::new("fn x() {\n\tgo();\n}\n", "rust").unwrap();
        run(&mut doc, to(0));
        run(&mut doc, EditCommand::Tab);
        assert!(
            doc.contents().starts_with('\t'),
            "indented with tabs already"
        );
        let mut doc = Document::new("a:\n  b: 1\n", "yaml").unwrap();
        run(&mut doc, to(0));
        run(&mut doc, EditCommand::Tab);
        assert!(doc.contents().starts_with(' '));
    }

    #[test]
    fn save_keeps_line_endings_bom_and_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.md");
        fs::write(&path, b"\xEF\xBB\xBFone\r\ntwo\r\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut doc = Document::open(&path).unwrap();
        assert_eq!(doc.language(), "markdown");
        run(&mut doc, to(3));
        run(&mut doc, EditCommand::Newline);
        typed(&mut doc, "1.5");
        assert!(doc.is_dirty());
        doc.save().unwrap();
        assert!(!doc.is_dirty());
        assert_eq!(
            fs::read(&path).unwrap(),
            b"\xEF\xBB\xBFone\r\n1.5\r\ntwo\r\n"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["notes.md"], "no temporary file is left");
    }

    #[test]
    fn save_writes_through_a_link_and_never_follows_a_stray_temporary_name() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.md");
        let link = dir.path().join("link.md");
        fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, "keep").unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join("link.md.saving")).unwrap();
        let mut doc = Document::open(&link).unwrap();
        run(&mut doc, to(3));
        typed(&mut doc, "!");
        doc.save().unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "old!");
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
        assert_eq!(doc.path(), Some(link.as_path()));
    }

    #[test]
    fn a_new_document_needs_a_path_to_save() {
        let mut doc = Document::new("x", "text").unwrap();
        assert!(matches!(doc.save(), Err(Error::NoPath)));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.txt");
        doc.save_as(&path).unwrap();
        assert_eq!(doc.path(), Some(path.as_path()));
        assert_eq!(fs::read_to_string(&path).unwrap(), "x");
    }

    #[test]
    fn markdown_is_highlighted() {
        let doc = Document::new("# Title\n\nSome *text*.\n", "markdown").unwrap();
        let mut budget = editor_model::highlight::SliceBudget::default();
        let spans = doc
            .doc()
            .highlight
            .unwrap()
            .with_spans(doc.text(), 1, &mut budget, |s| s.map(<[_]>::to_vec))
            .unwrap();
        assert!(!spans.is_empty(), "the heading has spans");
    }
}
