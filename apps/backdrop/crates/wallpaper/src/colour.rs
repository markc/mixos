// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! A solid colour as the document writes it: `#RRGGBB`, exactly six hex
//! digits, either case. Shorthand, alpha and names are refused.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    pub fn parse(text: &str) -> Result<Self, String> {
        let digits = text
            .strip_prefix('#')
            .filter(|d| d.len() == 6 && d.bytes().all(|b| b.is_ascii_hexdigit()));
        let Some(digits) = digits else {
            return Err(format!("{text:?} is not a colour as #RRGGBB"));
        };
        let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).map_err(|e| e.to_string());
        Ok(Self([channel(0)?, channel(2)?, channel(4)?]))
    }

    /// The colour as `#rrggbb`.
    pub fn hex(self) -> String {
        let [r, g, b] = self.0;
        format!("#{r:02x}{g:02x}{b:02x}")
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_is_hash_and_six_hex_digits() {
        assert_eq!(Rgb::parse("#0A1b2C"), Ok(Rgb([0x0a, 0x1b, 0x2c])));
        for bad in ["", "#", "0a1b2c", "#0a1b2", "#0a1b2c0", "#0a1b2g", "#0a1b2c ", "red", "#fff"] {
            assert!(Rgb::parse(bad).is_err(), "{bad:?} must be refused");
        }
        assert_eq!(Rgb([0x0a, 0x1b, 0x2c]).hex(), "#0a1b2c");
        assert_eq!(Rgb::parse(&Rgb([1, 2, 3]).hex()), Ok(Rgb([1, 2, 3])));
    }
}
