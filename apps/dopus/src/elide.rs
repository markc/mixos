// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Middle elision for file names and paths: the start and the extension
//! stay, the middle gives way to "…". Widths are measured with the real
//! font, never estimated from character counts, and the search is bounded.

use unicode_segmentation::UnicodeSegmentation;

/// `text` shortened in the middle to fit `width`, as `measure` measures it.
/// The extension (after the last `.` of the last path segment) is kept
/// whole when it is up to 12 graphemes long; a longer one keeps its end.
pub fn middle(text: &str, width: f32, mut measure: impl FnMut(&str) -> f32) -> String {
    if text.is_empty() || !width.is_finite() || width <= 0.0 {
        return String::new();
    }
    if measure(text) <= width {
        return text.to_owned();
    }
    if measure("…") > width {
        return String::new();
    }
    let boundaries: Vec<_> = text
        .grapheme_indices(true)
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let count = boundaries.len() - 1;
    if count <= 1 {
        return "…".into();
    }
    let extension = text
        .rfind('.')
        .filter(|i| *i > 0 && *i + 1 < text.len())
        .filter(|i| text.rfind('/').is_none_or(|slash| *i > slash + 1))
        .and_then(|i| boundaries.iter().position(|b| *b == i));
    let stem = extension.unwrap_or(count - 1);
    let suffix = extension.map_or(1, |i| count - i);
    let mut measures = 2;
    for keep in (1..=suffix.min(12)).rev() {
        let (mut low, mut high, mut best) = (0, stem, None);
        while low <= high {
            if measures >= 32 {
                return "…".into();
            }
            measures += 1;
            let prefix = low + (high - low) / 2;
            let candidate = if suffix > 12 {
                format!(
                    "{}….{}",
                    &text[..boundaries[prefix]],
                    &text[boundaries[count - keep + 1]..]
                )
            } else {
                format!(
                    "{}…{}",
                    &text[..boundaries[prefix]],
                    &text[boundaries[count - keep]..]
                )
            };
            if measure(&candidate) <= width {
                best = Some(candidate);
                low = prefix + 1;
            } else if prefix == 0 {
                break;
            } else {
                high = prefix - 1;
            }
        }
        if let Some(best) = best {
            return best;
        }
    }
    "…".into()
}

/// [`middle`] measured with `font` in `ui`.
pub fn fit(ui: &egui::Ui, text: &str, font: &egui::FontId, width: f32) -> String {
    let painter = ui.painter();
    middle(text, width, |s| {
        painter
            .layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
            .size()
            .x
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One unit per grapheme.
    fn chars(s: &str) -> f32 {
        s.graphemes(true).count() as f32
    }

    #[test]
    fn fits_are_untouched_and_the_extension_stays() {
        assert_eq!(middle("short.txt", 20.0, chars), "short.txt");
        let out = middle("a-very-long-file-name.txt", 12.0, chars);
        assert!(out.ends_with(".txt") && out.contains('…'), "{out}");
        assert!(chars(&out) <= 12.0, "{out}");
    }

    #[test]
    fn degenerate_widths() {
        assert_eq!(middle("abc", 0.0, chars), "");
        assert_eq!(middle("", 10.0, chars), "");
        assert_eq!(middle("abcdef", 1.0, chars), "…");
        assert_eq!(middle("abc", f32::NAN, chars), "");
    }

    #[test]
    fn graphemes_are_never_split() {
        let out = middle("👩‍👩‍👧‍👦👩‍👩‍👧‍👦👩‍👩‍👧‍👦👩‍👩‍👧‍👦.png", 4.0, chars);
        assert!(out.is_char_boundary(out.len()));
        assert!(out.contains('…'), "{out}");
    }

    #[test]
    fn a_dot_in_a_directory_is_not_an_extension() {
        let out = middle("/home/user/dir.d/long-name-without-ext", 12.0, chars);
        assert!(!out.ends_with(".d/long-name-without-ext"), "{out}");
        assert!(chars(&out) <= 12.0, "{out}");
    }
}
