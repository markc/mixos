// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shared SMTPUTF8 envelope and content requirements (RFC 6531).

pub fn valid_address(addr: &str, null_ok: bool) -> bool {
    if addr.is_empty() {
        return null_ok;
    }
    let Some((local, domain)) = addr.rsplit_once('@') else {
        return false;
    };
    addr.len() <= 254
        && !local.is_empty()
        && local.len() <= 64
        && !domain.is_empty()
        && domain.len() <= 255
        && valid_local(local)
        && !addr.chars().any(char::is_control)
        && dns_domain(domain).is_some()
}

fn valid_local(local: &str) -> bool {
    if let Some(quoted) = local.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        let mut escaped = false;
        for c in quoted.chars() {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                return false;
            }
        }
        return !escaped;
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return false;
    }
    local.chars().all(|c| {
        if c.is_ascii() {
            c.is_ascii_alphanumeric() || ".!#$%&'*+-/=?^_`{|}~".contains(c)
        } else {
            !c.is_whitespace()
        }
    })
}

/// DNS and authoritative domain rows use IDNA A-labels; envelopes retain UTF-8.
pub fn dns_domain(domain: &str) -> Option<String> {
    if domain.starts_with('.') || domain.ends_with('.') {
        return None;
    }
    let name = hickory_resolver::Name::from_utf8(domain)
        .ok()?
        .to_ascii()
        .to_ascii_lowercase();
    if !name.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) {
        return None;
    }
    Some(name)
}

/// U-label and A-label domains identify the same mailbox. Keep the original
/// spelling first so stored addresses and message envelopes are never rewritten.
pub fn mailbox_addresses(addr: &str) -> Vec<String> {
    let mut addresses = vec![addr.to_string()];
    if let Some((local, domain)) = addr.rsplit_once('@')
        && let Some(ascii) = dns_domain(domain)
    {
        let unicode = hickory_resolver::Name::from_ascii(&ascii)
            .map(|name| name.to_utf8())
            .unwrap_or_else(|_| ascii.clone());
        for domain in [ascii, unicode] {
            let candidate = format!("{local}@{domain}");
            if !addresses.contains(&candidate) {
                addresses.push(candidate);
            }
        }
    }
    addresses
}

pub fn utf8_headers(data: &[u8]) -> bool {
    let end = data
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .or_else(|| data.windows(2).position(|w| w == b"\n\n"))
        .unwrap_or(data.len());
    !data[..end].is_ascii()
}

pub fn needs_smtputf8(from: &str, recipients: &[&String], data: &[u8]) -> bool {
    !from.is_ascii() || recipients.iter().any(|addr| !addr.is_ascii()) || utf8_headers(data)
}

pub fn has_extension(ehlo: &str, name: &str) -> bool {
    ehlo.lines().skip(1).any(|line| {
        line.get(4..)
            .and_then(|line| line.split_ascii_whitespace().next())
            .is_some_and(|word| word.eq_ignore_ascii_case(name))
    })
}
