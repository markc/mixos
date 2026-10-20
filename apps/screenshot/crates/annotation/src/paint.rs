// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! One painter for preview, clipboard and export: what is saved is what was
//! shown. Shapes are stroked with tiny-skia; text is shaped with
//! cosmic-text over the one [`Face`] the caller supplies, so rendering never
//! depends on the fonts a host happens to have.
use crate::document::{Document, Kind, Shape};
use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, fontdb};
use image::{ImageEncoder, RgbaImage};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

/// The text face: a font system holding one embedded face, shared by every
/// render (a worker thread at a time).
pub struct Face {
    family: String,
    fonts: Mutex<(FontSystem, SwashCache)>,
}

impl std::fmt::Debug for Face {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Face")
            .field("family", &self.family)
            .finish()
    }
}

impl Face {
    /// A face from font file bytes (TrueType or OpenType).
    pub fn new(data: &[u8]) -> Result<Self, String> {
        let mut db = fontdb::Database::new();
        db.load_font_data(data.to_vec());
        let family = db
            .faces()
            .next()
            .and_then(|face| face.families.first().map(|(name, _)| name.clone()))
            .ok_or("the font data holds no usable face")?;
        db.set_sans_serif_family(family.clone());
        let fonts = FontSystem::new_with_locale_and_db("en-US".into(), db);
        Ok(Self {
            family,
            fonts: Mutex::new((fonts, SwashCache::new())),
        })
    }

    /// The face's family name.
    pub fn family(&self) -> &str {
        &self.family
    }
}

impl Document {
    /// The flattened image: the original with every object painted over it,
    /// then cropped.
    pub fn render(&self, face: &Face) -> Result<RgbaImage, String> {
        let (w, h) = self.dimensions();
        let mut pixmap = Pixmap::new(w, h).ok_or("image allocation failed")?;
        for (to, from) in pixmap
            .data_mut()
            .chunks_exact_mut(4)
            .zip(self.original.as_raw().chunks_exact(4))
        {
            for c in 0..3 {
                to[c] = ((u16::from(from[c]) * u16::from(from[3]) + 127) / 255) as u8;
            }
            to[3] = from[3];
        }
        for object in &self.history.current().objects {
            paint(&mut pixmap, &object.shape, face)?;
        }
        let mut bytes = pixmap.take();
        for px in bytes.chunks_exact_mut(4) {
            if px[3] != 0 {
                for c in 0..3 {
                    px[c] = ((u32::from(px[c]) * 255 + u32::from(px[3]) / 2) / u32::from(px[3]))
                        .min(255) as u8;
                }
            }
        }
        let image = RgbaImage::from_raw(w, h, bytes).ok_or("image layout mismatch")?;
        Ok(if let Some(c) = self.history.current().crop {
            image::imageops::crop_imm(&image, c.x, c.y, c.width, c.height).to_image()
        } else {
            image
        })
    }

    /// Publish the render at `path`, never replacing anything there
    /// ([`publish_png`]), as an ordinary file the umask governs.
    pub fn export(&self, path: &Path, face: &Face) -> Result<(), String> {
        if !path.is_absolute() {
            return Err("export path must be absolute".into());
        }
        let image = self.render(face)?;
        publish_png(&image, std::iter::once(path.to_path_buf()), PUBLIC).map(|_| ())
    }
}

/// The creation mode of a saved picture: the process umask decides what
/// others may read, as for any file an application saves.
pub const PUBLIC: u32 = 0o666;
/// The creation mode of a private file (whatever the umask).
pub const PRIVATE: u32 = 0o600;

/// Write `image` as PNG to the first of `candidates` that does not exist,
/// and return it. The PNG is encoded into a sibling created with `mode`
/// (the umask applies), synced, then hard-linked into place, which fails
/// rather than replace an existing file or follow a symlink; a taken name
/// moves on to the next candidate. The directory is synced before success
/// is reported, so the published name survives a crash. All candidates
/// must share one directory.
pub fn publish_png(
    image: &RgbaImage,
    candidates: impl IntoIterator<Item = PathBuf>,
    mode: u32,
) -> Result<PathBuf, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut candidates = candidates.into_iter().peekable();
    let parent = candidates
        .peek()
        .and_then(|p| p.parent())
        .ok_or("export needs a parent directory")?
        .to_path_buf();
    let temporary = parent.join(format!(".screenshot-{}.partial", uuid::Uuid::now_v7()));
    let result = (|| -> Result<PathBuf, String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        image::codecs::png::PngEncoder::new(&mut file)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| e.to_string())?;
        file.flush()
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        let mut last = String::from("no file name to publish at");
        for candidate in candidates {
            if candidate.parent() != Some(parent.as_path()) {
                return Err("every candidate must share one directory".into());
            }
            match std::fs::hard_link(&temporary, &candidate) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&temporary);
                    sync_dir(&parent)?;
                    return Ok(candidate);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    last = format!("{} already exists", candidate.display());
                }
                Err(e) => return Err(format!("publish PNG without overwrite: {e}")),
            }
        }
        Err(last)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

/// Sync `dir`, so the names created or renamed in it last.
pub fn sync_dir(dir: &Path) -> Result<(), String> {
    std::fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("sync {}: {e}", dir.display()))
}

fn paint(pixmap: &mut Pixmap, shape: &Shape, face: &Face) -> Result<(), String> {
    shape.validate()?;
    match shape.kind {
        Kind::Text => return paint_text(pixmap, shape, face, None),
        Kind::Number => return paint_marker(pixmap, shape, face),
        _ => {}
    }
    let mut path = PathBuilder::new();
    let a = shape.points[0];
    let b = shape.points[1];
    match shape.kind {
        Kind::Rectangle | Kind::Redact | Kind::Ellipse => {
            let (l, t, r, b) = shape.bounds();
            let Some(rect) = Rect::from_ltrb(l, t, r, b) else {
                return Ok(());
            };
            if shape.kind == Kind::Ellipse {
                path.push_oval(rect);
            } else {
                path.push_rect(rect);
            }
        }
        _ => {
            path.move_to(a.x, a.y);
            for p in &shape.points[1..] {
                path.line_to(p.x, p.y);
            }
            if shape.kind == Kind::Arrow {
                let angle = (b.y - a.y).atan2(b.x - a.x);
                let length = (shape.width * 4.0).max(12.0);
                for offset in [-0.5_f32, 0.5] {
                    path.move_to(b.x, b.y);
                    path.line_to(
                        b.x - length * (angle + offset).cos(),
                        b.y - length * (angle + offset).sin(),
                    );
                }
            }
        }
    }
    let Some(path) = path.finish() else {
        return Ok(());
    };
    let mut colour = shape.colour;
    match shape.kind {
        // Redaction is opaque whatever alpha was asked for.
        Kind::Redact => colour[3] = 255,
        Kind::Highlighter => colour[3] = colour[3].min(80),
        _ => {}
    }
    let mut paint = Paint::default();
    paint.set_color_rgba8(colour[0], colour[1], colour[2], colour[3]);
    paint.anti_alias = shape.kind != Kind::Redact;
    if shape.kind == Kind::Redact {
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    } else {
        let stroke = Stroke {
            width: shape.width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    Ok(())
}

/// A numbered marker: a disc in the object's colour, its number centred in
/// black or white, whichever stands out more.
fn paint_marker(pixmap: &mut Pixmap, shape: &Shape, face: &Face) -> Result<(), String> {
    let (l, t, r, b) = shape.bounds();
    let side = (r - l).min(b - t);
    let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
    if let Some(rect) = Rect::from_ltrb(
        cx - side / 2.0,
        cy - side / 2.0,
        cx + side / 2.0,
        cy + side / 2.0,
    ) {
        let mut path = PathBuilder::new();
        path.push_oval(rect);
        if let Some(path) = path.finish() {
            let mut paint = Paint::default();
            let [r, g, b, a] = shape.colour;
            paint.set_color_rgba8(r, g, b, a);
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
    let [r, g, b, _] = shape.colour;
    let luminance = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
    let ink = if luminance > 140.0 {
        [0, 0, 0, 255]
    } else {
        [255, 255, 255, 255]
    };
    paint_text(pixmap, shape, face, Some(ink))
}

/// Text inside the object's box, clipped to it and to the image, so hit
/// bounds, selection and export agree. A marker's number is centred.
fn paint_text(
    pixmap: &mut Pixmap,
    shape: &Shape,
    face: &Face,
    ink: Option<[u8; 4]>,
) -> Result<(), String> {
    let size = shape.size.ok_or("missing text size")?;
    let (left, top, right, bottom) = shape.bounds();
    let content = match shape.kind {
        Kind::Number => shape.number.ok_or("missing marker number")?.to_string(),
        _ => shape.text.clone().ok_or("missing text")?,
    };
    let mut guard = face.fonts.lock().map_err(|_| "text shaper poisoned")?;
    let (fonts, cache) = &mut *guard;
    let line_height = size * 1.25;
    let mut buffer = Buffer::new(fonts, Metrics::new(size, line_height));
    let attrs = Attrs::new().family(Family::Name(&face.family));
    {
        let mut buffer = buffer.borrow_with(fonts);
        buffer.set_size(Some(right - left), Some(bottom - top));
        buffer.set_text(&content, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(false);
    }
    // A marker's number sits in the middle of its disc.
    let (mut dx, mut dy) = (0.0f32, 0.0f32);
    if shape.kind == Kind::Number {
        let width = buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0f32, f32::max);
        dx = ((right - left) - width) / 2.0;
        dy = ((bottom - top) - line_height) / 2.0;
    }
    let [r, g, b, a] = ink.unwrap_or(shape.colour);
    let origin_x = (left + dx).floor() as i32;
    let origin_y = (top + dy).floor() as i32;
    let clip_l = (left.floor() as i32).max(0);
    let clip_t = (top.floor() as i32).max(0);
    let clip_r = (right.ceil() as i32).min(pixmap.width() as i32);
    let clip_b = (bottom.ceil() as i32).min(pixmap.height() as i32);
    let width = pixmap.width() as usize;
    let data = pixmap.data_mut();
    buffer.draw(
        fonts,
        cache,
        Color::rgba(r, g, b, a),
        |x, y, w, h, colour| {
            let x = origin_x + x;
            let y = origin_y + y;
            let [r, g, b, a] = colour.as_rgba();
            if a == 0 {
                return;
            }
            for yy in y.max(clip_t)..(y + h as i32).min(clip_b) {
                for xx in x.max(clip_l)..(x + w as i32).min(clip_r) {
                    let at = (yy as usize * width + xx as usize) * 4;
                    let px = &mut data[at..at + 4];
                    // Source-over onto premultiplied pixels.
                    for (to, from) in px.iter_mut().take(3).zip([r, g, b]) {
                        *to = ((u16::from(from) * u16::from(a)
                            + u16::from(*to) * (255 - u16::from(a))
                            + 127)
                            / 255) as u8;
                    }
                    px[3] = (u16::from(a) + (u16::from(px[3]) * (255 - u16::from(a)) + 127) / 255)
                        as u8;
                }
            }
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Crop, Point};
    use image::Rgba;

    pub(crate) fn face() -> Face {
        Face::new(include_bytes!(
            "../../../../../libs/toolkit/assets/fonts/Inter-Regular.ttf"
        ))
        .unwrap()
    }

    fn doc() -> Document {
        Document::new(RgbaImage::from_pixel(32, 24, Rgba([255, 255, 255, 255]))).unwrap()
    }

    fn shape(kind: Kind) -> Shape {
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
            s.size = Some(10.0);
        }
        if kind == Kind::Number {
            s.number = Some(1);
        }
        s
    }

    #[test]
    fn the_face_names_its_family() {
        assert_eq!(face().family(), "Inter");
        assert!(Face::new(b"not a font").is_err());
    }

    #[test]
    fn crop_is_nondestructive_and_export_is_no_clobber() {
        let face = face();
        let mut d = doc();
        d.add(shape(Kind::Redact)).unwrap();
        d.set_crop(Some(Crop {
            x: 4,
            y: 4,
            width: 12,
            height: 8,
        }))
        .unwrap();
        assert_eq!(d.render(&face).unwrap().dimensions(), (12, 8));
        assert_eq!(d.dimensions(), (32, 24));
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("shot.png");
        d.export(&p, &face).unwrap();
        let before = std::fs::read(&p).unwrap();
        assert!(d.export(&p, &face).is_err());
        assert_eq!(before, std::fs::read(&p).unwrap());
        assert_eq!(
            image::open(&p).unwrap().to_rgba8(),
            d.render(&face).unwrap()
        );
        assert!(d.export(Path::new("relative.png"), &face).is_err());
        d.undo();
        assert_eq!(d.output_dimensions(), (32, 24));
        // Nothing private is left behind.
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["shot.png"]);
    }

    #[test]
    fn a_symlink_is_never_followed_or_replaced() {
        let face = face();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere.png");
        let link = dir.path().join("link.png");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(doc().export(&link, &face).is_err());
        assert!(!target.exists());
    }

    #[test]
    fn redaction_is_opaque_even_with_translucent_colour() {
        let mut d = doc();
        let mut s = shape(Kind::Redact);
        s.colour = [0, 0, 0, 1];
        d.add(s).unwrap();
        assert_eq!(
            *d.render(&face()).unwrap().get_pixel(10, 10),
            Rgba([0, 0, 0, 255])
        );
    }

    #[test]
    fn every_kind_changes_the_render_and_edits_clear_redo() {
        let face = face();
        for kind in Kind::ALL {
            let mut d = doc();
            d.add(shape(kind)).unwrap();
            assert_ne!(d.render(&face).unwrap(), *d.original(), "{kind:?}");
            d.undo();
            assert!(d.can_redo());
            d.add(shape(Kind::Rectangle)).unwrap();
            assert!(!d.can_redo());
        }
    }

    #[test]
    fn a_marker_is_a_disc_with_contrasting_ink() {
        let face = face();
        let mut d = doc();
        let mut marker = shape(Kind::Number);
        marker.points = vec![Point { x: 2.0, y: 2.0 }, Point { x: 22.0, y: 22.0 }];
        marker.size = Some(14.0);
        d.add(marker).unwrap();
        let image = d.render(&face).unwrap();
        // The disc's rim is the marker's colour; somewhere inside is the
        // white ink of the number.
        assert_eq!(image.get_pixel(12, 4).0, [255, 0, 0, 255]);
        assert!(
            (6..18)
                .flat_map(|y| (6..18).map(move |x| (x, y)))
                .any(|(x, y)| image.get_pixel(x, y).0[1] > 200),
            "the number is drawn in white on red"
        );
    }

    #[test]
    fn publish_moves_to_the_next_free_name() {
        let dir = tempfile::tempdir().unwrap();
        let image = RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255]));
        let names = |n: usize| {
            (0..n)
                .map(|i| dir.path().join(format!("shot-{i}.png")))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            publish_png(&image, names(3), PUBLIC).unwrap(),
            dir.path().join("shot-0.png")
        );
        assert_eq!(
            publish_png(&image, names(3), PUBLIC).unwrap(),
            dir.path().join("shot-1.png")
        );
        std::fs::write(dir.path().join("shot-2.png"), b"taken").unwrap();
        assert!(
            publish_png(&image, names(3), PUBLIC)
                .unwrap_err()
                .contains("exists")
        );
        assert_eq!(
            std::fs::read(dir.path().join("shot-2.png")).unwrap(),
            b"taken"
        );
    }
}
