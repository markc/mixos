// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared SMTPUTF8 envelope and content requirements (RFC 6531).

pub fn valid_address(addr: &str, null_ok: bool) -> bool {
    if addr.is_empty() { return null_ok; }
    let Some((local, domain)) = addr.rsplit_once('@') else { return false; };
    !local.is_empty() && local.len() <= 64 && !domain.is_empty() && domain.len() <= 255
        && !local.contains('@')
        && !addr.chars().any(|c| c.is_control() || c.is_whitespace() || "<>\"()\\,;:".contains(c))
        && !local.starts_with('.') && !local.ends_with('.') && !local.contains("..")
        && domain.split('.').all(|label| !label.is_empty() && label.len() <= 63
            && !label.starts_with('-') && !label.ends_with('-')
            && label.chars().all(|c| c.is_alphanumeric() || c == '-'))
}

pub fn utf8_headers(data: &[u8]) -> bool {
    let end = data.windows(4).position(|w| w == b"\r\n\r\n")
        .or_else(|| data.windows(2).position(|w| w == b"\n\n"))
        .unwrap_or(data.len());
    !data[..end].is_ascii()
}

pub fn needs_smtputf8(from: &str, recipients: &[&String], data: &[u8]) -> bool {
    !from.is_ascii() || recipients.iter().any(|addr| !addr.is_ascii()) || utf8_headers(data)
}

pub fn has_extension(ehlo: &str, name: &str) -> bool {
    ehlo.lines().skip(1).any(|line| line.get(4..)
        .and_then(|line| line.split_ascii_whitespace().next())
        .is_some_and(|word| word.eq_ignore_ascii_case(name)))
}
