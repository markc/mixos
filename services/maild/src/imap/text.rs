// SPDX-License-Identifier: MIT OR Apache-2.0

//! Streaming case-insensitive substring search with normalised line endings.

/// KMP keeps work linear in the input and allocates only for the needle.
/// `needle` has already been lowercased and CRLF-normalised by SEARCH.
pub fn contains(haystack: &str, needle: &str) -> bool {
    let pattern: Vec<char> = needle.chars().collect();
    if pattern.is_empty() { return true; }
    let mut prefix = vec![0; pattern.len()];
    let mut j = 0;
    for i in 1..pattern.len() {
        while j > 0 && pattern[i] != pattern[j] { j = prefix[j-1]; }
        if pattern[i] == pattern[j] { j += 1; }
        prefix[i] = j;
    }
    let mut chars = haystack.chars().peekable();
    j = 0;
    while let Some(c) = chars.next() {
        let c = if c == '\r' {
            if chars.peek() == Some(&'\n') { chars.next(); }
            '\n'
        } else { c };
        for c in c.to_lowercase() {
            while j > 0 && c != pattern[j] { j = prefix[j-1]; }
            if c == pattern[j] { j += 1; }
            if j == pattern.len() { return true; }
        }
    }
    false
}
