// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Address completion for the compose To, Cc and Bcc fields: pure
//! functions over text, no state and no I/O.
//!
//! Only the entry being typed is completed: the text after the last `,`
//! or `;` that is outside quotes and angle brackets, by the same rules
//! [`jmap::parse_addresses`] splits on ([`token`]). Suggestions come from
//! the account's address books first, then from the addresses the account
//! has seen (recent recipients and senders), ranked, deduplicated by
//! address, without the ones already in the field, at most [`LIMIT`]
//! ([`rank`]). Taking one replaces the entry with `Name <addr>, `, the name
//! quoted when RFC 5322 needs it ([`accept`]).

use serde::Serialize;

/// Most suggestions shown.
pub const LIMIT: usize = 8;

/// Characters typed before suggestions start.
pub const MIN_CHARS: usize = 2;

/// Where a suggestion came from; contacts rank first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Contact,
    Recent,
}

/// One address to offer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Suggestion {
    pub name: String,
    pub email: String,
    pub source: Source,
}

/// The entry being typed in `text`: its start (bytes) and the entry,
/// leading space trimmed.
pub fn token(text: &str) -> (usize, &str) {
    let (mut quoted, mut escaped, mut angle) = (false, false, false);
    let mut start = 0;
    for (at, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && c == '\\' {
            escaped = true;
        } else if c == '"' && !angle {
            quoted = !quoted;
        } else if !quoted && c == '<' {
            angle = true;
        } else if !quoted && c == '>' {
            angle = false;
        } else if !quoted && !angle && (c == ',' || c == ';') {
            start = at + c.len_utf8();
        }
    }
    let entry = &text[start..];
    let trimmed = entry.trim_start();
    (start + (entry.len() - trimmed.len()), trimmed)
}

/// The query to look up for `text`: its typed entry, once it has at
/// least [`MIN_CHARS`] characters.
pub fn query(text: &str) -> Option<&str> {
    let (_, entry) = token(text);
    let entry = entry.trim_end();
    (entry.chars().count() >= MIN_CHARS).then_some(entry)
}

/// `name` as RFC 5322 needs it in a display name: quoted (with `"` and
/// `\` escaped) when it holds a special character, as is otherwise.
pub fn display_name(name: &str) -> String {
    const SPECIALS: &[char] = &['(', ')', '<', '>', '[', ']', ':', ';', '@', '\\', ',', '.', '"'];
    if name.contains(SPECIALS) {
        let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        name.to_owned()
    }
}

/// One suggestion as the field writes it.
pub fn formatted(suggestion: &Suggestion) -> String {
    let name = suggestion.name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case(&suggestion.email) {
        suggestion.email.clone()
    } else {
        format!("{} <{}>", display_name(name), suggestion.email)
    }
}

/// `text` with its typed entry replaced by `suggestion` and a separator,
/// ready for the next address.
pub fn accept(text: &str, suggestion: &Suggestion) -> String {
    let (start, _) = token(text);
    let head = text[..start].trim_end();
    let entry = formatted(suggestion);
    if head.is_empty() {
        format!("{entry}, ")
    } else {
        format!("{head} {entry}, ")
    }
}

/// The addresses already in `text` (lower case), the typed entry aside.
pub fn present(text: &str) -> Vec<String> {
    let (start, _) = token(text);
    jmap::parse_addresses(&text[..start])
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.email.to_ascii_lowercase())
        .collect()
}

/// Whether `suggestion` matches `query`: a case-insensitive substring of
/// its name or address.
pub fn matches(suggestion: &Suggestion, query: &str) -> bool {
    let query = query.to_lowercase();
    suggestion.name.to_lowercase().contains(&query)
        || suggestion.email.to_lowercase().contains(&query)
}

/// Where `query` sits in `text` (bytes), case-insensitively, for the
/// highlight.
pub fn highlight(text: &str, query: &str) -> Option<std::ops::Range<usize>> {
    if query.is_empty() {
        return None;
    }
    let lower = text.to_lowercase();
    // Lower-casing can change byte lengths outside ASCII: only highlight
    // when it did not.
    if lower.len() != text.len() {
        return None;
    }
    let at = lower.find(&query.to_lowercase())?;
    Some(at..at + query.len())
}

/// The list to show for `query`: `contacts` (already the server's matches)
/// first, then the `recent` addresses that match, deduplicated by address,
/// without those in `present`, at most `limit`.
pub fn rank(
    contacts: Vec<Suggestion>,
    recent: &[Suggestion],
    query: &str,
    present: &[String],
    limit: usize,
) -> Vec<Suggestion> {
    let mut seen: Vec<String> = present.to_vec();
    let mut out = Vec::new();
    let recent = recent.iter().filter(|s| matches(s, query)).cloned();
    for suggestion in contacts.into_iter().chain(recent) {
        let key = suggestion.email.trim().to_ascii_lowercase();
        if key.is_empty() || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(suggestion);
        if out.len() == limit {
            break;
        }
    }
    out
}

/// One suggestion per address on `contact`'s card: the indexed address and
/// every JSContact `emails` entry.
pub fn from_contact(contact: &jmap::Contact) -> Vec<Suggestion> {
    let name = contact.display_name();
    let mut emails: Vec<String> = contact.email.iter().cloned().collect();
    if let Some(map) = contact.data.get("emails").and_then(|e| e.as_object()) {
        for entry in map.values() {
            if let Some(address) = entry.get("address").and_then(|a| a.as_str()) {
                emails.push(address.to_owned());
            }
        }
    }
    let mut out: Vec<Suggestion> = Vec::new();
    for email in emails {
        let email = email.trim().to_owned();
        if !email.is_empty() && !out.iter().any(|s| s.email.eq_ignore_ascii_case(&email)) {
            out.push(Suggestion {
                name: name.clone(),
                email,
                source: Source::Contact,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(name: &str, email: &str, source: Source) -> Suggestion {
        Suggestion {
            name: name.into(),
            email: email.into(),
            source,
        }
    }

    #[test]
    fn the_typed_entry_is_after_the_last_separator_outside_quotes() {
        assert_eq!(token("an"), (0, "an"));
        assert_eq!(token("bob@x.invalid, an"), (15, "an"));
        assert_eq!(token("bob@x.invalid; an"), (15, "an"));
        // A comma inside quotes or angle brackets is not a separator.
        assert_eq!(token("\"Smith, Ann\" <a@x.invalid>, cy").1, "cy");
        assert_eq!(token("\"Smith, An").1, "\"Smith, An");
        assert_eq!(token("bob@x.invalid, ").1, "");
        assert_eq!(query("bob@x.invalid, a"), None, "one character is too few");
        assert_eq!(query("bob@x.invalid, an "), Some("an"));
    }

    #[test]
    fn accepting_replaces_only_the_entry_and_adds_a_separator() {
        let ann = s("Ann Example", "ann@x.invalid", Source::Contact);
        assert_eq!(accept("an", &ann), "Ann Example <ann@x.invalid>, ");
        assert_eq!(
            accept("bob@x.invalid, an", &ann),
            "bob@x.invalid, Ann Example <ann@x.invalid>, "
        );
        // In the middle of a list the earlier entries stay as typed.
        assert_eq!(
            accept("\"Smith, Cy\" <cy@x.invalid>;  an", &ann),
            "\"Smith, Cy\" <cy@x.invalid>; Ann Example <ann@x.invalid>, "
        );
    }

    #[test]
    fn names_with_specials_are_quoted() {
        let smith = s("Smith, Ann", "ann@x.invalid", Source::Contact);
        assert_eq!(formatted(&smith), "\"Smith, Ann\" <ann@x.invalid>");
        let odd = s("Ann \"A\" O.", "ann@x.invalid", Source::Contact);
        assert_eq!(formatted(&odd), "\"Ann \\\"A\\\" O.\" <ann@x.invalid>");
        let bare = s("", "ann@x.invalid", Source::Recent);
        assert_eq!(formatted(&bare), "ann@x.invalid");
        // What accept writes parses back to the same address.
        let parsed = jmap::parse_addresses(&accept("", &smith)).unwrap();
        assert_eq!(parsed[0].email, "ann@x.invalid");
        assert_eq!(parsed[0].name.as_deref(), Some("Smith, Ann"));
    }

    #[test]
    fn contacts_rank_first_duplicates_and_present_addresses_go() {
        let contacts = vec![
            s("Ann Example", "ann@x.invalid", Source::Contact),
            s("Ann Work", "ann@work.invalid", Source::Contact),
        ];
        let recent = vec![
            s("Ann", "ANN@x.invalid", Source::Recent),
            s("Annabel", "annabel@y.invalid", Source::Recent),
            s("Bob", "bob@y.invalid", Source::Recent),
            s("", "anne@z.invalid", Source::Recent),
        ];
        let present = vec!["ann@work.invalid".to_owned()];
        let ranked = rank(contacts, &recent, "ann", &present, LIMIT);
        let emails: Vec<&str> = ranked.iter().map(|s| s.email.as_str()).collect();
        assert_eq!(emails, ["ann@x.invalid", "annabel@y.invalid", "anne@z.invalid"]);
        assert_eq!(ranked[0].source, Source::Contact);
        let many: Vec<Suggestion> = (0..20)
            .map(|n| s("Ann", &format!("ann{n}@x.invalid"), Source::Recent))
            .collect();
        assert_eq!(rank(Vec::new(), &many, "ann", &[], LIMIT).len(), LIMIT);
    }

    #[test]
    fn the_present_addresses_are_read_from_the_field() {
        assert_eq!(
            present("Bob <BOB@x.invalid>, cy@x.invalid, an"),
            ["bob@x.invalid", "cy@x.invalid"]
        );
    }

    #[test]
    fn the_match_is_highlighted() {
        assert_eq!(highlight("Ann Example", "exa"), Some(4..7));
        assert_eq!(highlight("Ann", "zz"), None);
    }

    #[test]
    fn a_card_gives_one_suggestion_per_address() {
        let contact: jmap::Contact = serde_json::from_value(serde_json::json!({
            "id": "P1",
            "addressBookId": "B1",
            "data": {
                "name": { "full": "Ann Example" },
                "emails": {
                    "e1": { "address": "ann@x.invalid" },
                    "e2": { "address": "ann@work.invalid" }
                }
            },
            "email": "ann@x.invalid"
        }))
        .unwrap();
        let list = from_contact(&contact);
        let emails: Vec<&str> = list.iter().map(|s| s.email.as_str()).collect();
        assert_eq!(emails, ["ann@x.invalid", "ann@work.invalid"]);
        assert!(list.iter().all(|s| s.name == "Ann Example"));
    }
}
