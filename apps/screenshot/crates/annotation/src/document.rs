// SPDX-License-Identifier: MIT OR Apache-2.0
//! Immutable source pixels and bounded editable objects. Undo snapshots
//! hold geometry, never raster copies; the original is shared, never
//! written.
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};

/// The largest image a document holds, in pixels.
pub const MAX_PIXELS: u64 = 32 * 1024 * 1024;
/// The most objects one document holds.
pub const MAX_OBJECTS: usize = 256;
/// The most points one freehand stroke keeps.
pub const MAX_POINTS: usize = 16_384;
/// A stroke's width, in original-image pixels. Wide enough that a preset
/// drawn on a large image shown scaled down (an 8K shot at fit) keeps the
/// width it shows on screen.
pub const WIDTH: std::ops::RangeInclusive<f32> = 0.5..=512.0;
/// A text or marker size, in original-image pixels (as [`WIDTH`]).
pub const TEXT_SIZE: std::ops::RangeInclusive<f32> = 8.0..=1024.0;
const MAX_HISTORY: usize = 128;
const MAX_DOCUMENT_POINTS: usize = 131_072;
const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;
const MAX_SIDE: u32 = 16_384;

/// A position in original-image pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn valid(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.x.abs() <= 65536.0
            && self.y.abs() <= 65536.0
    }
}

/// The part of the original that is shown and exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// What an object draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Pen,
    Highlighter,
    Redact,
    Text,
    Number,
}

impl Kind {
    /// Every kind, in toolbar order.
    pub const ALL: [Self; 9] = [
        Self::Arrow,
        Self::Line,
        Self::Rectangle,
        Self::Ellipse,
        Self::Pen,
        Self::Highlighter,
        Self::Redact,
        Self::Text,
        Self::Number,
    ];

    /// The wire name (`kind` in `screenshot.annotate`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Arrow => "arrow",
            Self::Rectangle => "rectangle",
            Self::Ellipse => "ellipse",
            Self::Pen => "pen",
            Self::Highlighter => "highlighter",
            Self::Redact => "redact",
            Self::Text => "text",
            Self::Number => "number",
        }
    }

    /// The kind named `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// A stroke that keeps every point (the others keep two).
    pub fn freehand(self) -> bool {
        matches!(self, Self::Pen | Self::Highlighter)
    }

    /// Drawn from a box: text, a numbered marker.
    pub fn boxed_text(self) -> bool {
        matches!(self, Self::Text | Self::Number)
    }
}

/// One object's geometry and look, in original-image pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    pub kind: Kind,
    pub points: Vec<Point>,
    pub colour: [u8; 4],
    pub width: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
}

impl Shape {
    /// A shape of `kind` through `points`, with no text fields.
    pub fn new(kind: Kind, points: Vec<Point>, colour: [u8; 4], width: f32) -> Self {
        Self {
            kind,
            points,
            colour,
            width,
            text: None,
            size: None,
            number: None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.width.is_finite() || !WIDTH.contains(&self.width) {
            return Err(format!(
                "stroke width must be {}..{} pixels",
                WIDTH.start(),
                WIDTH.end()
            ));
        }
        let count = self.points.len();
        if !(2..=MAX_POINTS).contains(&count)
            || self.points.iter().any(|p| !p.valid())
            || (!self.kind.freehand() && count != 2)
        {
            return Err("invalid annotation points".into());
        }
        if self.kind.boxed_text() {
            let (l, t, r, b) = self.bounds();
            if r - l < 1.0 || b - t < 1.0 {
                return Err("text needs a non-empty box".into());
            }
            if self
                .size
                .is_none_or(|s| !s.is_finite() || !TEXT_SIZE.contains(&s))
            {
                return Err(format!(
                    "text size must be {}..{} pixels",
                    TEXT_SIZE.start(),
                    TEXT_SIZE.end()
                ));
            }
            match self.kind {
                Kind::Text
                    if self.text.as_ref().is_none_or(|t| {
                        t.trim().is_empty() || t.len() > 4096 || t.lines().count() > 16
                    }) || self.number.is_some() =>
                {
                    return Err("text must contain 1..4096 bytes and at most 16 lines".into());
                }
                Kind::Number
                    if self.number.is_none_or(|n| !(1..=9999).contains(&n))
                        || self.text.is_some() =>
                {
                    return Err("marker number must be 1..9999".into());
                }
                _ => {}
            }
        } else if self.text.is_some() || self.size.is_some() || self.number.is_some() {
            return Err("text fields only apply to text and number objects".into());
        }
        Ok(())
    }

    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        let mut shape = self.clone();
        for p in &mut shape.points {
            p.x += dx;
            p.y += dy;
        }
        shape
    }

    /// Left, top, right, bottom of the points.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        self.points.iter().fold(
            (
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ),
            |(l, t, r, b), p| (l.min(p.x), t.min(p.y), r.max(p.x), b.max(p.y)),
        )
    }
}

/// An object with its stable id.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Object {
    pub id: u64,
    pub shape: Shape,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub(crate) struct State {
    pub(crate) objects: Vec<Object>,
    pub(crate) crop: Option<Crop>,
}

impl State {
    fn bytes(&self) -> usize {
        self.objects
            .iter()
            .map(|o| {
                std::mem::size_of::<Object>()
                    + o.shape.points.len() * std::mem::size_of::<Point>()
                    + o.shape.text.as_ref().map_or(0, String::len)
            })
            .sum()
    }
}

/// One screenshot being annotated.
#[derive(Debug, Clone)]
pub struct Document {
    pub(crate) original: Arc<RgbaImage>,
    pub(crate) state: State,
    undo: Vec<State>,
    redo: Vec<State>,
    saved: State,
    next_id: u64,
}

impl Document {
    pub fn new(image: RgbaImage) -> Result<Self, String> {
        check_size(image.width(), image.height())?;
        Ok(Self {
            original: Arc::new(image),
            state: State::default(),
            undo: vec![],
            redo: vec![],
            saved: State::default(),
            next_id: 1,
        })
    }

    /// Decode the image at `path`: one regular file, opened without
    /// blocking (a FIFO or device is refused, never waited on), its size
    /// checked before the pixels are decoded.
    pub fn open(path: &Path) -> Result<Self, String> {
        use std::io::{BufReader, Seek};
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .map_err(|e| e.to_string())?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("image input must be a regular file".into());
        }
        let probe = file.try_clone().map_err(|e| e.to_string())?;
        let (w, h) = image::ImageReader::new(BufReader::new(probe))
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .into_dimensions()
            .map_err(|e| e.to_string())?;
        check_size(w, h)?;
        file.rewind().map_err(|e| e.to_string())?;
        let mut reader = image::ImageReader::new(BufReader::new(file))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAX_SIDE);
        limits.max_image_height = Some(MAX_SIDE);
        limits.max_alloc = Some(MAX_PIXELS * 8);
        reader.limits(limits);
        Self::new(reader.decode().map_err(|e| e.to_string())?.to_rgba8())
    }

    /// The original's size.
    pub fn dimensions(&self) -> (u32, u32) {
        self.original.dimensions()
    }

    /// The exported size: the crop's, else the original's.
    pub fn output_dimensions(&self) -> (u32, u32) {
        self.state
            .crop
            .map_or(self.dimensions(), |c| (c.width, c.height))
    }

    pub fn original(&self) -> &RgbaImage {
        &self.original
    }

    pub fn crop(&self) -> Option<Crop> {
        self.state.crop
    }

    pub fn objects(&self) -> &[Object] {
        &self.state.objects
    }

    pub fn object(&self, id: u64) -> Option<&Object> {
        self.state.objects.iter().find(|o| o.id == id)
    }

    /// Changed since it was last saved (or since it was captured).
    pub fn dirty(&self) -> bool {
        self.state != self.saved
    }

    /// Anything drawn on it, or cropped: the export differs from the original.
    pub fn annotated(&self) -> bool {
        !self.state.objects.is_empty() || self.state.crop.is_some()
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.state.clone();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The number the next numbered marker takes; `None` once 9999 is
    /// used, so no number ever repeats.
    pub fn next_number(&self) -> Option<u32> {
        let next = self
            .state
            .objects
            .iter()
            .filter_map(|o| o.shape.number)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        (next <= 9999).then_some(next)
    }

    /// A copy for a raster worker: no history, no saved checkpoint.
    pub fn raster_snapshot(&self) -> Self {
        Self {
            original: self.original.clone(),
            state: self.state.clone(),
            undo: vec![],
            redo: vec![],
            saved: State::default(),
            next_id: self.next_id,
        }
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.state.clone());
        while self.undo.len() > MAX_HISTORY
            || self.undo.iter().map(State::bytes).sum::<usize>() > MAX_HISTORY_BYTES
        {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Add `shape`; its new id.
    pub fn add(&mut self, shape: Shape) -> Result<u64, String> {
        shape.validate()?;
        if self.state.objects.len() >= MAX_OBJECTS {
            return Err("annotation limit reached".into());
        }
        if self
            .state
            .objects
            .iter()
            .map(|o| o.shape.points.len())
            .sum::<usize>()
            + shape.points.len()
            > MAX_DOCUMENT_POINTS
        {
            return Err("document point budget exceeded".into());
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("annotation id exhausted")?;
        self.checkpoint();
        self.state.objects.push(Object { id, shape });
        Ok(id)
    }

    pub fn delete(&mut self, id: u64) -> Result<(), String> {
        let at = self.position(id)?;
        self.checkpoint();
        self.state.objects.remove(at);
        Ok(())
    }

    pub fn move_object(&mut self, id: u64, dx: f32, dy: f32) -> Result<(), String> {
        let at = self.position(id)?;
        let moved = self.state.objects[at].shape.translated(dx, dy);
        moved.validate()?;
        if dx != 0.0 || dy != 0.0 {
            self.checkpoint();
            self.state.objects[at].shape = moved;
        }
        Ok(())
    }

    /// Change object `id`'s colour and stroke width (one undo step).
    pub fn restyle(&mut self, id: u64, colour: [u8; 4], width: f32) -> Result<(), String> {
        let at = self.position(id)?;
        let mut shape = self.state.objects[at].shape.clone();
        shape.colour = colour;
        shape.width = width;
        shape.validate()?;
        if shape != self.state.objects[at].shape {
            self.checkpoint();
            self.state.objects[at].shape = shape;
        }
        Ok(())
    }

    fn position(&self, id: u64) -> Result<usize, String> {
        self.state
            .objects
            .iter()
            .position(|o| o.id == id)
            .ok_or_else(|| "no such annotation".into())
    }

    pub fn set_crop(&mut self, crop: Option<Crop>) -> Result<(), String> {
        if let Some(c) = crop {
            let (w, h) = self.dimensions();
            if c.width == 0
                || c.height == 0
                || c.x.checked_add(c.width).is_none_or(|x| x > w)
                || c.y.checked_add(c.height).is_none_or(|y| y > h)
            {
                return Err("crop outside image".into());
            }
        }
        if self.state.crop != crop {
            self.checkpoint();
            self.state.crop = crop;
        }
        Ok(())
    }

    pub fn undo(&mut self) -> bool {
        if let Some(s) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.state, s));
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(s) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.state, s));
            true
        } else {
            false
        }
    }

    /// The topmost object within `tolerance` pixels of `p`.
    pub fn hit(&self, p: Point, tolerance: f32) -> Option<u64> {
        self.state
            .objects
            .iter()
            .rev()
            .find(|o| {
                let (l, t, r, b) = o.shape.bounds();
                let margin = tolerance.max(o.shape.width / 2.0);
                p.x >= l - margin && p.x <= r + margin && p.y >= t - margin && p.y <= b + margin
            })
            .map(|o| o.id)
    }

    /// The document as the Bus reports it.
    pub fn info(&self) -> serde_json::Value {
        let (width, height) = self.output_dimensions();
        let (original_width, original_height) = self.dimensions();
        serde_json::json!({
            "width": width, "height": height,
            "original_width": original_width, "original_height": original_height,
            "dirty": self.dirty(), "annotated": self.annotated(),
            "objects": self.state.objects, "crop": self.state.crop,
            "undo": self.undo.len(), "redo": self.redo.len(),
        })
    }
}

fn check_size(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || u64::from(w) * u64::from(h) > MAX_PIXELS
    {
        Err("image exceeds the screenshot pixel budget".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    pub(crate) fn doc() -> Document {
        Document::new(RgbaImage::from_pixel(32, 24, Rgba([255, 255, 255, 255]))).unwrap()
    }

    pub(crate) fn shape(kind: Kind) -> Shape {
        let mut s = Shape::new(
            kind,
            vec![Point { x: 4.0, y: 4.0 }, Point { x: 20.0, y: 16.0 }],
            [255, 0, 0, 255],
            3.0,
        );
        if kind == Kind::Text {
            s.text = Some("A".into());
        }
        if kind.boxed_text() {
            s.size = Some(8.0);
        }
        if kind == Kind::Number {
            s.number = Some(1);
        }
        s
    }

    #[test]
    fn non_regular_image_inputs_are_refused_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pipe.png");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is NUL-terminated and lives for this synchronous call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(Document::open(&path).unwrap_err().contains("regular file"));
        assert!(Document::open(directory.path()).is_err());
        assert!(Document::open(Path::new("/dev/zero")).is_err());
    }

    #[test]
    fn gestures_undo_without_mutating_original() {
        let mut d = doc();
        let original = d.original.clone();
        let id = d.add(shape(Kind::Arrow)).unwrap();
        d.move_object(id, 2.0, 3.0).unwrap();
        assert_eq!(d.undo.len(), 2);
        assert!(Arc::ptr_eq(&original, &d.original));
        d.undo();
        assert_eq!(d.objects()[0].shape.points[0], Point { x: 4.0, y: 4.0 });
        d.undo();
        assert!(!d.dirty());
        d.redo();
        assert_eq!(d.objects()[0].id, id);
        d.mark_saved();
        assert!(!d.dirty());
        d.delete(id).unwrap();
        assert!(d.dirty());
        d.undo();
        assert!(!d.dirty());
    }

    #[test]
    fn malformed_and_oversized_operations_do_not_change_history() {
        let mut d = doc();
        let mut s = shape(Kind::Pen);
        s.points[0].x = f32::NAN;
        assert!(d.add(s).is_err());
        assert!(
            d.set_crop(Some(Crop {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 2
            }))
            .is_err()
        );
        assert!(!d.can_undo());
        for _ in 0..MAX_OBJECTS {
            d.add(shape(Kind::Line)).unwrap();
        }
        assert!(d.add(shape(Kind::Line)).is_err());
        assert_eq!(d.undo.len(), MAX_HISTORY);
    }

    #[test]
    fn text_fields_belong_to_text_objects_only() {
        let mut line = shape(Kind::Line);
        line.text = Some("no".into());
        assert!(line.validate().is_err());
        let mut text = shape(Kind::Text);
        text.text = Some("  ".into());
        assert!(text.validate().is_err());
        let mut marker = shape(Kind::Number);
        marker.number = Some(0);
        assert!(marker.validate().is_err());
        let mut arrow = shape(Kind::Arrow);
        arrow.points.push(Point { x: 1.0, y: 1.0 });
        assert!(arrow.validate().is_err(), "only freehand keeps more points");
    }

    #[test]
    fn markers_count_up_and_restyle_is_one_undo_step() {
        let mut d = doc();
        assert_eq!(d.next_number(), Some(1));
        let mut first = shape(Kind::Number);
        first.number = d.next_number();
        let id = d.add(first).unwrap();
        assert_eq!(d.next_number(), Some(2));
        let mut last = shape(Kind::Number);
        last.number = Some(9999);
        let top = d.add(last).unwrap();
        assert_eq!(d.next_number(), None, "no number repeats at the cap");
        d.delete(top).unwrap();
        d.restyle(id, [0, 0, 255, 255], 6.0).unwrap();
        assert_eq!(d.object(id).unwrap().shape.colour, [0, 0, 255, 255]);
        d.undo();
        assert_eq!(d.object(id).unwrap().shape.colour, [255, 0, 0, 255]);
        assert!(d.restyle(id, [0, 0, 0, 255], 0.0).is_err());
    }

    #[test]
    fn hit_finds_the_topmost_object() {
        let mut d = doc();
        let below = d.add(shape(Kind::Rectangle)).unwrap();
        let above = d.add(shape(Kind::Ellipse)).unwrap();
        assert_eq!(d.hit(Point { x: 10.0, y: 10.0 }, 1.0), Some(above));
        d.delete(above).unwrap();
        assert_eq!(d.hit(Point { x: 10.0, y: 10.0 }, 1.0), Some(below));
        assert_eq!(d.hit(Point { x: 30.0, y: 23.0 }, 1.0), None);
    }

    #[test]
    fn kinds_round_trip_by_name_and_wire() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from_name(kind.name()), Some(kind));
            let wire = serde_json::to_value(kind).unwrap();
            assert_eq!(wire, kind.name());
        }
        assert!(serde_json::from_value::<Shape>(serde_json::json!({"kind":"arrow","points":[],"colour":[0,0,0,255],"width":2,"extra":1})).is_err());
    }
}
