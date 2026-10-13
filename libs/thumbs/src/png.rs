// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The few PNG facts the thumbnail cache needs, read from the bytes with no
//! pixel decoding: the image size (IHDR), the `tEXt` keys, and a copy with
//! keys set. The Thumbnail Managing Standard keeps its `Thumb::*` keys in
//! `tEXt` chunks; external thumbnailers write only the image, so the cache
//! adds the keys itself.

const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// One chunk: its type and data.
struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
}

/// The most chunks read from one PNG. A thumbnailer's output has a few, or
/// a few hundred IDAT chunks at most (an xx-large RGBA image in 8 KiB
/// chunks is about 500); past this it is not a thumbnail.
pub const MAX_CHUNKS: usize = 4096;

/// Every chunk up to and including IEND, or `None` for anything that is not
/// a well-formed PNG (bad signature, a length past the end, no IHDR first,
/// no IEND, more than [`MAX_CHUNKS`]).
fn chunks(bytes: &[u8]) -> Option<Vec<Chunk<'_>>> {
    let mut rest = bytes.strip_prefix(&SIGNATURE)?;
    let mut out = Vec::new();
    loop {
        if rest.len() < 12 || out.len() >= MAX_CHUNKS {
            return None;
        }
        let len = u32::from_be_bytes(rest[0..4].try_into().ok()?) as usize;
        let kind: [u8; 4] = rest[4..8].try_into().ok()?;
        let end = 8usize.checked_add(len)?;
        let data = rest.get(8..end)?;
        rest = rest.get(end + 4..)?;
        if out.is_empty() && &kind != b"IHDR" {
            return None;
        }
        out.push(Chunk { kind, data });
        if &kind == b"IEND" {
            return Some(out);
        }
    }
}

/// The image's width and height, from IHDR.
pub fn size(bytes: &[u8]) -> Option<(u32, u32)> {
    let chunks = chunks(bytes)?;
    let ihdr = chunks.first()?.data;
    if ihdr.len() < 8 {
        return None;
    }
    let w = u32::from_be_bytes(ihdr[0..4].try_into().ok()?);
    let h = u32::from_be_bytes(ihdr[4..8].try_into().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// Chunks kept free under [`MAX_CHUNKS`] in a thumbnailer's output: the
/// cache adds up to five `tEXt` keys when it stores one, and the stored
/// entry must still be readable.
pub const KEY_ROOM: usize = 8;

/// Whether a 13-byte IHDR body is legal: width and height 1..=2^31-1, a
/// bit depth allowed for its colour type, compression and filter method 0,
/// interlace 0 or 1.
fn ihdr_legal(ihdr: &[u8]) -> bool {
    let Ok(ihdr) = <[u8; 13]>::try_from(ihdr) else {
        return false;
    };
    let side = |b: [u8; 4]| (1..=0x7FFF_FFFF).contains(&u32::from_be_bytes(b));
    let [
        w0,
        w1,
        w2,
        w3,
        h0,
        h1,
        h2,
        h3,
        depth,
        colour,
        compression,
        filter,
        interlace,
    ] = ihdr;
    let depth_ok = match colour {
        0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(depth, 8 | 16),
        _ => false,
    };
    side([w0, w1, w2, w3])
        && side([h0, h1, h2, h3])
        && depth_ok
        && compression == 0
        && filter == 0
        && interlace <= 1
}

/// Whether `bytes` is a structurally whole PNG, checked without decoding a
/// pixel (this library has no decoders): the signature; a legal 13-byte
/// IHDR first and only there ([`ihdr_legal`]); a PLTE before the image
/// data when the colour type is indexed, and none for greyscale; one run of
/// consecutive IDAT chunks; IEND last, empty, with nothing after it; a
/// valid CRC on every chunk; and at most [`MAX_CHUNKS`] less [`KEY_ROOM`]
/// chunks. A thumbnailer's output must pass before it is cached.
pub fn is_whole(bytes: &[u8]) -> bool {
    let Some(mut rest) = bytes.strip_prefix(&SIGNATURE) else {
        return false;
    };
    let (mut count, mut idat, mut idat_ended) = (0usize, false, false);
    let (mut colour, mut plte) = (0u8, false);
    loop {
        if rest.len() < 12 || count >= MAX_CHUNKS - KEY_ROOM {
            return false;
        }
        let len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let Some(end) = 8usize.checked_add(len) else {
            return false;
        };
        let (Some(body), Some(crc)) = (rest.get(4..end), rest.get(end..end + 4)) else {
            return false;
        };
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(body);
        if hasher.finalize().to_be_bytes() != crc {
            return false;
        }
        let (kind, data) = body.split_at(4);
        if count == 0 {
            if kind != b"IHDR" || !ihdr_legal(data) {
                return false;
            }
            colour = data[9];
        } else {
            match kind {
                b"IHDR" => return false,
                b"PLTE" => {
                    let fits = !data.is_empty() && data.len() % 3 == 0 && data.len() <= 768;
                    if plte || idat || matches!(colour, 0 | 4) || !fits {
                        return false;
                    }
                    plte = true;
                }
                b"IDAT" => {
                    if idat_ended || (colour == 3 && !plte) {
                        return false;
                    }
                    idat = true;
                }
                _ => idat_ended |= idat,
            }
        }
        count += 1;
        rest = &rest[end + 4..];
        if kind == b"IEND" {
            return idat && data.is_empty() && rest.is_empty();
        }
    }
}

/// The value of `tEXt` key `key` (Latin-1, read as such).
pub fn text(bytes: &[u8], key: &str) -> Option<String> {
    chunks(bytes)?.into_iter().find_map(|chunk| {
        if &chunk.kind != b"tEXt" {
            return None;
        }
        let nul = chunk.data.iter().position(|b| *b == 0)?;
        (&chunk.data[..nul] == key.as_bytes())
            .then(|| chunk.data[nul + 1..].iter().map(|b| *b as char).collect())
    })
}

/// A copy of `bytes` with each `(key, value)` set as a `tEXt` chunk right
/// after IHDR, replacing any chunk with the same key. Keys and values must be
/// Latin-1 (`None` otherwise, for a malformed PNG, or when the copy would
/// hold more than [`MAX_CHUNKS`] chunks and so not read back).
pub fn with_text(bytes: &[u8], entries: &[(&str, &str)]) -> Option<Vec<u8>> {
    let latin1 = |s: &str| -> Option<Vec<u8>> {
        s.chars().map(|c| u8::try_from(u32::from(c)).ok()).collect()
    };
    let mut added: Vec<Vec<u8>> = Vec::new();
    for (key, value) in entries {
        let key = latin1(key)?;
        if key.is_empty() || key.len() > 79 || key.contains(&0) {
            return None;
        }
        let mut data = key;
        data.push(0);
        data.extend(latin1(value)?);
        added.push(data);
    }
    let keys: Vec<&[u8]> = entries.iter().map(|(k, _)| k.as_bytes()).collect();
    let mut out = SIGNATURE.to_vec();
    for (index, chunk) in chunks(bytes)?.into_iter().enumerate() {
        let replaced = &chunk.kind == b"tEXt"
            && chunk
                .data
                .iter()
                .position(|b| *b == 0)
                .is_some_and(|nul| keys.contains(&&chunk.data[..nul]));
        if !replaced {
            push_chunk(&mut out, &chunk.kind, chunk.data);
        }
        if index == 0 {
            for data in &added {
                push_chunk(&mut out, b"tEXt", data);
            }
        }
    }
    chunks(&out).is_some().then_some(out)
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    out.extend(kind);
    out.extend(data);
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(data);
    out.extend(crc.finalize().to_be_bytes());
}

/// A 1x1 transparent PNG: the body of a failure record (the standard asks
/// only that it carry the keys).
pub fn blank() -> Vec<u8> {
    // IHDR: 1x1, 8-bit, colour type 6 (RGBA). IDAT: zlib of one filter byte
    // and four zero bytes.
    let mut out = SIGNATURE.to_vec();
    push_chunk(&mut out, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    push_chunk(
        &mut out,
        b"IDAT",
        &[
            0x78, 0x01, 0x01, 0x05, 0x00, 0xfa, 0xff, 0, 0, 0, 0, 0, 0x00, 0x05, 0x00, 0x01,
        ],
    );
    push_chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blank_png_is_well_formed() {
        let png = blank();
        assert_eq!(size(&png), Some((1, 1)));
        assert!(text(&png, "Thumb::URI").is_none());
    }

    #[test]
    fn keys_are_set_after_ihdr_and_replaced() {
        let png = with_text(
            &blank(),
            &[("Thumb::URI", "file:///a"), ("Thumb::MTime", "7")],
        )
        .unwrap();
        assert_eq!(size(&png), Some((1, 1)));
        assert_eq!(text(&png, "Thumb::URI").as_deref(), Some("file:///a"));
        assert_eq!(text(&png, "Thumb::MTime").as_deref(), Some("7"));
        // Setting a key again replaces it rather than adding a second.
        let again = with_text(&png, &[("Thumb::MTime", "9")]).unwrap();
        assert_eq!(text(&again, "Thumb::MTime").as_deref(), Some("9"));
        assert_eq!(text(&again, "Thumb::URI").as_deref(), Some("file:///a"));
        assert_eq!(
            chunks(&again)
                .unwrap()
                .iter()
                .filter(|c| &c.kind == b"tEXt")
                .count(),
            2
        );
    }

    #[test]
    fn malformed_bytes_are_refused() {
        assert!(size(b"not a png").is_none());
        let mut cut = blank();
        cut.truncate(cut.len() - 6);
        assert!(size(&cut).is_none());
        assert!(with_text(&blank(), &[("Thumb::URI", "caf\u{0101}")]).is_none());
    }

    /// Only a whole PNG passes: a header alone, a bad CRC, a cut file or
    /// trailing bytes do not.
    #[test]
    fn only_whole_pngs_pass() {
        let png = blank();
        assert!(is_whole(&png));
        assert!(is_whole(
            &with_text(&png, &[("Thumb::URI", "file:///a")]).unwrap()
        ));
        // IHDR then IEND, no IDAT: what png::size alone accepts.
        let mut header_only = SIGNATURE.to_vec();
        push_chunk(
            &mut header_only,
            b"IHDR",
            &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0],
        );
        push_chunk(&mut header_only, b"IEND", &[]);
        assert_eq!(size(&header_only), Some((1, 1)));
        assert!(!is_whole(&header_only));
        let mut bad_crc = png.clone();
        let at = bad_crc.len() - 20; // inside IDAT's data
        bad_crc[at] ^= 0xff;
        assert!(!is_whole(&bad_crc));
        let mut cut = png.clone();
        cut.truncate(cut.len() - 4);
        assert!(!is_whole(&cut));
        let mut trailing = png.clone();
        trailing.push(0);
        assert!(!is_whole(&trailing));
        assert!(!is_whole(b"not a png"));
        // Too many chunks: refused, by both readers.
        let mut many = SIGNATURE.to_vec();
        push_chunk(&mut many, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        for _ in 0..MAX_CHUNKS {
            push_chunk(&mut many, b"tEXt", b"k\0v");
        }
        push_chunk(&mut many, b"IEND", &[]);
        assert!(!is_whole(&many));
        assert!(size(&many).is_none());
    }

    const RGBA_1X1: [u8; 13] = [0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0];

    /// A PNG of the given chunks (IHDR first as given, IEND added).
    fn build(chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let mut out = SIGNATURE.to_vec();
        for (kind, data) in chunks {
            push_chunk(&mut out, kind, data);
        }
        push_chunk(&mut out, b"IEND", &[]);
        out
    }

    /// IHDR's fields, the chunk order and the palette rules are checked:
    /// output that would pass by CRC alone, then fail to decode, is refused.
    #[test]
    fn only_legal_structure_passes() {
        let idat: &[u8] = &[0x78, 0x01];
        assert!(is_whole(&build(&[(b"IHDR", &RGBA_1X1), (b"IDAT", idat)])));
        let with_ihdr = |at: usize, value: u8| {
            let mut ihdr = RGBA_1X1;
            ihdr[at] = value;
            build(&[(b"IHDR", &ihdr), (b"IDAT", idat)])
        };
        assert!(!is_whole(&with_ihdr(10, 1)), "compression method 1");
        assert!(!is_whole(&with_ihdr(11, 1)), "filter method 1");
        assert!(!is_whole(&with_ihdr(12, 2)), "interlace method 2");
        assert!(!is_whole(&with_ihdr(8, 4)), "4-bit RGBA");
        assert!(!is_whole(&with_ihdr(9, 5)), "colour type 5");
        assert!(!is_whole(&with_ihdr(0, 0x80)), "width past 2^31-1");
        assert!(is_whole(&with_ihdr(12, 1)), "Adam7 is legal");
        // A second IHDR; image data split by another chunk.
        assert!(!is_whole(&build(&[
            (b"IHDR", &RGBA_1X1),
            (b"IHDR", &RGBA_1X1),
            (b"IDAT", idat),
        ])));
        assert!(!is_whole(&build(&[
            (b"IHDR", &RGBA_1X1),
            (b"IDAT", idat),
            (b"tEXt", b"k\0v"),
            (b"IDAT", idat),
        ])));
        assert!(is_whole(&build(&[
            (b"IHDR", &RGBA_1X1),
            (b"IDAT", idat),
            (b"IDAT", idat),
            (b"tEXt", b"k\0v"),
        ])));
        // Indexed colour needs its palette first; greyscale has none.
        let mut indexed = RGBA_1X1;
        indexed[9] = 3;
        let palette: &[u8] = &[0, 0, 0];
        assert!(is_whole(&build(&[
            (b"IHDR", &indexed),
            (b"PLTE", palette),
            (b"IDAT", idat),
        ])));
        assert!(!is_whole(&build(&[(b"IHDR", &indexed), (b"IDAT", idat)])));
        assert!(!is_whole(&build(&[
            (b"IHDR", &indexed),
            (b"IDAT", idat),
            (b"PLTE", palette),
        ])));
        let mut grey = RGBA_1X1;
        grey[9] = 0;
        assert!(!is_whole(&build(&[
            (b"IHDR", &grey),
            (b"PLTE", palette),
            (b"IDAT", idat),
        ])));
        // IEND carries no data.
        let mut iend_data = SIGNATURE.to_vec();
        push_chunk(&mut iend_data, b"IHDR", &RGBA_1X1);
        push_chunk(&mut iend_data, b"IDAT", idat);
        push_chunk(&mut iend_data, b"IEND", b"x");
        assert!(!is_whole(&iend_data));
    }

    /// The largest output that passes still reads back once the cache has
    /// added its keys; tagging past the limit is refused, not stored broken.
    #[test]
    fn whole_output_has_room_for_the_keys() {
        let idat: &[u8] = &[0x78, 0x01];
        let mut chunks: Vec<(&[u8; 4], &[u8])> = vec![(b"IHDR", &RGBA_1X1)];
        // IHDR, the IDATs and IEND: the most a thumbnailer may write.
        chunks.extend(std::iter::repeat_n(
            (b"IDAT", idat),
            MAX_CHUNKS - KEY_ROOM - 2,
        ));
        let largest = build(&chunks);
        assert!(is_whole(&largest));
        let keys = [
            ("Thumb::URI", "file:///a"),
            ("Thumb::MTime", "1"),
            ("Thumb::Size", "2"),
            ("Software", "dopus"),
            ("Thumb::Mimetype", "image/png"),
        ];
        let tagged = with_text(&largest, &keys).expect("room for the keys");
        assert_eq!(text(&tagged, "Thumb::URI").as_deref(), Some("file:///a"));
        chunks.push((b"IDAT", idat));
        assert!(!is_whole(&build(&chunks)), "one chunk too many");
        let mut full: Vec<(&[u8; 4], &[u8])> = vec![(b"IHDR", &RGBA_1X1)];
        full.extend(std::iter::repeat_n((b"IDAT", idat), MAX_CHUNKS - 2));
        assert!(size(&build(&full)).is_some(), "MAX_CHUNKS reads");
        assert!(
            with_text(&build(&full), &keys).is_none(),
            "past it does not"
        );
    }
}
