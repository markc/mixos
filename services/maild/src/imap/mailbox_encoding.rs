// SPDX-License-Identifier: MIT OR Apache-2.0

//! Session wire encoding: modified UTF-7 until ENABLE switches to UTF-8.

use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};

pub fn decode(name: &str) -> Result<String, String> {
    if !name.is_ascii() {
        return Err("UTF-8 mailbox names require ENABLE IMAP4rev2 or UTF8=ACCEPT".into());
    }
    let mut result = String::new();
    let mut rest = name;
    while let Some(open) = rest.find('&') {
        result.push_str(&rest[..open]);
        rest = &rest[open + 1..];
        let close = rest.find('-').ok_or("unterminated modified UTF-7 shift")?;
        if close == 0 {
            result.push('&');
        } else {
            let encoded = rest[..close].replace(',', "/");
            let bytes = STANDARD_NO_PAD
                .decode(encoded)
                .map_err(|_| "invalid modified UTF-7")?;
            if bytes.len() % 2 != 0 {
                return Err("invalid modified UTF-7 UTF-16 length".into());
            }
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
                .collect();
            result.push_str(
                &String::from_utf16(&units).map_err(|_| "invalid modified UTF-7 UTF-16")?,
            );
        }
        rest = &rest[close + 1..];
    }
    result.push_str(rest);
    Ok(result)
}

pub fn encode(name: &str) -> String {
    let mut result = String::new();
    let mut shifted = Vec::new();
    fn flush(out: &mut String, bytes: &mut Vec<u8>) {
        if !bytes.is_empty() {
            out.push('&');
            out.push_str(&STANDARD_NO_PAD.encode(&*bytes).replace('/', ","));
            out.push('-');
            bytes.clear();
        }
    }
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut result, &mut shifted);
            result.push(c);
            if c == '&' {
                result.push('-');
            }
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                shifted.extend_from_slice(&unit.to_be_bytes());
            }
        }
    }
    flush(&mut result, &mut shifted);
    result
}

/// APPEND has one mailbox argument followed by flags and a date. Only the
/// mailbox uses modified UTF-7; keyword bytes must remain unchanged.
pub fn decode_first_mailbox(text: &str) -> Result<String, String> {
    let (name, rest) = super::op::list::take_mailbox_name(text.trim_start())?;
    Ok(format!(
        "{}{}",
        super::mailbox_name::quote_mailbox(&decode(&name)?),
        rest
    ))
}

/// Map a command's mailbox strings without changing its grammar. This is used
/// only for commands whose astring arguments are mailbox names or patterns.
pub fn map_tokens(text: &str, encode_wire: bool) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let c = rest.chars().next().unwrap();
        if c.is_ascii_whitespace() || c == '(' || c == ')' {
            out.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        }
        if c == '"' {
            let (value, tail) = super::op::list::take_mailbox_token(rest)?;
            let name = if encode_wire {
                encode(&value)
            } else {
                decode(&value)?
            };
            out.push_str(&super::mailbox_name::quote_mailbox(&name));
            rest = tail;
        } else {
            let end = rest
                .find(|c: char| c.is_ascii_whitespace() || c == '(' || c == ')')
                .unwrap_or(rest.len());
            let name = if encode_wire {
                encode(&rest[..end])
            } else {
                decode(&rest[..end])?
            };
            out.push_str(&name);
            rest = &rest[end..];
        }
    }
    Ok(out)
}

pub fn response(text: &str, utf8: bool) -> String {
    if utf8 {
        return text.to_string();
    }
    text.split_inclusive("\r\n")
        .map(|line| {
            if ["* LIST ", "* LSUB ", "* STATUS "]
                .iter()
                .any(|p| line.starts_with(p))
            {
                map_tokens(line, true).unwrap_or_else(|_| line.to_string())
            } else {
                line.to_string()
            }
        })
        .collect()
}
