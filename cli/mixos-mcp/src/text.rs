// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

/// Return the longest prefix within `max_bytes`, ending on a UTF-8 character boundary.
pub(crate) fn truncate_bytes(text: &str, max_bytes: usize) -> &str {
    &text[..text.floor_char_boundary(max_bytes)]
}

#[cfg(test)]
mod tests {
    use super::truncate_bytes;

    #[test]
    fn multibyte_characters_straddling_every_preview_limit() {
        for limit in [12, 120, 150, 200, 500, 2000] {
            for ch in ['—', '😀'] {
                for offset in 1..ch.len_utf8() {
                    let prefix = "a".repeat(limit - offset);
                    let text = format!("{prefix}{ch}tail");
                    assert_eq!(truncate_bytes(&text, limit), prefix);
                }
            }
        }
    }

    #[test]
    fn preserves_complete_characters_and_ascii_at_the_limit() {
        for limit in [12, 120, 150, 200, 500, 2000] {
            for ch in ['a', '—', '😀'] {
                let prefix = format!("{}{ch}", "a".repeat(limit - ch.len_utf8()));
                let text = format!("{prefix}tail");
                assert_eq!(truncate_bytes(&text, limit), prefix);
            }
        }
    }

    #[test]
    fn handles_empty_short_and_zero_limit_inputs() {
        assert_eq!(truncate_bytes("", 500), "");
        assert_eq!(truncate_bytes("—😀", 0), "");
        assert_eq!(truncate_bytes("—😀", 2), "");
        assert_eq!(truncate_bytes("—😀", 7), "—😀");
        assert_eq!(truncate_bytes("—😀", usize::MAX), "—😀");
    }
}
