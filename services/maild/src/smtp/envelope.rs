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
        && (address_literal(domain) || dns_domain(domain).is_some())
}

pub(super) fn address_literal(domain: &str) -> bool {
    let Some(literal) = domain.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return false;
    };
    if literal
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("IPv6:"))
    {
        literal[5..].parse::<std::net::Ipv6Addr>().is_ok()
    } else {
        literal.parse::<std::net::Ipv4Addr>().is_ok()
    }
}

/// BODY=8BITMIME describes content after the top-level header block.
pub fn eight_bit_body(data: &[u8]) -> bool {
    let crlf = data
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4);
    let lf = data.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    let body = match (crlf, lf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    body.is_some_and(|start| !data[start..].is_ascii())
}

/// RFC 4954 trace identities are xtext, not authority to send as that mailbox.
/// Accept and discard them under the existing untrusted-client policy.
pub fn valid_auth_parameter(parameter: &str) -> bool {
    let Some((name, value)) = parameter.split_once('=') else {
        return false;
    };
    if !name.eq_ignore_ascii_case("AUTH") {
        return false;
    }
    let mut decoded = Vec::new();
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'+' => {
                let high = bytes.next().and_then(|b| (b as char).to_digit(16));
                let low = bytes.next().and_then(|b| (b as char).to_digit(16));
                let (Some(high), Some(low)) = (high, low) else {
                    return false;
                };
                decoded.push((high * 16 + low) as u8);
            }
            0x21..=0x7e if byte != b'=' => decoded.push(byte),
            _ => return false,
        }
    }
    std::str::from_utf8(&decoded)
        .is_ok_and(|mailbox| mailbox == "<>" || valid_address(mailbox, false))
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
    if data.is_ascii() {
        return false;
    }
    let end = data
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .into_iter()
        .chain(data.windows(2).position(|w| w == b"\n\n"))
        .min()
        .unwrap_or(data.len());
    if !data[..end].is_ascii() {
        return true;
    }
    // RFC 6531 section 3.6 includes MIME part headers. Body text alone
    // requires 8BITMIME, while a raw UTF-8 MIME header requires SMTPUTF8.
    let Some(message) = mail_parser::MessageParser::default().parse(data) else {
        return false;
    };
    let mut pending = vec![&message];
    while let Some(message) = pending.pop() {
        for part in &message.parts {
            if message
                .raw_message
                .get(part.offset_header as usize..part.offset_body as usize)
                .is_some_and(|header| !header.is_ascii())
            {
                return true;
            }
            if part.encoding == mail_parser::Encoding::None
                && let mail_parser::PartType::Message(inner) = &part.body
            {
                pending.push(inner);
            }
        }
    }
    false
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
