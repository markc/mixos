// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Grayscale SSIM: uniform sliding 8x8 valid windows, population covariance,
//! Rec.601 luma (alpha ignored), L=255, K1=0.01, K2=0.03. No padding.
//! PNG inputs, or tightly packed RGBA bytes when --size WxH is supplied.
use image::{GrayImage, ImageReader, RgbaImage};
use std::{error::Error, path::Path};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn size(text: &str) -> Result<(u32, u32)> {
    let (w, h) = text.split_once('x').ok_or("expected WxH")?;
    let pair = (w.parse()?, h.parse()?);
    if pair.0 == 0 || pair.1 == 0 {
        return Err("dimensions must be positive".into());
    }
    Ok(pair)
}

fn load(path: &Path, raw: Option<(u32, u32)>) -> Result<GrayImage> {
    let rgba = if let Some((w, h)) = raw {
        let bytes = std::fs::read(path)?;
        if u64::try_from(bytes.len())? != u64::from(w) * u64::from(h) * 4 {
            return Err("raw input must contain exactly width * height * 4 bytes".into());
        }
        RgbaImage::from_raw(w, h, bytes)
            .ok_or("raw input must contain exactly width * height * 4 bytes")?
    } else {
        let reader = ImageReader::open(path)?.with_guessed_format()?;
        if reader.format() != Some(image::ImageFormat::Png) {
            return Err("expected PNG (or --size for raw RGBA)".into());
        }
        reader.decode()?.to_rgba8()
    };
    Ok(GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y);
        image::Luma([
            (0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2])).round()
                as u8,
        ])
    }))
}

fn ssim(a: &GrayImage, b: &GrayImage) -> Result<f64> {
    if a.dimensions() != b.dimensions() {
        return Err("image dimensions differ".into());
    }
    let (w, h) = a.dimensions();
    if w < 8 || h < 8 {
        return Err("SSIM needs at least 8x8 pixels".into());
    }
    let mut total = 0.0;
    for y in 0..=h - 8 {
        for x in 0..=w - 8 {
            let (mut sa, mut sb, mut aa, mut bb, mut ab) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for dy in 0..8 {
                for dx in 0..8 {
                    let a = f64::from(a.get_pixel(x + dx, y + dy)[0]);
                    let b = f64::from(b.get_pixel(x + dx, y + dy)[0]);
                    sa += a;
                    sb += b;
                    aa += a * a;
                    bb += b * b;
                    ab += a * b;
                }
            }
            let (ma, mb) = (sa / 64.0, sb / 64.0);
            let (va, vb, cov) = (
                aa / 64.0 - ma * ma,
                bb / 64.0 - mb * mb,
                ab / 64.0 - ma * mb,
            );
            let (c1, c2) = ((0.01_f64 * 255.0).powi(2), (0.03_f64 * 255.0).powi(2));
            total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2))
                / ((ma * ma + mb * mb + c1) * (va + vb + c2));
        }
    }
    Ok(total / (f64::from(w - 7) * f64::from(h - 7)))
}

fn run() -> Result<bool> {
    let mut args = std::env::args().skip(1);
    let a = args
        .next()
        .ok_or("usage: testkit-ssim A B [--threshold T] [--crop X,Y,WxH] [--size WxH]")?;
    let b = args.next().ok_or("missing B")?;
    let mut threshold = 0.99;
    let mut raw = None;
    let mut crop = None;
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("option needs a value")?;
        match flag.as_str() {
            "--threshold" => threshold = value.parse::<f64>()?,
            "--size" => raw = Some(size(&value)?),
            "--crop" => {
                let parts: Vec<_> = value.split(',').collect();
                if parts.len() != 3 {
                    return Err("crop needs X,Y,WxH".into());
                }
                let (w, h) = size(parts[2])?;
                crop = Some((parts[0].parse::<u32>()?, parts[1].parse::<u32>()?, w, h));
            }
            _ => return Err(format!("unknown option {flag}").into()),
        }
    }
    if !threshold.is_finite() || !(-1.0..=1.0).contains(&threshold) {
        return Err("threshold must be finite and in -1..=1".into());
    }
    let (mut a, mut b) = (load(Path::new(&a), raw)?, load(Path::new(&b), raw)?);
    // Refuse mismatched originals even if a crop would conceal the mismatch.
    if a.dimensions() != b.dimensions() {
        return Err("image dimensions differ".into());
    }
    if let Some((x, y, w, h)) = crop {
        if x.checked_add(w).is_none_or(|right| right > a.width())
            || y.checked_add(h).is_none_or(|bottom| bottom > a.height())
        {
            return Err("crop lies outside image".into());
        }
        a = image::imageops::crop_imm(&a, x, y, w, h).to_image();
        b = image::imageops::crop_imm(&b, x, y, w, h).to_image();
    }
    let value = ssim(&a, &b)?;
    let pass = value >= threshold;
    println!("TESTKIT_SSIM ssim={value:.12} threshold={threshold} pass={pass}");
    Ok(pass)
}

fn main() {
    match run() {
        Ok(true) => (),
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("testkit-ssim: {error}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical() {
        let image =
            GrayImage::from_fn(17, 19, |x, y| image::Luma([((x * 31 + y * 7) % 256) as u8]));
        assert_eq!(ssim(&image, &image).unwrap(), 1.0);
    }
    #[test]
    fn known_luminance_distortion() {
        let a = GrayImage::from_pixel(16, 16, image::Luma([100]));
        let b = GrayImage::from_pixel(16, 16, image::Luma([110]));
        // Constant patches: covariance term cancels, SSIM = 22006.5025/22106.5025.
        let expected = 22006.5025 / 22106.5025;
        let value = ssim(&a, &b).unwrap();
        println!("known distortion 100 -> 110: {value:.12}");
        assert!((value - expected).abs() < 1e-12);
    }
    #[test]
    fn mismatched_sizes_refused() {
        assert!(ssim(&GrayImage::new(16, 16), &GrayImage::new(16, 17)).is_err());
        assert!(ssim(&GrayImage::new(7, 8), &GrayImage::new(7, 8)).is_err());
    }

    #[test]
    fn inverted_checkerboard_has_negative_covariance() {
        let a = GrayImage::from_fn(8, 8, |x, y| image::Luma([if (x + y) % 2 == 0 { 0 } else { 255 }]));
        let b = GrayImage::from_fn(8, 8, |x, y| image::Luma([255 - a.get_pixel(x, y)[0]]));
        // Equal means; variance = 255^2/4 and covariance = -variance.
        let expected = (-2.0 * 16256.25 + 58.5225) / (2.0 * 16256.25 + 58.5225);
        assert!((ssim(&a, &b).unwrap() - expected).abs() < 1e-12);
    }
}
