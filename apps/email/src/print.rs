// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Printing an email. MixOS has no print service, so the email becomes one
//! self-contained HTML page that the browser opens and prints through its
//! own dialog (which also offers Save to PDF).
//!
//! [`page`] is pure: a header block (From, To, Cc, Date, Subject), then the
//! body as the reader shows it — Markdown rendered by pulldown-cmark, plain
//! text escaped and wrapped. Nothing in the message reaches the page as
//! markup: raw HTML (a Markdown body's, or an HTML-only body) is escaped,
//! images are never fetched (their alt text stands in), and only http,
//! https and mailto links stay links. A CSP meta allows no source at all
//! beyond the page's own inline style and its one `window.print()` call.
//!
//! [`write_page`] puts the page in `$XDG_RUNTIME_DIR/email-print/` (the
//! directory 0700, the file 0600) and prunes pages older than a day; the
//! shell runs it off the UI thread and hands the file to the browser.

use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use jmap::{Email, EmailAddress};
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};

use crate::reader::Format;

/// The page's Content-Security-Policy: no fetches of any kind, only the
/// inline style and the inline print call.
pub const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'";

/// The browser's desktop id (`apps.list`); `apps.launch` opens the page.
pub const BROWSER: &str = "firefox";

/// How long a printed page is kept before a later print prunes it.
pub const KEEP: Duration = Duration::from_secs(24 * 60 * 60);

/// The runtime directory's subdirectory the pages are written to.
pub const DIR: &str = "email-print";

/// `text` with the five HTML-significant characters escaped; safe in an
/// element and in a quoted attribute.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// An address list as a header shows it: `Name <addr>`, or the address.
fn addresses(list: Option<&Vec<EmailAddress>>) -> String {
    list.map(|list| {
        list.iter()
            .map(|a| match a.name.as_deref().map(str::trim) {
                Some(name) if !name.is_empty() => format!("{name} <{}>", a.email),
                _ => a.email.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    })
    .unwrap_or_default()
}

/// `date`, else `receivedAt`, in full local time; the raw value when it is
/// not RFC 3339.
fn date(email: &Email) -> String {
    let Some(raw) = email.date.as_deref().or(email.received_at.as_deref()) else {
        return String::new();
    };
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%a %-d %b %Y %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| raw.to_owned())
}

/// Markdown as HTML with nothing from the message passed through: raw HTML
/// becomes text (a block as preformatted text), an image becomes its alt
/// text, and a link whose address is not http, https or mailto becomes its
/// text.
pub fn markdown(source: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    // Per open link: whether it was kept.
    let mut links: Vec<bool> = Vec::new();
    let events = Parser::new_ext(source, options).filter_map(|event| match event {
        Event::Start(Tag::HtmlBlock) => Some(Event::Start(Tag::CodeBlock(CodeBlockKind::Indented))),
        Event::End(TagEnd::HtmlBlock) => Some(Event::End(TagEnd::CodeBlock)),
        Event::Html(html) | Event::InlineHtml(html) => Some(Event::Text(html)),
        // The alt text between them is ordinary text events.
        Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => None,
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let keep = toolkit::markdown::openable(&dest_url);
            links.push(keep);
            keep.then_some(Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                id,
            }))
        }
        Event::End(TagEnd::Link) => links
            .pop()
            .unwrap_or(false)
            .then_some(Event::End(TagEnd::Link)),
        Event::InlineMath(text) | Event::DisplayMath(text) => Some(Event::Code(text)),
        Event::FootnoteReference(label) => Some(Event::Text(CowStr::from(format!("[{label}]")))),
        other => Some(other),
    });
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events);
    out
}

/// The page's light print styles at the reader's text size, in points.
fn style(points: f64) -> String {
    // System colours: the print is light whatever the window's theme, and
    // the page names no colour of its own.
    format!(
        "@page {{ margin: 18mm 16mm; }}\n\
         :root {{ color-scheme: light; }}\n\
         html {{ background: Canvas; color: CanvasText; }}\n\
         body {{ font-family: sans-serif; font-size: {points}pt; line-height: 1.45; margin: 0; }}\n\
         .headers {{ border-bottom: 1px solid GrayText; margin-bottom: 1.2em; padding-bottom: 0.6em; }}\n\
         .headers h1 {{ font-size: 1.35em; margin: 0 0 0.5em; }}\n\
         .headers table {{ border-collapse: collapse; }}\n\
         .headers th {{ color: GrayText; font-weight: normal; padding: 0 1em 0.15em 0; text-align: left; vertical-align: top; white-space: nowrap; }}\n\
         .headers td {{ overflow-wrap: anywhere; padding: 0 0 0.15em; }}\n\
         .plain, pre {{ font-family: monospace; font-size: 0.92em; overflow-wrap: anywhere; white-space: pre-wrap; }}\n\
         .plain {{ font-family: inherit; font-size: 1em; margin: 0; }}\n\
         .note {{ color: GrayText; font-style: italic; }}\n\
         blockquote {{ border-left: 2px solid GrayText; margin-left: 0; padding-left: 0.9em; }}\n\
         table {{ border-collapse: collapse; }}\n\
         .body th, .body td {{ border: 1px solid GrayText; padding: 0.2em 0.5em; }}\n\
         a {{ color: inherit; }}\n\
         img {{ display: none; }}\n"
    )
}

/// The whole page for `email`, its body shown as `format` (the reader's),
/// at `points` (the reader's text size).
pub fn page(email: &Email, format: Format, points: f64) -> String {
    let subject = email.subject.as_deref().unwrap_or("");
    let mut rows = String::new();
    for (key, value) in [
        ("from", addresses(email.from.as_ref())),
        ("to", addresses(email.to.as_ref())),
        ("cc", addresses(email.cc.as_ref())),
        ("print-date", date(email)),
        ("subject", subject.to_owned()),
    ] {
        if value.is_empty() {
            continue;
        }
        rows.push_str(&format!(
            "<tr><th>{}</th><td>{}</td></tr>\n",
            escape(&crate::label(key)),
            escape(&value)
        ));
    }
    let body = match email.text_body_value() {
        Some(text) if format == Format::Markdown && crate::reader::has_text(email) => {
            markdown(text)
        }
        Some(text) if crate::reader::has_text(email) => {
            format!("<pre class=\"plain\">{}</pre>", escape(text))
        }
        // An HTML body (served as the text body, or the only body) is its
        // source, escaped, as the reader shows it.
        Some(html) => html_source(html),
        None => match email.html_body_value() {
            Some(html) => html_source(html),
            None => format!("<p class=\"note\">{}</p>", escape(&crate::label("no-body"))),
        },
    };
    format!(
        "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n\
         <title>{title}</title>\n<style>\n{style}</style>\n\
         <script>window.addEventListener(\"load\", function () {{ window.print(); }});</script>\n\
         </head>\n<body>\n<header class=\"headers\">\n<h1>{title}</h1>\n<table>\n{rows}</table>\n</header>\n\
         <main class=\"body\">\n{body}\n</main>\n</body>\n</html>\n",
        csp = CSP,
        title = escape(subject),
        style = style(points),
    )
}

fn html_source(html: &str) -> String {
    format!(
        "<p class=\"note\">{}</p>\n<pre>{}</pre>",
        escape(&crate::label("html-source")),
        escape(html)
    )
}

/// `path` as a `file://` URI, every byte outside the unreserved set and
/// `/` percent-encoded.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// `$XDG_RUNTIME_DIR/<name>`, created 0700 (and made 0700 when it is
/// already there): a private directory the session's files go in.
pub fn runtime_dir(name: &str) -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| crate::label("no-runtime-dir"))?;
    private_dir(&base.join(name))
}

/// `dir`, created 0700 if missing; an existing one must be a real
/// directory (not a link) and is set to 0700.
pub fn private_dir(dir: &Path) -> Result<PathBuf, String> {
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.is_dir() {
        return Err(format!("{}: not a directory", dir.display()));
    }
    if meta.permissions().mode() & 0o777 != 0o700 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(dir.to_path_buf())
}

/// Remove the regular files in `dir` last changed more than `keep` ago.
/// Best effort: a file that cannot be read or removed stays.
pub fn prune(dir: &Path, keep: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > keep);
        if meta.is_file() && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Write `html` as a new 0600 page in `dir`, after pruning the old ones;
/// the file's path.
pub fn write_in(dir: &Path, html: &str) -> Result<PathBuf, String> {
    prune(dir, KEEP);
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = dir.join(format!("email-{}-{stamp}.html", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(html.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Write `html` into the session's print directory. It blocks: the shell
/// calls it off the UI thread.
pub fn write_page(html: &str) -> Result<PathBuf, String> {
    write_in(&runtime_dir(DIR)?, html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn email(text: Option<(&str, &str)>, html: Option<&str>) -> Email {
        let mut value = json!({
            "id": "E1",
            "subject": "Plan <b>&</b> \"notes\"",
            "from": [{ "name": "Ann <boss>", "email": "ann@example.invalid" }],
            "to": [{ "name": null, "email": "bob@example.invalid" }],
            "cc": [{ "name": "Cy", "email": "cy@example.invalid" }],
            "receivedAt": "2026-10-13T01:02:03Z",
            "bodyValues": {},
            "textBody": [],
            "htmlBody": [],
        });
        if let Some((kind, body)) = text {
            value["textBody"] = json!([{ "partId": "1", "type": kind }]);
            value["bodyValues"]["1"] = json!({ "value": body });
        }
        if let Some(body) = html {
            value["htmlBody"] = json!([{ "partId": "2", "type": "text/html" }]);
            value["bodyValues"]["2"] = json!({ "value": body });
        }
        serde_json::from_value(value).unwrap()
    }

    fn render(
        text: Option<(&str, &str)>,
        html: Option<&str>,
        format: Format,
        points: f64,
    ) -> String {
        page(&email(text, html), format, points)
    }

    /// The `<body>` of a page: what the message could have put there.
    fn inside(page: &str) -> &str {
        page.split("<body>").nth(1).unwrap()
    }

    #[test]
    fn the_headers_are_present_and_escaped() {
        let page = render(Some(("text/plain", "Hi")), None, Format::Plain, 15.0);
        for key in ["from", "to", "cc", "print-date", "subject"] {
            assert!(
                page.contains(&format!("<th>{}</th>", escape(&crate::label(key)))),
                "{key}"
            );
        }
        assert!(page.contains("Ann &lt;boss&gt; &lt;ann@example.invalid&gt;"));
        assert!(page.contains("bob@example.invalid"));
        assert!(page.contains("Cy &lt;cy@example.invalid&gt;"));
        assert!(page.contains("2026"), "the date");
        assert!(page.contains("<title>Plan &lt;b&gt;&amp;&lt;/b&gt; &quot;notes&quot;</title>"));
        assert!(!inside(&page).contains("<b>"));
    }

    #[test]
    fn the_csp_print_call_and_text_size_are_in_the_head() {
        let page = render(Some(("text/plain", "Hi")), None, Format::Plain, 18.5);
        assert!(page.contains(&format!(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"{CSP}\">"
        )));
        assert!(page.contains("window.print()"));
        assert!(page.contains("font-size: 18.5pt"));
        assert!(page.contains("@page"));
        // The one script is the page's own, in the head.
        assert_eq!(page.matches("<script").count(), 1);
        assert!(!inside(&page).contains("<script"));
    }

    #[test]
    fn plain_text_is_escaped_and_wrapped() {
        let text = "a < b && c > d\n<script>alert(1)</script>";
        let page = render(Some(("text/plain", text)), None, Format::Plain, 15.0);
        assert!(page.contains(
            "<pre class=\"plain\">a &lt; b &amp;&amp; c &gt; d\n&lt;script&gt;alert(1)&lt;/script&gt;</pre>"
        ));
        assert!(page.contains("white-space: pre-wrap"));
    }

    #[test]
    fn markdown_renders_but_its_raw_html_is_escaped() {
        let md = "# Plan\n\n- **one**\n- two\n\n<div onclick=\"x()\">raw</div>\n\nand <span style=\"x\">inline</span> too\n";
        let page = render(Some(("text/markdown", md)), None, Format::Markdown, 15.0);
        let shown = inside(&page);
        assert!(shown.contains("<h1>Plan</h1>"));
        assert!(shown.contains("<strong>one</strong>"));
        assert!(shown.contains("&lt;div onclick=&quot;x()&quot;&gt;raw&lt;/div&gt;"));
        assert!(shown.contains("&lt;span style=&quot;x&quot;&gt;inline&lt;/span&gt;"));
        assert!(!shown.contains("<div") && !shown.contains("<span"));
    }

    #[test]
    fn no_remote_source_reaches_the_page() {
        let md = "![a chart](https://tracker.example.invalid/pixel.png \"t\")\n\n\
                  <img src=\"https://tracker.example.invalid/x.png\">\n\n\
                  [site](https://example.invalid/) [bad](javascript:alert(1)) [file](file:///etc/passwd)\n";
        let page = render(Some(("text/markdown", md)), None, Format::Markdown, 15.0);
        let shown = inside(&page);
        assert!(!shown.contains("<img"), "{shown}");
        assert!(!shown.contains("src="), "{shown}");
        assert!(shown.contains("a chart"), "the alt text stands in");
        assert!(shown.contains("<a href=\"https://example.invalid/\">site</a>"));
        assert!(!shown.contains("javascript:alert(1)\""), "{shown}");
        assert!(!shown.contains("href=\"javascript"), "{shown}");
        assert!(!shown.contains("href=\"file"), "{shown}");
        assert!(shown.contains("bad") && shown.contains("file"));
    }

    #[test]
    fn an_html_body_is_its_escaped_source() {
        let html = "<p>Hi <img src=\"https://tracker.example.invalid/p.gif\"></p>";
        let page = render(None, Some(html), Format::Plain, 15.0);
        let shown = inside(&page);
        assert!(shown.contains("&lt;p&gt;Hi &lt;img src=&quot;https://tracker"));
        assert!(!shown.contains("<img") && !shown.contains("<p>Hi"));
        // The same when the server serves the HTML as the text body.
        let page = render(Some(("text/html", html)), None, Format::Markdown, 15.0);
        assert!(!inside(&page).contains("<img"));
    }

    #[test]
    fn a_page_is_written_private_and_old_ones_are_pruned() {
        let root = tempfile::tempdir().unwrap();
        let dir = private_dir(&root.path().join(DIR)).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let old = dir.join("old.html");
        std::fs::write(&old, "x").unwrap();
        let file = std::fs::File::options().write(true).open(&old).unwrap();
        file.set_modified(SystemTime::now() - KEEP - Duration::from_secs(60))
            .unwrap();
        let fresh = dir.join("fresh.html");
        std::fs::write(&fresh, "y").unwrap();
        let path = write_in(&dir, "<p>page</p>").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "<p>page</p>");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!old.exists(), "pruned");
        assert!(fresh.exists(), "kept");
    }

    #[test]
    fn a_file_uri_is_percent_encoded() {
        assert_eq!(
            file_uri(Path::new("/run/user/1002/email-print/a b#?.html")),
            "file:///run/user/1002/email-print/a%20b%23%3F.html"
        );
    }
}
