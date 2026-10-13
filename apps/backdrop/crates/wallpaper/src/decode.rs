// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Decoding one wallpaper image. The controller runs this on its one decode
//! worker, never on the UI or compositor thread.
//!
//! One file is opened once, with `O_NONBLOCK` so a FIFO can never block the
//! worker. The handle is checked (a regular file, within [`MAX_FILE_BYTES`]),
//! read through a `take` that refuses anything past the limit, and the
//! resulting bytes are the only input: the format, the header's dimensions and
//! the pixels all come from those bytes, never from a second open by path.
//! Limits are applied before any pixel is allocated: the decoder's own limits
//! (`image::Limits`), and the header's dimensions against [`MAX_SIDE`] and
//! [`MAX_PIXELS`].
//!
//! Per decode, the most memory is: the compressed bytes ([`MAX_FILE_BYTES`]),
//! the decoder's allocations ([`MAX_ALLOC_BYTES`]), and the kept RGBA, at most
//! [`MAX_PIXELS`] × 4 bytes (128 MiB) before the downscale. The downscale brings
//! the kept image to [`TARGET_SIDE`] on the longest side (56.25 MiB at most).
//! These are separate budgets: conversion can overlap the decoder's image,
//! and resizing can overlap the 128 MiB RGBA buffer. Resident images are
//! accounted separately by the controller; this is not a process RSS cap.

use image::{ImageFormat, ImageReader, RgbaImage, imageops::FilterType};
use std::fs::OpenOptions;
use std::io::{BufReader, Cursor, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

/// Compressed input larger than this is refused.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// The longest side a decoded image may have.
pub const MAX_SIDE: u32 = 16_384;
/// Pixels a decoded image may have (32 Mpx: 128 MiB of RGBA).
pub const MAX_PIXELS: u64 = 32 * 1024 * 1024;
/// Allocation the decoder may make in all, intermediates included.
pub const MAX_ALLOC_BYTES: u64 = 256 * 1024 * 1024;
/// The longest side kept: the texture target, larger images are downscaled.
pub const TARGET_SIDE: u32 = 3840;

/// A decoded wallpaper: RGBA, downscaled to the texture target.
#[derive(Debug, Clone)]
pub struct Decoded {
    pub size: [u32; 2],
    pub rgba: RgbaImage,
}

impl Decoded {
    /// Bytes of RGBA this image holds.
    pub fn bytes(&self) -> u64 {
        u64::from(self.size[0]) * u64::from(self.size[1]) * 4
    }
}

/// The decoder's limits: the image's dimensions and its allocation.
fn limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC_BYTES);
    limits
}

/// Read `path` once: one opened handle, checked as a regular file, at most
/// [`MAX_FILE_BYTES`]. A FIFO or device is refused on the handle, after an
/// `O_NONBLOCK` open that cannot block.
pub fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let shown = path.display();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("{shown}: {e}"))?;
    let meta = file.metadata().map_err(|e| format!("{shown}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{shown} is not a regular file"));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!(
            "{shown} is {} bytes; the limit is {MAX_FILE_BYTES}",
            meta.len()
        ));
    }
    let mut bytes = Vec::with_capacity(meta.size().min(MAX_FILE_BYTES) as usize);
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{shown}: {e}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("{shown} grew past the {MAX_FILE_BYTES} byte limit"));
    }
    Ok(bytes)
}

/// The dimensions a decode may allocate for.
pub fn check_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("an empty image".into());
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!(
            "{width} × {height} is over the {MAX_SIDE}-pixel side limit"
        ));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!(
            "{width} × {height} is over the {MAX_PIXELS}-pixel limit"
        ));
    }
    Ok(())
}

/// Decode PNG or JPEG `bytes` under the limits, downscaled to the texture
/// target. `shown` names the source in errors.
pub fn decode_bytes(bytes: &[u8], shown: &str) -> Result<Decoded, String> {
    let format = match image::guess_format(bytes) {
        Ok(format @ (ImageFormat::Png | ImageFormat::Jpeg)) => format,
        Ok(other) => return Err(format!("{shown} is {other:?}, not PNG or JPEG")),
        Err(_) => return Err(format!("{shown} is not a PNG or JPEG image")),
    };
    // The header first: the dimensions are checked before any pixel buffer.
    let mut header = ImageReader::with_format(Cursor::new(bytes), format);
    header.limits(limits());
    let (width, height) = header
        .into_dimensions()
        .map_err(|e| format!("{shown}: header: {e}"))?;
    check_dimensions(width, height).map_err(|e| format!("{shown}: {e}"))?;
    let mut reader = ImageReader::with_format(BufReader::new(Cursor::new(bytes)), format);
    reader.limits(limits());
    let rgba = reader
        .decode()
        .map_err(|e| format!("{shown}: decode: {e}"))?
        .into_rgba8();
    if rgba.dimensions() != (width, height) {
        return Err(format!("{shown}: the header and the pixels disagree"));
    }
    let rgba = downscale(rgba);
    Ok(Decoded {
        size: [rgba.width(), rgba.height()],
        rgba,
    })
}

/// Read and decode the file at `path` (see [`read_bounded`], [`decode_bytes`]).
pub fn load(path: &Path) -> Result<Decoded, String> {
    let bytes = read_bounded(path)?;
    decode_bytes(&bytes, &path.display().to_string())
}

/// `image` scaled down so its longest side is at most [`TARGET_SIDE`].
fn downscale(rgba: RgbaImage) -> RgbaImage {
    let (width, height) = rgba.dimensions();
    let longest = width.max(height);
    if longest <= TARGET_SIDE {
        return rgba;
    }
    let scale = f64::from(TARGET_SIDE) / f64::from(longest);
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).max(1);
    image::imageops::resize(&rgba, side(width), side(height), FilterType::Triangle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage, Rgba};

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// CRC-32 (IEEE, reflected), as PNG chunks carry it.
    fn crc32(parts: &[&[u8]]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for part in parts {
            for &byte in *part {
                crc ^= u32::from(byte);
                for _ in 0..8 {
                    crc = if crc & 1 == 1 {
                        (crc >> 1) ^ 0xEDB8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
        }
        !crc
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
    }

    /// A PNG whose header claims `width` × `height` and carries no pixels.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA, no interlace
        chunk(&mut out, b"IHDR", &ihdr);
        // A valid empty zlib stream lets the PNG reader inspect IDAT and
        // report dimensions. Pixel decoding is never reached by these tests.
        chunk(
            &mut out,
            b"IDAT",
            &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01],
        );
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn a_png_and_a_jpeg_decode_to_their_size() {
        let dir = dir();
        let png = dir.path().join("a.png");
        RgbaImage::from_pixel(40, 20, Rgba([10, 20, 30, 255]))
            .save(&png)
            .unwrap();
        let jpeg = dir.path().join("b.jpg");
        DynamicImage::ImageRgb8(RgbImage::from_pixel(30, 30, Rgb([200, 100, 50])))
            .save_with_format(&jpeg, ImageFormat::Jpeg)
            .unwrap();
        assert_eq!(load(&png).unwrap().size, [40, 20]);
        assert_eq!(load(&jpeg).unwrap().size, [30, 30]);
    }

    #[test]
    fn a_wide_image_is_downscaled_to_the_texture_target() {
        let dir = dir();
        let path = dir.path().join("wide.png");
        RgbaImage::new(4000, 10).save(&path).unwrap();
        assert_eq!(load(&path).unwrap().size, [TARGET_SIDE, 10]);
    }

    #[test]
    fn an_oversized_png_header_is_refused_before_any_pixel_buffer() {
        // Valid chunks, a header claiming 20000 × 20000 and no pixel data: the
        // dimension limit must answer, not the decoder.
        let dir = dir();
        let path = dir.path().join("huge.png");
        std::fs::write(&path, png_header(20_000, 20_000)).unwrap();
        // The image library's own dimension limit (`Image size exceeds limit`)
        // refuses this header first, and our side check would say the same.
        let error = load(&path).unwrap_err();
        assert!(error.contains("limit"), "{error}");
        // Just inside the side limit but over the pixel limit.
        let path = dir.path().join("wide.png");
        std::fs::write(&path, png_header(8_000, 8_000)).unwrap();
        let error = load(&path).unwrap_err();
        assert!(error.contains("pixel limit"), "{error}");
    }

    #[test]
    fn a_fifo_is_refused_without_blocking_the_worker() {
        use std::os::unix::ffi::OsStrExt;
        let dir = dir();
        let fifo = dir.path().join("pipe.png");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        // No writer exists: a blocking open would hang this test.
        let error = load(&fifo).unwrap_err();
        assert!(error.contains("not a regular file"), "{error}");
    }

    #[test]
    fn refused_inputs_are_named() {
        let dir = dir();
        let corrupt = dir.path().join("corrupt.png");
        std::fs::write(&corrupt, b"not an image at all").unwrap();
        assert!(load(&corrupt).unwrap_err().contains("PNG or JPEG"));
        assert!(load(&dir.path().join("missing.png")).is_err());
        let big = dir.path().join("big.png");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(load(&big).unwrap_err().contains("limit"));
        assert!(load(dir.path()).is_err(), "a directory is not an image");
    }

    #[test]
    fn dimensions_are_limited_before_allocation() {
        assert!(check_dimensions(0, 10).is_err());
        assert!(check_dimensions(MAX_SIDE + 1, 1).is_err());
        assert!(
            check_dimensions(8000, 8000).is_err(),
            "64 Mpx is over the pixel limit"
        );
        assert!(check_dimensions(8192, 4096).is_ok());
        assert!(check_dimensions(MAX_SIDE, 1).is_ok());
    }
}
