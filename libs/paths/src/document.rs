// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded strict-data reader for the layout schema (five path strings).
use super::{Result, Root, absolute};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub(super) fn parse(source: &str) -> Result<BTreeMap<Root, PathBuf>> {
    if source.len() > 65536 {
        return Err("layout.mx exceeds 64 KiB".into());
    }
    let mut p = Reader {
        chars: source.chars().peekable(),
    };
    p.space();
    let braced = p.chars.peek() == Some(&'{');
    if braced {
        p.chars.next();
    }
    let mut out = BTreeMap::new();
    loop {
        p.space();
        if braced && p.chars.peek() == Some(&'}') {
            p.chars.next();
            p.space();
            if p.chars.next().is_some() {
                return Err("data after layout map".into());
            }
            return Ok(out);
        }
        if p.chars.peek().is_none() {
            return if braced {
                Err("unclosed layout map".into())
            } else {
                Ok(out)
            };
        }
        let key = if matches!(p.chars.peek(), Some('\'' | '"')) {
            p.string()?
        } else {
            let mut s = String::new();
            while p
                .chars
                .peek()
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_')
            {
                s.push(p.chars.next().unwrap());
            }
            s
        };
        let root = Root::parse(&key)?;
        if root == Root::Prefix {
            return Err("layout.mx refuses a prefix key; use MIXOS_PREFIX".into());
        }
        p.space();
        if p.chars.next() != Some(':') {
            return Err("expected ':' in layout.mx".into());
        }
        p.space();
        let path = PathBuf::from(p.string()?);
        absolute(&path)?;
        if out.insert(root, path).is_some() {
            return Err(format!("duplicate layout root: {key}"));
        }
        // Unlike a general document reader, values here can only be strings.
        let newline = p.space();
        if p.chars.peek() == Some(&',') {
            p.chars.next();
        } else if p.chars.peek().is_some() && p.chars.peek() != Some(&'}') && (braced || !newline) {
            return Err("expected layout entry separator".into());
        }
    }
}

struct Reader<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}
impl Reader<'_> {
    fn space(&mut self) -> bool {
        let mut newline = false;
        loop {
            while self.chars.peek().is_some_and(|c| c.is_whitespace()) {
                newline |= self.chars.next() == Some('\n');
            }
            let comment = self.chars.peek() == Some(&'#') || {
                let mut look = self.chars.clone();
                look.next() == Some('-') && look.next() == Some('-')
            };
            if !comment {
                return newline;
            }
            for c in self.chars.by_ref() {
                if c == '\n' {
                    newline = true;
                    break;
                }
            }
        }
    }
    fn string(&mut self) -> Result<String> {
        let quote = self.chars.next().ok_or("expected layout path string")?;
        if quote != '\'' && quote != '"' {
            return Err("layout paths must be quoted strings".into());
        }
        let mut out = String::new();
        while let Some(c) = self.chars.next() {
            if c == quote {
                return Ok(out);
            }
            if c == '\\' && quote == '"' {
                let c = self.chars.next().ok_or("unclosed layout escape")?;
                match c {
                    '\\' => out.push('\\'),
                    c if c == quote => out.push(c),
                    '$' | '~' if quote == '"' => out.push(c),
                    'n' if quote == '"' => out.push('\n'),
                    't' if quote == '"' => out.push('\t'),
                    'r' if quote == '"' => out.push('\r'),
                    'u' if quote == '"' => {
                        let mut digits = String::new();
                        if self.chars.peek() == Some(&'{') {
                            self.chars.next();
                            loop {
                                match self.chars.next() {
                                    Some('}') => break,
                                    Some(c) if c.is_ascii_hexdigit() && digits.len() < 6 => {
                                        digits.push(c)
                                    }
                                    _ => return Err("invalid layout Unicode escape".into()),
                                }
                            }
                        } else {
                            for _ in 0..4 {
                                digits.push(
                                    self.chars
                                        .next()
                                        .filter(char::is_ascii_hexdigit)
                                        .ok_or("invalid layout Unicode escape")?,
                                );
                            }
                        }
                        out.push(
                            u32::from_str_radix(&digits, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .ok_or("invalid layout Unicode scalar")?,
                        );
                    }
                    _ => {
                        out.push('\\');
                        out.push(c);
                    }
                }
            } else {
                if quote == '"'
                    && ((c == '$' && self.chars.peek() == Some(&'{'))
                        || (out.is_empty() && c == '~'))
                {
                    return Err("layout.mx is strict data; interpolation is refused".into());
                }
                out.push(c);
            }
        }
        Err("unclosed layout string".into())
    }
}
