// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Printing: one self-contained HTML page for the browser's print dialog
//! (which also saves to PDF). MixOS has no print service and no PDF
//! writer, so the page is the whole of it.
//!
//! [`page`] is pure. The page holds the headers (From, To, Cc, Date,
//! Subject) and the body as the reader shows it: Markdown rendered to
//! HTML, or the plain text in a wrapping `<pre>`. Nothing from the email
//! is ever passed through as markup: raw HTML in Markdown becomes escaped
//! text, images become their alt text (a remote image would tell the sender
//! the mail was printed), and only http, https and mailto links keep their
//! address. A strict Content-Security-Policy allows the inline style and
//! the one inline script that opens the print dialog, and nothing else.
//! Colours are the design's light tokens, whatever the window's mode.

use std::hash::{Hash, Hasher};

use jmap::{Email, EmailAddress};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// The page's colours, as CSS hex, from the design's light theme.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub text: String,
    pub dim: String,
    pub background: String,
    pub link: String,
    pub code: String,
    pub rule: String,
}

impl Palette {
    /// The light Mono design's colours: black on white, as paper wants.
    pub fn light() -> Self {
        let theme = toolkit::Theme::for_context(design::DesignContext {
            scheme: design::Scheme::Mono,
            mode: design::Mode::Light,
            ..design::DesignContext::default()
        });
        let v = toolkit::style::visuals(&theme);
        let hex = |c: egui::Color32| format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b());
        Self {
            text: hex(v.text_color()),
            dim: hex(v.weak_text_color()),
            background: hex(v.panel_fill),
            link: hex(v.hyperlink_color),
            code: hex(v.code_bg_color),
            rule: hex(v.widgets.noninteractive.bg_stroke.color),
        }
    }
}

/// The page's file name: a hash of the message (its Message-ID, else the
/// JMAP id), so printing one email again replaces its page.
pub fn file_name(email: &Email) -> String {
    let mut hasher = std::hash::DefaultHasher::new();
    match email.message_id.as_ref().and_then(|ids| ids.first()) {
        Some(id) => id.hash(&mut hasher),
        None => email.id.hash(&mut hasher),
    }
    format!("{:016x}.html", hasher.finish())
}

/// `text` safe as HTML text and attribute content.
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

fn addresses(list: &Option<Vec<EmailAddress>>) -> String {
    list.iter()
        .flatten()
        .map(jmap::format_address)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The text the page prints: the text body, else the HTML body's text.
fn body_text(email: &Email) -> String {
    let html_part = email
        .text_body
        .as_ref()
        .and_then(|parts| parts.first())
        .and_then(|part| part.content_type.as_deref())
        .is_some_and(|t| t.trim().to_ascii_lowercase().starts_with("text/html"));
    match email.text_body_value() {
        Some(text) if html_part => crate::quote::html_text(text),
        Some(text) => text.to_owned(),
        None => email
            .html_body_value()
            .map(crate::quote::html_text)
            .unwrap_or_default(),
    }
}

/// Markdown as HTML, with every raw HTML event made text, images reduced
/// to their alt text, and links kept only to openable addresses.
pub fn markdown_html(source: &str) -> String {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    // Whether each open link was kept, so its end goes with it.
    let mut links: Vec<bool> = Vec::new();
    let events = Parser::new_ext(source, options).filter_map(|event| match event {
        // Escaped here, quotes and all, and passed on as already-safe markup.
        Event::Html(text) | Event::InlineHtml(text) => {
            Some(Event::InlineHtml(escape(&text).into()))
        }
        Event::Start(Tag::HtmlBlock) => Some(Event::Start(Tag::Paragraph)),
        Event::End(TagEnd::HtmlBlock) => Some(Event::End(TagEnd::Paragraph)),
        Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => None,
        Event::Start(Tag::Link { ref dest_url, .. }) => {
            let keep = toolkit::markdown::openable(dest_url);
            links.push(keep);
            keep.then_some(event)
        }
        Event::End(TagEnd::Link) => links.pop().unwrap_or(false).then_some(event),
        other => Some(other),
    });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events);
    html
}

/// The print page for `email`: Markdown rendered when `markdown`, else the
/// plain text; the body at `font_pt` points.
pub fn page(email: &Email, markdown: bool, font_pt: f32, palette: &Palette) -> String {
    let subject = email.subject.clone().unwrap_or_default();
    let date = email
        .date
        .as_deref()
        .or(email.received_at.as_deref())
        .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%a %-d %b %Y, %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    let mut rows = String::new();
    for (key, value) in [
        ("from", addresses(&email.from)),
        ("to", addresses(&email.to)),
        ("cc", addresses(&email.cc)),
        ("print-date", date),
        ("subject", subject.clone()),
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
    let text = body_text(email);
    let body = if markdown {
        format!("<main class=\"markdown\">{}</main>", markdown_html(&text))
    } else {
        format!("<main><pre class=\"plain\">{}</pre></main>", escape(&text))
    };
    let Palette {
        text: ink,
        dim,
        background,
        link,
        code,
        rule,
    } = palette;
    format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'">
<meta name="referrer" content="no-referrer">
<title>{title}</title>
<style>
@page {{ margin: 18mm 16mm; }}
html {{ background: {background}; color: {ink}; }}
body {{ margin: 0; font: {font_pt}pt/1.45 Georgia, "Noto Serif", "DejaVu Serif", serif; }}
table.headers {{ border-collapse: collapse; margin-bottom: 0.8em; font-size: 0.9em; }}
table.headers th {{ text-align: left; padding: 0.1em 1em 0.1em 0; color: {dim}; font-weight: normal; vertical-align: top; }}
table.headers td {{ padding: 0.1em 0; }}
hr {{ border: 0; border-top: 1px solid {rule}; margin: 0 0 1em; }}
pre, code {{ font-family: "JetBrains Mono", "DejaVu Sans Mono", monospace; font-size: 0.9em; }}
pre {{ white-space: pre-wrap; overflow-wrap: anywhere; }}
pre.plain {{ font-family: inherit; font-size: 1em; }}
main.markdown pre {{ background: {code}; padding: 0.6em 0.8em; border-radius: 4px; }}
main.markdown code {{ background: {code}; padding: 0 0.2em; border-radius: 3px; }}
main.markdown pre code {{ background: none; padding: 0; }}
main.markdown blockquote {{ margin: 0 0 0 0.2em; padding-left: 0.9em; border-left: 3px solid {rule}; color: {dim}; }}
main.markdown table {{ border-collapse: collapse; }}
main.markdown th, main.markdown td {{ border: 1px solid {rule}; padding: 0.2em 0.6em; }}
a {{ color: {link}; }}
</style></head>
<body>
<table class="headers">
{rows}</table>
<hr>
{body}
<script>window.addEventListener('load',()=>window.print())</script>
</body></html>
"#,
        title = escape(&subject),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn palette() -> Palette {
        Palette {
            text: "#000000".into(),
            dim: "#555555".into(),
            background: "#ffffff".into(),
            link: "#0000ee".into(),
            code: "#eeeeee".into(),
            rule: "#cccccc".into(),
        }
    }

    fn email(body: &str) -> Email {
        serde_json::from_value(json!({
            "id": "E1",
            "subject": "Plans <b>&</b> more",
            "from": [{ "name": "Bob \"B\"", "email": "bob@example.invalid" }],
            "to": [{ "name": "Ann", "email": "ann@example.invalid" }],
            "cc": [{ "email": "cy@example.invalid" }],
            "receivedAt": "2026-10-13T01:02:03Z",
            "textBody": [{ "partId": "1", "type": "text/plain" }],
            "bodyValues": { "1": { "value": body } },
        }))
        .unwrap()
    }

    #[test]
    fn the_headers_are_there_and_escaped() {
        let page = page(&email("hi"), false, 15.0, &palette());
        for key in ["from", "to", "cc", "print-date", "subject"] {
            assert!(page.contains(&format!("<th>{}</th>", crate::label(key))), "{key}");
        }
        assert!(page.contains("Plans &lt;b&gt;&amp;&lt;/b&gt; more"));
        assert!(!page.contains("<b>&</b>"));
        assert!(page.contains("bob@example.invalid"));
        assert!(page.contains("cy@example.invalid"));
    }

    #[test]
    fn the_csp_and_the_print_script_are_there() {
        let page = page(&email("hi"), false, 15.0, &palette());
        assert!(page.contains(
            "content=\"default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'\""
        ));
        assert!(page.contains("window.addEventListener('load',()=>window.print())"));
        assert!(page.contains("font: 15pt/1.45"));
    }

    #[test]
    fn plain_text_is_escaped_in_a_wrapping_pre() {
        let page = page(&email("a <script>x</script> & b"), false, 12.0, &palette());
        assert!(page.contains("<pre class=\"plain\">a &lt;script&gt;x&lt;/script&gt; &amp; b</pre>"));
        // The page's own script is the only one.
        assert_eq!(page.matches("<script>").count(), 1);
    }

    #[test]
    fn markdown_html_is_escaped_and_images_are_alt_text() {
        let html = markdown_html(
            "# Hi\n\n<div onclick=\"x\">raw</div>\n\nan <b>inline</b> tag ![a cat](https://track.invalid/p.gif)\n\n[ok](https://mixos.dev/) [bad](javascript:alert(1)) [file](file:///etc/passwd)",
        );
        assert!(html.contains("<h1>Hi</h1>"));
        assert!(html.contains("&lt;div onclick=&quot;x&quot;&gt;raw&lt;/div&gt;"), "{html}");
        assert!(html.contains("an &lt;b&gt;inline&lt;/b&gt; tag a cat"), "{html}");
        assert!(!html.contains("<img"), "{html}");
        assert!(!html.contains("track.invalid"), "no remote URL survives: {html}");
        assert!(html.contains("<a href=\"https://mixos.dev/\">ok</a>"), "{html}");
        assert!(!html.contains("javascript:"), "{html}");
        assert!(!html.contains("file://"), "{html}");
        assert!(html.contains("bad") && html.contains("file"));
    }

    #[test]
    fn no_src_attribute_points_anywhere() {
        let page = page(
            &email("![x](https://a.invalid/x.png)\n\n<img src=\"https://b.invalid/y.png\">"),
            true,
            15.0,
            &palette(),
        );
        assert!(!page.contains("<img"), "{page}");
        assert!(!page.contains("src=\""), "{page}");
    }

    #[test]
    fn the_file_name_follows_the_message() {
        let a = email("x");
        assert!(file_name(&a).ends_with(".html"));
        assert_eq!(file_name(&a), file_name(&a));
        let mut b = a.clone();
        b.message_id = Some(vec!["<m@example.invalid>".into()]);
        assert_ne!(file_name(&a), file_name(&b));
        assert!(!file_name(&b).contains('/'));
    }
}
