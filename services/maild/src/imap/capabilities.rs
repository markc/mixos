// SPDX-License-Identifier: MIT OR Apache-2.0

//! Capabilities are an implementation allowlist. Configuration can select a
//! subset, but cannot advertise protocol features without an implementation.

pub const INITIAL: &[&str] = &[
    "IMAP4rev2", "IMAP4rev1", "SASL-IR", "AUTH=PLAIN", "AUTH=LOGIN", "ID",
    "NAMESPACE", "CHILDREN", "SPECIAL-USE", "CREATE-SPECIAL-USE", "UNSELECT",
    "LITERAL+", "UIDPLUS", "MOVE", "IDLE", "ENABLE", "UTF8=ACCEPT", "BINARY",
    "ESEARCH", "SEARCHRES", "LIST-EXTENDED", "LIST-STATUS", "STATUS=SIZE",
    "MULTIAPPEND",
];

pub fn render(caps: &[&str]) -> String { caps.join(" ") }

pub fn render_owned(caps: &[String]) -> String {
    caps.iter().filter_map(|cap| {
        let cap = cap.trim();
        INITIAL.iter().find(|supported| supported.eq_ignore_ascii_case(cap)).copied()
    }).collect::<Vec<_>>().join(" ")
}

/// Use the configured literal cap on CAPABILITY, greeting and authentication.
pub fn for_config(cfg: &super::config::ImapConfig) -> String {
    let body = match cfg.advertise_capabilities.as_ref() {
        Some(caps) => render_owned(caps),
        None => render(INITIAL),
    };
    format!("{body} APPENDLIMIT={}", cfg.max_literal_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overrides_cannot_advertise_missing_or_malformed_capabilities() {
        let caps = ["IMAP4rev2", "LITERAL+ ", "CONDSTORE", "QRESYNC", "IDLE", "X UNKNOWN", "AUTH=\0PLAIN"].map(str::to_string);
        assert_eq!(render_owned(&caps), "IMAP4rev2 LITERAL+ IDLE");
    }
    #[test]
    fn appendlimit_comes_from_configuration() {
        let cfg = super::super::config::ImapConfig { max_literal_bytes: 4096, ..Default::default() };
        assert!(for_config(&cfg).ends_with("APPENDLIMIT=4096"));
    }
}
