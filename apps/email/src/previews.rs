// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Received image previews. Only the current reader owns textures. Jobs
//! retain their slots until completion, including after the reader changes.

use std::collections::BTreeMap;
use std::io::Cursor;

use egui::{ColorImage, Context, TextureHandle};
use image::{ImageFormat, ImageReader, Limits};
use serde_json::{Value, json};

pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_JOBS: usize = 2;

/// The format is chosen from MIME, never sniffed into an unapproved decoder.
pub fn format(part: &jmap::Attachment) -> Option<ImageFormat> {
    if !part.downloadable() || part.size > MAX_BYTES {
        return None;
    }
    match part
        .content_type
        .as_deref()?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" => Some(ImageFormat::Jpeg),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::WebP),
        _ => None,
    }
}

/// Run on the effects runtime's blocking pool. `decode` reads one still
/// image (the first GIF frame), never an animation iterator.
pub fn decode(bytes: &[u8], kind: ImageFormat) -> Result<ColorImage, String> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(crate::label("preview-too-large"));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), kind);
    reader.limits(limits);
    let rgba = reader
        .decode()
        .map_err(|error| error.to_string())?
        .into_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

pub enum Preview {
    Queued,
    Loading,
    Ready(ColorImage),
    Shown(TextureHandle),
    Failed(String),
}

impl Preview {
    pub fn info(&self) -> Value {
        match self {
            Self::Queued | Self::Loading | Self::Ready(_) => json!({"state": "loading"}),
            Self::Shown(_) => json!({"state": "shown"}),
            Self::Failed(reason) => json!({"state": "failed", "reason": reason}),
        }
    }
}

/// Account, connection generation, email and open ticket distinguish even
/// a close/reopen of the same email from an earlier request.
pub type Owner = (String, u64, String, u64);

#[derive(Default)]
pub struct Cache {
    owner: Option<Owner>,
    epoch: u64,
    next: u64,
    entries: BTreeMap<String, Preview>,
    jobs: BTreeMap<u64, (u64, String)>,
}

impl Cache {
    pub fn sync(&mut self, owner: Option<Owner>, parts: &[jmap::Attachment]) {
        if self.owner == owner {
            return;
        }
        self.owner = owner;
        self.epoch += 1;
        self.entries.clear();
        if self.owner.is_some() {
            for part in parts {
                if format(part).is_some()
                    && let Some(id) = &part.part_id
                {
                    self.entries.insert(id.clone(), Preview::Queued);
                }
            }
        }
    }

    pub fn get(&self, part: &str) -> Option<&Preview> {
        self.entries.get(part)
    }

    pub fn is_for(&self, owner: &Owner) -> bool {
        self.owner.as_ref() == Some(owner)
    }

    /// Reserve before dispatch. Old jobs keep their slots, so moving
    /// between emails cannot exceed the process-wide two-job limit.
    pub fn schedule(&mut self, parts: &[jmap::Attachment]) -> Vec<crate::work::Work> {
        let Some((account, _, _, _)) = &self.owner else {
            return Vec::new();
        };
        let mut work = Vec::new();
        for part in parts {
            if self.jobs.len() >= MAX_JOBS {
                break;
            }
            let Some(id) = &part.part_id else { continue };
            if !matches!(self.entries.get(id), Some(Preview::Queued)) {
                continue;
            }
            if let (Some(blob), Some(kind)) = (&part.blob_id, format(part)) {
                self.next += 1;
                self.jobs.insert(self.next, (self.epoch, id.clone()));
                self.entries.insert(id.clone(), Preview::Loading);
                work.push(crate::work::Work::Preview {
                    account: account.clone(),
                    ticket: self.next,
                    blob: blob.clone(),
                    kind,
                });
            }
        }
        work
    }

    pub fn complete(&mut self, ticket: u64, result: Result<ColorImage, String>) {
        if let Some((epoch, part)) = self.jobs.remove(&ticket)
            && epoch == self.epoch
            && self.owner.is_some()
        {
            self.entries.insert(
                part,
                match result {
                    Ok(image) => Preview::Ready(image),
                    Err(reason) => Preview::Failed(reason),
                },
            );
        }
    }

    /// Called only on the window thread, before drawing or reporting shown.
    pub fn upload(&mut self, ctx: &Context) {
        let max = ctx.input(|input| input.max_texture_side);
        for (part, preview) in &mut self.entries {
            if matches!(preview, Preview::Ready(_)) {
                let Preview::Ready(image) = std::mem::replace(preview, Preview::Loading) else {
                    unreachable!();
                };
                *preview = if image.size[0] > max || image.size[1] > max {
                    Preview::Failed(crate::label("preview-texture-limit"))
                } else {
                    Preview::Shown(ctx.load_texture(
                        format!("email-preview-{}-{part}", self.epoch),
                        image,
                        egui::TextureOptions::LINEAR,
                    ))
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    fn part(id: &str, mime: &str) -> jmap::Attachment {
        serde_json::from_value(json!({
            "partId": id, "blobId": format!("blob-{id}"),
            "name": "preview.png", "type": mime, "size": 100,
        }))
        .unwrap()
    }

    fn owner(id: &str) -> Option<Owner> {
        Some(("account".into(), 1, id.into(), 1))
    }

    fn ticket(work: &crate::work::Work) -> u64 {
        match work {
            crate::work::Work::Preview { ticket, .. } => *ticket,
            _ => panic!("expected preview work"),
        }
    }

    #[test]
    fn only_small_decodable_raster_attachments_are_inline() {
        for mime in [
            "image/png",
            "image/jpeg",
            "image/gif",
            "image/webp",
            " IMAGE/PNG ",
        ] {
            assert!(format(&part("1", mime)).is_some(), "{mime}");
        }
        for mime in [
            "image/svg+xml",
            "application/pdf",
            "image/bmp",
            "image/avif",
            "text/html",
        ] {
            assert!(format(&part("1", mime)).is_none(), "{mime}");
        }
        let mut image = part("1", "image/png");
        image.size = MAX_BYTES;
        assert!(format(&image).is_some());
        image.size += 1;
        assert!(format(&image).is_none());
        image.size = 100;
        image.undecodable = true;
        assert!(format(&image).is_none());
        image.undecodable = false;
        image.blob_id = None;
        assert!(format(&image).is_none());
    }

    #[test]
    fn two_slots_cover_download_decode_and_stale_jobs() {
        let mut cache = Cache::default();
        let parts: Vec<_> = (0..5).map(|i| part(&i.to_string(), "image/png")).collect();
        cache.sync(owner("first"), &parts);
        let first = cache.schedule(&parts);
        assert_eq!(first.len(), MAX_JOBS);
        assert!(cache.schedule(&parts).is_empty());
        cache.complete(ticket(&first[0]), Err("invalid image".into()));
        assert_eq!(
            cache.get("0").unwrap().info(),
            json!({"state": "failed", "reason": "invalid image"})
        );
        let next = cache.schedule(&parts);
        assert_eq!(next.len(), 1);
        // Closing the email frees textures, but ongoing work still owns slots.
        cache.sync(None, &[]);
        assert!(cache.entries.is_empty());
        cache.sync(owner("second"), &parts);
        assert!(cache.schedule(&parts).is_empty());
        cache.complete(ticket(&first[1]), Err("old failure".into()));
        assert!(matches!(cache.get("1"), Some(Preview::Queued)));
        assert_eq!(cache.schedule(&parts).len(), 1);
        cache.complete(ticket(&next[0]), Err("another old failure".into()));
        assert!(matches!(cache.get("2"), Some(Preview::Queued)));
        assert_eq!(cache.schedule(&parts).len(), 1);
        assert_eq!(cache.jobs.len(), MAX_JOBS);
    }

    #[test]
    fn a_closed_email_result_is_dropped_even_after_reopening_its_id() {
        let mut cache = Cache::default();
        let parts = [part("1", "image/png")];
        cache.sync(owner("first"), &parts);
        let work = cache.schedule(&parts);
        cache.sync(None, &[]);
        cache.sync(owner("first"), &parts);
        let image = ColorImage::from_rgba_unmultiplied([1, 1], &[255, 0, 0, 255]);
        cache.complete(ticket(&work[0]), Ok(image));
        assert!(matches!(cache.get("1"), Some(Preview::Queued)));
    }

    #[test]
    fn animated_gif_decodes_only_its_first_frame() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for pixel in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                encoder
                    .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                        2,
                        1,
                        image::Rgba(pixel),
                    )))
                    .unwrap();
            }
        }
        let still = decode(&bytes, ImageFormat::Gif).unwrap();
        assert_eq!(still.size, [2, 1]);
        assert!(
            still
                .pixels
                .iter()
                .all(|pixel| *pixel == egui::Color32::RED)
        );
    }

    #[test]
    fn png_dimensions_are_refused_before_allocating_the_image() {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        assert_eq!(decode(&png, ImageFormat::Png).unwrap().size, [1, 1]);
        // A real IHDR with its CRC repaired: the refusal must be the limits,
        // not a corrupt PNG checksum or an attempt to allocate 40 GB.
        png[16..20].copy_from_slice(&100_000_u32.to_be_bytes());
        png[20..24].copy_from_slice(&100_000_u32.to_be_bytes());
        let mut crc = u32::MAX;
        for byte in &png[12..29] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320_u32.wrapping_mul(crc & 1));
            }
        }
        png[29..33].copy_from_slice(&(!crc).to_be_bytes());
        let error = decode(&png, ImageFormat::Png).unwrap_err();
        assert!(error.to_ascii_lowercase().contains("limit"), "{error}");
    }
}
