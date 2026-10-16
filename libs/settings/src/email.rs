// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The email app's settings (`email.*`): its JMAP accounts, in tree order,
//! the default From account and the mail list options.
//!
//! **Nothing here is secret, and nothing secret can be put here.** The
//! snapshot and summary are retained on the Bus for every client to read, so
//! an account has no password field, and every record refuses unknown keys:
//! an account object carrying `password` (or any other key) is refused by
//! `settings.apply`. Passwords live only in the email app's private secrets
//! file.
//!
//! Like [`crate::terminal`], the group is omitted from the serialised form at
//! its defaults (no accounts, no default, the default mail options), so a
//! profile written before it re-serialises byte for byte and its seals still
//! verify. It does not touch the resolved design.
use crate::model::{Diagnostic, Snapshot, Summary, deserialize_integer_u32, integer_u32};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ACCOUNTS_MAX: usize = 64;
pub const ID_MAX_BYTES: usize = 64;
/// The prefix [`mint_account_id`] gives a new id. Any valid id is accepted.
pub const ID_PREFIX: &str = "acct-";
pub const NAME_MAX_BYTES: usize = 128;
pub const URL_MAX_BYTES: usize = 2048;
pub const ADDRESS_MAX_BYTES: usize = 254;
pub const CA_PATH_MAX_BYTES: usize = 4096;
pub const SIGNATURE_MAX_CHARS: usize = 4096;
pub const PAGE_SIZE_MIN: u32 = 10;
pub const PAGE_SIZE_MAX: u32 = 500;

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

fn error(path: &str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new("validation_failed", path, message)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Email {
    /// The accounts, in the order the app's tree shows them; at most
    /// [`ACCOUNTS_MAX`], ids unique.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<AccountEntry>,
    /// The account new mail is sent from by default: an id in `accounts`, or
    /// None for the first enabled account.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_account: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    pub mail: MailOptions,
}

impl Email {
    pub fn is_default(&self) -> bool {
        is_default(self)
    }

    pub fn account(&self, id: &str) -> Option<&AccountEntry> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// The account compose sends from: the default account when it is set
    /// and enabled, else the first enabled account.
    pub fn from_account(&self) -> Option<&AccountEntry> {
        self.default_account
            .as_deref()
            .and_then(|id| self.account(id))
            .filter(|a| a.enabled)
            .or_else(|| self.accounts.iter().find(|a| a.enabled))
    }
}

/// The email settings of a snapshot.
pub fn from_snapshot(snapshot: &Snapshot) -> Email {
    snapshot.desktop.email.clone()
}

/// The email settings of a summary (defaults when an older settingsd omits
/// them).
pub fn from_summary(summary: &Summary) -> Email {
    summary.email.clone()
}

/// One JMAP account. Every field is written out; `ca_path`, `enabled` and
/// `signature` may be omitted on input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountEntry {
    /// Stable and unique: 1–64 of `a-z`, `0-9`, `-`; minted by the writer
    /// ([`mint_account_id`]) and never reused for another account.
    pub id: String,
    /// The label in the app's tree: 1–128 bytes, no control characters, no
    /// surrounding space.
    pub name: String,
    /// The JMAP base URL: `https://`, a host, no trailing slash.
    pub url: String,
    /// The login, and the default From address.
    pub email: String,
    /// An absolute path to a PEM CA bundle to trust for this account, or None
    /// for the system roots.
    #[serde(default)]
    pub ca_path: Option<String>,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
    /// Plain text appended to new drafts; at most [`SIGNATURE_MAX_CHARS`].
    #[serde(default)]
    pub signature: String,
}

fn enabled_default() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MailOptions {
    /// Messages fetched per list page, [`PAGE_SIZE_MIN`] to
    /// [`PAGE_SIZE_MAX`].
    #[serde(
        skip_serializing_if = "is_default_page_size",
        deserialize_with = "deserialize_integer_u32"
    )]
    pub page_size: u32,
    #[serde(skip_serializing_if = "is_true")]
    pub mark_read_on_open: bool,
    /// Ask before deleting messages.
    #[serde(skip_serializing_if = "is_true")]
    pub confirm_delete: bool,
}
fn is_default_page_size(size: &u32) -> bool {
    *size == 100
}
fn is_true(value: &bool) -> bool {
    *value
}
impl Default for MailOptions {
    fn default() -> Self {
        Self {
            page_size: 100,
            mark_read_on_open: true,
            confirm_delete: true,
        }
    }
}

/// Whether `id` is an account id this group takes.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_MAX_BYTES
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Whether `name` is an account label this group takes.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX_BYTES
        && !name.chars().any(char::is_control)
        && name.trim() == name
}

/// Whether `url` is a JMAP base this group takes: `https://`, a host, no
/// trailing slash, no space or control characters.
///
/// The authority is `host[:port]` and nothing else: userinfo
/// (`user:secret@host`, even with an empty user or secret) is refused, so a
/// password cannot ride into a retained snapshot inside the URL. So are a
/// query, a fragment and a backslash, which parsers disagree about.
pub fn valid_url(url: &str) -> bool {
    if url.len() > URL_MAX_BYTES
        || url.ends_with('/')
        || url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '?' | '#' | '\\'))
    {
        return false;
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split_once('/').map_or(rest, |(authority, _)| authority);
    valid_authority(authority)
}

/// `host[:port]`: a DNS name or IPv4 address of letters, digits, `-` and
/// `.`, or a bracketed IPv6 literal; the port 1 to 65535.
fn valid_authority(authority: &str) -> bool {
    if authority.contains('@') {
        return false;
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let Some((literal, after)) = v6.split_once(']') else {
            return false;
        };
        if literal.is_empty() || !literal.bytes().all(|b| b.is_ascii_hexdigit() || b == b':') {
            return false;
        }
        match after {
            "" => ("v6", None),
            _ => match after.strip_prefix(':') {
                Some(port) => ("v6", Some(port)),
                None => return false,
            },
        }
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let host_ok = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    let port_ok = port.is_none_or(|p| {
        !p.is_empty()
            && p.len() <= 5
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n))
    });
    host_ok && port_ok
}

/// Whether `address` is a login this group takes: one `@` with text on both
/// sides, no space or control characters. Whether it exists is the server's
/// check.
pub fn valid_address(address: &str) -> bool {
    address.len() <= ADDRESS_MAX_BYTES
        && address.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && !domain.is_empty() && !domain.contains('@')
        })
        && !address.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Whether `path` is a CA path this group takes: absolute, no NUL. Whether
/// it exists is the app's check.
pub fn valid_ca_path(path: &str) -> bool {
    path.starts_with('/') && path.len() <= CA_PATH_MAX_BYTES && !path.contains('\0')
}

/// Whether `signature` is a signature this group takes: at most
/// [`SIGNATURE_MAX_CHARS`], no control characters but newline and tab.
pub fn valid_signature(signature: &str) -> bool {
    signature.chars().count() <= SIGNATURE_MAX_CHARS
        && !signature
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
}

/// Check every account and option. A diagnostic about one account names it
/// by position: `email.accounts[2].url`.
pub fn validate(email: &Email) -> Result<(), Diagnostic> {
    if email.accounts.len() > ACCOUNTS_MAX {
        return Err(error(
            "email.accounts",
            format!("At most {ACCOUNTS_MAX} accounts"),
        ));
    }
    for (index, account) in email.accounts.iter().enumerate() {
        let path = |field: &str| format!("email.accounts[{index}].{field}");
        if !valid_id(&account.id) {
            return Err(error(
                &path("id"),
                "Use 1–64 lowercase ASCII letters, digits or dash",
            ));
        }
        if email.accounts[..index].iter().any(|a| a.id == account.id) {
            return Err(error(&path("id"), "Duplicate account id"));
        }
        if !valid_name(&account.name) {
            return Err(error(&path("name"), "Name must be a plain label"));
        }
        if !valid_url(&account.url) {
            return Err(error(
                &path("url"),
                "Use https://host[:port][/path]: no credentials, query, fragment or trailing slash",
            ));
        }
        if !valid_address(&account.email) {
            return Err(error(&path("email"), "Expected one address with one @"));
        }
        if let Some(ca) = &account.ca_path
            && !valid_ca_path(ca)
        {
            return Err(error(&path("ca_path"), "CA path must be absolute"));
        }
        if !valid_signature(&account.signature) {
            return Err(error(
                &path("signature"),
                format!("At most {SIGNATURE_MAX_CHARS} characters of plain text"),
            ));
        }
    }
    if let Some(id) = &email.default_account
        && email.account(id).is_none()
    {
        return Err(error(
            "email.default_account",
            "Default account must name an account",
        ));
    }
    if !(PAGE_SIZE_MIN..=PAGE_SIZE_MAX).contains(&email.mail.page_size) {
        return Err(error(
            "email.mail.page_size",
            format!("Expected {PAGE_SIZE_MIN}..{PAGE_SIZE_MAX}"),
        ));
    }
    Ok(())
}

/// A new account id, unique among `existing`: [`ID_PREFIX`] and twelve hex
/// digits from the clock and the process.
pub fn mint_account_id(existing: &[AccountEntry]) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    mint_account_id_from(existing, nanos ^ (u64::from(std::process::id()) << 32))
}

/// [`mint_account_id`] from a given seed: the same seed and accounts give
/// the same id.
pub fn mint_account_id_from(existing: &[AccountEntry], mut seed: u64) -> String {
    loop {
        let hash = blake3::hash(&seed.to_le_bytes()).to_hex();
        let id = format!("{ID_PREFIX}{}", &hash[..12]);
        if !existing.iter().any(|a| a.id == id) {
            return id;
        }
        seed = seed.wrapping_add(1);
    }
}

/// The `describe()` entries, one per field.
pub fn describe() -> Value {
    serde_json::json!({
        "email": {"type":"email", "null":"every email setting back to its default", "secrets":"none: passwords never enter settingsd"},
        "email.accounts": {"type":"list", "of":"account", "max_items":ACCOUNTS_MAX, "default":[], "applies":"the whole list at once, in tree order",
            "account": {
                "id": {"type":"string", "pattern":"[a-z0-9-]{1,64}", "unique":true, "minted":"acct- and 12 hex digits"},
                "name": {"type":"string", "max_bytes":NAME_MAX_BYTES},
                "url": {"type":"string", "prefix":"https://", "trailing_slash":false, "userinfo":false, "query":false, "fragment":false, "max_bytes":URL_MAX_BYTES},
                "email": {"type":"string", "max_bytes":ADDRESS_MAX_BYTES, "one":"@"},
                "ca_path": {"type":"string|null", "absolute":true, "max_bytes":CA_PATH_MAX_BYTES, "default":null, "null":"the system roots"},
                "enabled": {"type":"boolean", "default":true},
                "signature": {"type":"string", "max_chars":SIGNATURE_MAX_CHARS, "default":""}
            }, "unknown_keys":"refused"},
        "email.default_account": {"type":"string|null", "names":"an id in email.accounts", "default":null, "null":"the first enabled account"},
        "email.mail.page_size": {"type":"integer", "minimum":PAGE_SIZE_MIN, "maximum":PAGE_SIZE_MAX, "default":100},
        "email.mail.mark_read_on_open": {"type":"boolean", "default":true},
        "email.mail.confirm_delete": {"type":"boolean", "default":true}
    })
}

fn typed<T: serde::de::DeserializeOwned>(path: &str, value: Value) -> Result<T, Diagnostic> {
    serde_json::from_value(value).map_err(|e| error(path, e.to_string()))
}

/// A whole number, as Mix data sends it (100 or 100.0).
fn whole(path: &str, value: Value) -> Result<u32, Diagnostic> {
    integer_u32(typed::<f64>(path, value)?)
        .ok_or_else(|| error(path, "Expected an integral u32 number"))
}

/// Set (Some) or reset (None) one field. `field` is the path after `email`;
/// empty is the whole group.
pub(crate) fn set(
    email: &mut Email,
    path: &str,
    field: &[&str],
    value: Option<Value>,
) -> Result<(), Diagnostic> {
    let defaults = MailOptions::default();
    match field {
        [] => {
            *email = value
                .filter(|v| !v.is_null())
                .map_or(Ok(Email::default()), |v| typed(path, v))?
        }
        ["accounts"] => email.accounts = value.map_or(Ok(Vec::new()), |v| typed(path, v))?,
        ["default_account"] => email.default_account = typed(path, value.unwrap_or(Value::Null))?,
        ["mail", "page_size"] => {
            email.mail.page_size = value.map_or(Ok(defaults.page_size), |v| whole(path, v))?
        }
        ["mail", "mark_read_on_open"] => {
            email.mail.mark_read_on_open =
                value.map_or(Ok(defaults.mark_read_on_open), |v| typed(path, v))?
        }
        ["mail", "confirm_delete"] => {
            email.mail.confirm_delete =
                value.map_or(Ok(defaults.confirm_delete), |v| typed(path, v))?
        }
        _ => return Err(error(path, "Unknown field or scope")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Desktop;
    use crate::resolve::{patch, resolve};
    use serde_json::json;
    use std::collections::BTreeMap;

    /// Apply one change and resolve: Ok when the profile accepts it.
    fn apply(path: &str, value: Value) -> Result<Desktop, Diagnostic> {
        apply_to(&Desktop::default(), path, value)
    }

    fn apply_to(base: &Desktop, path: &str, value: Value) -> Result<Desktop, Diagnostic> {
        let next = patch(base, &BTreeMap::from([(path.into(), value)]), &[])?;
        resolve(&next).map_err(|mut errors| errors.remove(0))?;
        Ok(next)
    }

    fn account(id: &str) -> Value {
        json!({"id":id, "name":"Gold Coast", "url":"https://mail.example.org", "email":"admin@example.org"})
    }

    #[test]
    fn an_old_profile_re_serialises_byte_for_byte() {
        let old = r#"{"appearance":{"scheme":"forest","mode":"dark","contrast":"normal","source":null},"ui":{"density":1.0,"text_scale":1.0,"reduced_motion":false},"shell":{"panels":{"bottom":{"edge":"bottom","mode":"dock","thickness":40}},"page_order":[]},"apps":{},"terminal":{"padding":1}}"#;
        let desktop: Desktop = serde_json::from_str(old).unwrap();
        assert!(desktop.email.is_default());
        assert_eq!(serde_json::to_string(&desktop).unwrap(), old);
        assert!(
            !serde_json::to_string(&Desktop::default())
                .unwrap()
                .contains("email"),
            "omitted at its defaults"
        );
    }

    #[test]
    fn defaults_round_trip() {
        let e = Email::default();
        assert!(e.accounts.is_empty());
        assert_eq!(e.default_account, None);
        assert_eq!(
            (
                e.mail.page_size,
                e.mail.mark_read_on_open,
                e.mail.confirm_delete
            ),
            (100, true, true)
        );
        validate(&e).unwrap();
        let full = json!({"accounts":[], "default_account":null,
            "mail":{"page_size":100, "mark_read_on_open":true, "confirm_delete":true}});
        let read: Email = serde_json::from_value(full).unwrap();
        assert_eq!(read, e);
        assert_eq!(serde_json::to_value(&read).unwrap(), json!({}));
        // An account takes its optional fields' defaults and writes every
        // field out; Mix data reads the same.
        let read: AccountEntry = serde_json::from_value(account("acct-1")).unwrap();
        assert_eq!(
            (
                read.ca_path.as_deref(),
                read.enabled,
                read.signature.as_str()
            ),
            (None, true, "")
        );
        let changed = Email {
            accounts: vec![read],
            default_account: Some("acct-1".into()),
            mail: MailOptions {
                page_size: 50,
                confirm_delete: false,
                ..Default::default()
            },
        };
        let text = serde_json::to_string(&changed).unwrap();
        assert_eq!(
            text,
            r#"{"accounts":[{"id":"acct-1","name":"Gold Coast","url":"https://mail.example.org","email":"admin@example.org","ca_path":null,"enabled":true,"signature":""}],"default_account":"acct-1","mail":{"page_size":50,"confirm_delete":false}}"#
        );
        assert_eq!(serde_json::from_str::<Email>(&text).unwrap(), changed);
        assert_eq!(
            strict::from_str::<Email>(
                r#"{accounts: [{id: "acct-1", name: "Gold Coast", url: "https://mail.example.org", email: "admin@example.org"}], default_account: "acct-1", mail: {page_size: 50.0, confirm_delete: false}}"#
            )
            .unwrap(),
            changed
        );
        // Each path resets to its default, and the group as a whole.
        let desktop = Desktop {
            email: changed,
            ..Default::default()
        };
        let paths = [
            "email.accounts",
            "email.default_account",
            "email.mail.page_size",
            "email.mail.mark_read_on_open",
            "email.mail.confirm_delete",
        ];
        let reset: Vec<String> = paths.iter().map(|p| (*p).into()).collect();
        assert_eq!(
            patch(&desktop, &BTreeMap::new(), &reset).unwrap(),
            Desktop::default()
        );
        assert_eq!(
            patch(&desktop, &BTreeMap::new(), &["email".into()]).unwrap(),
            Desktop::default(),
            "the whole group"
        );
        let reset = patch(
            &desktop,
            &BTreeMap::from([("email".into(), Value::Null)]),
            &[],
        )
        .unwrap();
        assert_eq!(reset, Desktop::default(), "null is the group's reset");
        let described = crate::describe();
        assert_eq!(described["fields"]["email.mail.page_size"]["default"], 100);
        for path in paths.into_iter().chain(["email"]) {
            assert!(described["fields"].get(path).is_some(), "{path}");
        }
    }

    #[test]
    fn each_path_applies() {
        let one = apply("email.accounts", json!([account("acct-a"), account("b-2")])).unwrap();
        assert_eq!(
            one.email
                .accounts
                .iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>(),
            ["acct-a", "b-2"],
            "in the order given"
        );
        let two = apply_to(&one, "email.default_account", json!("b-2")).unwrap();
        assert_eq!(two.email.default_account.as_deref(), Some("b-2"));
        assert_eq!(two.email.from_account().unwrap().id, "b-2");
        let three = apply_to(&two, "email.default_account", Value::Null).unwrap();
        assert_eq!(three.email.default_account, None);
        assert_eq!(three.email.from_account().unwrap().id, "acct-a");
        let four = apply_to(&three, "email.mail.page_size", json!(250.0)).unwrap();
        assert_eq!(four.email.mail.page_size, 250);
        let five = apply_to(&four, "email.mail.mark_read_on_open", json!(false)).unwrap();
        assert!(!five.email.mail.mark_read_on_open);
        let six = apply_to(&five, "email.mail.confirm_delete", json!(false)).unwrap();
        assert!(!six.email.mail.confirm_delete);
        // The list is replaced, never merged.
        let seven = apply_to(&six, "email.accounts", json!([account("b-2")])).unwrap();
        assert_eq!(seven.email.accounts.len(), 1);
        // The design is untouched.
        assert_eq!(
            crate::digest(&resolve(&seven).unwrap()).unwrap(),
            crate::digest(&resolve(&Desktop::default()).unwrap()).unwrap()
        );
        // A disabled default falls back to the first enabled account.
        let mut email = seven.email.clone();
        email
            .accounts
            .insert(0, serde_json::from_value(account("first")).unwrap());
        email.accounts[1].enabled = false;
        email.default_account = Some("b-2".into());
        assert_eq!(email.from_account().unwrap().id, "first");
    }

    #[test]
    fn boundaries_are_accepted_and_one_step_outside_refused() {
        for (path, inside, outside) in [
            (
                "email.mail.page_size",
                vec![json!(10), json!(500), json!(100.0)],
                vec![
                    json!(9),
                    json!(501),
                    json!(100.5),
                    json!("100"),
                    json!(null),
                ],
            ),
            (
                "email.mail.mark_read_on_open",
                vec![json!(true), json!(false)],
                vec![json!("yes"), json!(1)],
            ),
            (
                "email.mail.confirm_delete",
                vec![json!(true), json!(false)],
                vec![json!("no"), json!(0)],
            ),
            (
                "email.default_account",
                vec![json!(null)],
                vec![json!("acct-missing"), json!(1)],
            ),
        ] {
            for value in inside {
                assert!(
                    apply(path, value.clone()).is_ok(),
                    "{path} = {value} accepted"
                );
            }
            for value in outside {
                let refused = apply(path, value.clone());
                assert_eq!(
                    refused.as_ref().map(|_| ()).unwrap_err().path,
                    path,
                    "{path} = {value} refused at its own path"
                );
            }
        }
    }

    #[test]
    fn accounts_are_validated_field_by_field() {
        let with = |field: &str, value: Value| {
            let mut a = account("acct-1");
            a[field] = value;
            apply("email.accounts", json!([a]))
        };
        for (field, inside, outside) in [
            (
                "id",
                vec![json!("a"), json!("acct-0f3a"), json!("x".repeat(64))],
                vec![
                    json!(""),
                    json!("Acct"),
                    json!("acct_1"),
                    json!("acct 1"),
                    json!("x".repeat(65)),
                ],
            ),
            (
                "name",
                vec![json!("Gold Coast"), json!("x".repeat(128))],
                vec![
                    json!(""),
                    json!(" Gold"),
                    json!("Gold\n"),
                    json!("x".repeat(129)),
                ],
            ),
            (
                "url",
                vec![
                    json!("https://mail.example.org"),
                    json!("https://mail.example.org:8443/jmap"),
                    json!("https://192.0.2.1"),
                    json!("https://[2001:db8::1]:443/jmap"),
                    json!("https://mail.example.org/a@b"),
                ],
                vec![
                    json!("http://mail.example.org"),
                    json!("https://mail.example.org/"),
                    json!("https://"),
                    json!("https:///path"),
                    json!("https://mail example.org"),
                    json!("mail.example.org"),
                    json!("https://user:pw@mail.example.org"),
                    json!("https://@mail.example.org"),
                    json!("https://mail.example.org?token=x"),
                    json!("https://mail.example.org/jmap#frag"),
                    json!("https://mail.example.org\\@evil.example"),
                    json!("https://mail.example.org:0"),
                    json!("https://mail.example.org:65536"),
                    json!("https://mail.example.org:"),
                    json!("https://mail.example.org:x"),
                    json!("https://[2001:db8::1"),
                    json!("https://mail.example.org\u{7f}"),
                ],
            ),
            (
                "email",
                vec![json!("admin@example.org"), json!("a@b")],
                vec![
                    json!(""),
                    json!("admin"),
                    json!("@example.org"),
                    json!("admin@"),
                    json!("a@b@c"),
                    json!("ad min@example.org"),
                ],
            ),
            (
                "ca_path",
                vec![json!(null), json!("/etc/ssl/ca.pem")],
                vec![json!("ca.pem"), json!("./ca.pem"), json!("")],
            ),
            (
                "signature",
                vec![
                    json!(""),
                    json!("-- \nMark\tGold Coast"),
                    json!("é".repeat(4096)),
                ],
                vec![json!("x".repeat(4097)), json!("bell\u{7}")],
            ),
        ] {
            let at = format!("email.accounts[0].{field}");
            for value in inside {
                assert!(with(field, value.clone()).is_ok(), "{field} = {value}");
            }
            for value in outside {
                let refused = with(field, value.clone());
                assert_eq!(
                    refused.map(|_| ()).unwrap_err().path,
                    at,
                    "{field} = {value} refused at its own path"
                );
            }
        }
        // Ids are unique; the list is bounded.
        let refused = apply(
            "email.accounts",
            json!([account("acct-1"), account("acct-2"), account("acct-1")]),
        )
        .unwrap_err();
        assert_eq!(refused.path, "email.accounts[2].id");
        let many: Vec<Value> = (0..ACCOUNTS_MAX)
            .map(|i| account(&format!("a{i}")))
            .collect();
        assert!(apply("email.accounts", json!(many)).is_ok());
        let too_many: Vec<Value> = (0..=ACCOUNTS_MAX)
            .map(|i| account(&format!("a{i}")))
            .collect();
        assert_eq!(
            apply("email.accounts", json!(too_many)).unwrap_err().path,
            "email.accounts"
        );
        // Required fields are required.
        let mut a = account("acct-1");
        a.as_object_mut().unwrap().remove("url");
        assert!(apply("email.accounts", json!([a])).is_err());
    }

    #[test]
    fn a_password_never_enters_the_profile() {
        let mut a = account("acct-1");
        a["password"] = json!("hunter2");
        let refused = apply("email.accounts", json!([a.clone()])).unwrap_err();
        assert_eq!(refused.path, "email.accounts");
        assert!(refused.message.contains("password"), "{}", refused.message);
        // Nor through the whole group, nor anywhere else in it.
        assert!(apply("email", json!({"accounts":[a]})).is_err());
        assert!(apply("email", json!({"password":"hunter2"})).is_err());
        assert!(apply("email.password", json!("hunter2")).is_err());
        assert!(apply("email.accounts.0.password", json!("hunter2")).is_err());
        assert!(
            serde_json::from_value::<Desktop>(json!({"email":{"mail":{"password":"x"}}})).is_err()
        );
    }

    #[test]
    fn a_password_in_url_userinfo_is_refused_without_echoing_it() {
        for url in [
            "https://user:hunter2@mail.example.org",
            "https://:hunter2@mail.example.org",
            "https://user:@mail.example.org",
            "https://hunter2@mail.example.org/jmap",
            "https://user:hunter2@mail.example.org:8443",
        ] {
            let mut a = account("acct-1");
            a["url"] = json!(url);
            let refused = apply("email.accounts", json!([a])).unwrap_err();
            assert_eq!(refused.path, "email.accounts[0].url", "{url}");
            let shown = serde_json::to_string(&refused).unwrap();
            assert!(
                !shown.contains("hunter2") && !shown.contains("user:"),
                "{shown}"
            );
        }
    }

    #[test]
    fn unknown_keys_and_scopes_are_refused() {
        for path in [
            "email.mail",
            "email.signature",
            "email.accounts.0",
            "email.mail.typo",
        ] {
            assert!(apply(path, json!(1)).is_err(), "{path}");
        }
        assert!(apply("email", json!({"mail":{"page_size":20}})).is_ok());
        assert!(apply("email", json!({"mail":{"page_size":5}})).is_err());
        // Removing the default account needs the default changed in the
        // same batch.
        let one = apply(
            "email",
            json!({"accounts":[account("a")], "default_account":"a"}),
        )
        .unwrap();
        assert_eq!(
            apply_to(&one, "email.accounts", json!([]))
                .unwrap_err()
                .path,
            "email.default_account"
        );
        let both = patch(
            &one,
            &BTreeMap::from([("email.accounts".into(), json!([]))]),
            &["email.default_account".into()],
        )
        .unwrap();
        resolve(&both).unwrap();
    }

    #[test]
    fn minted_ids_are_valid_unique_and_reproducible() {
        let id = mint_account_id(&[]);
        assert!(id.starts_with(ID_PREFIX) && valid_id(&id), "{id}");
        assert_eq!(id.len(), ID_PREFIX.len() + 12);
        let first = mint_account_id_from(&[], 7);
        assert_eq!(first, mint_account_id_from(&[], 7), "deterministic");
        let taken: AccountEntry = serde_json::from_value(account(&first)).unwrap();
        let second = mint_account_id_from(std::slice::from_ref(&taken), 7);
        assert_ne!(second, first, "a taken id is skipped");
        assert_eq!(second, mint_account_id_from(&[], 8));
        assert_ne!(mint_account_id_from(&[], u64::MAX), "");
    }

    #[test]
    fn the_helpers_read_snapshots_and_summaries() {
        let desktop = apply("email.accounts", json!([account("acct-1")])).unwrap();
        let effective = resolve(&desktop).unwrap();
        let snapshot = Snapshot {
            schema: crate::SCHEMA,
            binding: crate::Binding {
                instance: "test".into(),
                profile: "default".into(),
            },
            incarnation: "i".into(),
            revision: crate::Revision(2),
            design_revision: crate::Revision(1),
            source_digest: "s".into(),
            desktop,
            effective,
        };
        assert_eq!(from_snapshot(&snapshot).accounts[0].id, "acct-1");
        let summary = Summary::of(&snapshot);
        assert_eq!(from_summary(&summary), from_snapshot(&snapshot));
        // An older settingsd's summary has no email: the defaults.
        let mut value = serde_json::to_value(&summary).unwrap();
        value.as_object_mut().unwrap().remove("email");
        let older: Summary = serde_json::from_value(value).unwrap();
        assert!(from_summary(&older).is_default());
    }
}
