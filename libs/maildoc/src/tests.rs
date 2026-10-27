// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

use super::*;
use proptest::prelude::*;

fn framed(headers: &str) -> String {
    format!("---\n{headers}\n---\n")
}

#[test]
fn refuses_routing_and_reserved_headers_by_name() {
    for name in [
        "to", "command", "id", "type", "from", "rc", "reply-to", "args", "json",
        "recipient_connection", "broker_uid", "broker_pid", "broker_node", "broker_mesh",
        "broker_user", "mesh_from", "topic", "topic_seq", "topic_node", "title",
    ] {
        for value in [": x", ":"] {
            let error = parse(&framed(&format!("{name}{value}"))).unwrap_err();
            assert_eq!(error, Refusal::HeaderNotAllowed(name.into()));
            assert!(error.to_string().contains(name));
        }
    }
}

#[test]
fn refuses_unknown_mail_header() {
    assert_eq!(
        parse(&framed("mail-foo: x")),
        Err(Refusal::HeaderNotAllowed("mail-foo".into()))
    );
}

#[test]
fn patch_and_other_json_shaped_subjects_round_trip() {
    for subject in ["[PATCH] fix", "{not JSON", "\"quoted\"", "normal: text"] {
        let document = MailDocument { subject: subject.into(), ..Default::default() };
        let text = write(&document);
        assert_eq!(parse(&text).unwrap(), document);
        assert!(wire::parse_strict(&text).is_ok());
        if !subject.starts_with('"') {
            assert_eq!(parse(&framed(&format!("mail-subject: {subject}"))).unwrap().subject, subject);
        }
    }
}

#[test]
fn significant_subject_whitespace_round_trips() {
    for subject in ["  leading", "trailing  ", " \t ", "\u{2003}subject\u{2003}"] {
        let document = MailDocument { subject: subject.into(), ..Default::default() };
        assert_eq!(parse(&write(&document)).unwrap(), document);
    }
}

#[test]
fn body_is_written_verbatim_and_trailing_whitespace_normalises_on_read() {
    let document = MailDocument {
        subject: "body".into(),
        body: "  first  \nsecond \n\n \t\u{2003}".into(),
        ..Default::default()
    };
    let text = write(&document);
    assert!(text.ends_with(&document.body));
    let parsed = parse(&text).unwrap();
    assert_eq!(parsed.body, "  first  \nsecond");
    assert_eq!(parse(&write(&parsed)).unwrap(), parsed);
    let plain = MailDocument { body: "no newline".into(), ..Default::default() };
    assert!(write(&plain).ends_with("no newline"));
    assert_eq!(parse(&write(&plain)).unwrap(), plain);
}

#[test]
fn crlf_has_its_own_refusal() {
    for text in ["---\r\nmail-subject: x\r\n---\r\n", "---\nmail-subject: x\r\n---\n", "---\n---\nbody\r\n"] {
        assert_eq!(parse(text), Err(Refusal::Crlf));
    }
}

#[test]
fn empty_headers_are_omitted_and_empty_values_refused() {
    let document = MailDocument {
        from: Some(String::new()),
        reply_to: Some(String::new()),
        in_reply_to: Some(String::new()),
        ..Default::default()
    };
    let text = write(&document);
    assert!(!text.contains("mail-"));
    assert_eq!(parse(&text).unwrap(), MailDocument::default());
    for name in HEADERS {
        for value in ["", " ", "\"\""] {
            assert!(parse(&framed(&format!("{name}: {value}"))).is_err());
        }
        assert!(parse(&framed(&format!("{name}:"))).is_err());
    }
}

#[test]
fn attachments_are_absolute_paths_only() {
    for value in [r#"["relative.pdf"]"#, r#"["/absolute", "../relative"]"#, r#"[1]"#, r#"{"path":"/x"}"#, r#""[\"/x\"]""#, "[broken", "null"] {
        assert!(matches!(parse(&framed(&format!("mail-attachments: {value}"))), Err(Refusal::InvalidValue { .. })));
    }
    let document = parse(&framed(r#"mail-attachments: ["/nonexistent/report.pdf", "/space and \"quote\".txt"]"#)).unwrap();
    assert_eq!(document.attachments[0], PathBuf::from("/nonexistent/report.pdf"));
    assert_eq!(parse(&write(&document)).unwrap(), document);
    assert!(parse(&framed("mail-attachments: []")).unwrap().attachments.is_empty());
}

#[test]
fn refuses_cr_lf_in_decoded_values_and_raw_headers() {
    for name in HEADERS {
        for escaped in [r#""x\ny@example.org""#, r#""x\ry@example.org""#] {
            let value = if *name == "mail-attachments" { format!("[{escaped}]") } else { escaped.into() };
            assert!(parse(&framed(&format!("{name}: {value}"))).is_err());
        }
        assert!(parse(&framed(&format!("{name}: x\ry@example.org"))).is_err());
        assert!(parse(&framed(&format!("{name}: x\ny@example.org"))).is_err());
    }
}

#[test]
fn relay_cap_counts_utf8_bytes_including_framing() {
    let mut text = String::from("---\nmail-subject: x\n---\n");
    text.push_str(&"x".repeat(MAX_DOCUMENT_BYTES - text.len()));
    assert!(parse(&text).is_ok());
    text.push('x');
    assert_eq!(parse(&text), Err(Refusal::TooLarge { bytes: MAX_DOCUMENT_BYTES + 1 }));
    let text = "é".repeat(MAX_DOCUMENT_BYTES / 2 + 1);
    assert_eq!(parse(&text), Err(Refusal::TooLarge { bytes: text.len() }));
}

#[test]
fn address_lists_trim_entries_and_only_lightly_validate() {
    let document = parse(&framed("mail-to: Alice <alice@example.org>,  bob@example.org\nmail-cc: cc@example.org\nmail-bcc: bcc@example.org\nmail-from: Name <from@example.org>\nmail-reply-to: reply@example.org")).unwrap();
    assert_eq!(document.to, ["Alice <alice@example.org>", "bob@example.org"]);
    assert_eq!(parse(&write(&document)).unwrap(), document);
    for value in ["no-address", ",a@b", "a@b,", "a@b,  ,c@d"] {
        assert!(parse(&framed(&format!("mail-to: {value}"))).is_err());
    }
    for name in ["mail-from", "mail-reply-to"] {
        assert!(parse(&framed(&format!("{name}: no-address"))).is_err());
    }
    assert!(parse(&framed("mail-to: @")).is_ok());
}

#[test]
fn references_are_whitespace_separated_and_keep_order() {
    let document = parse(&framed("mail-in-reply-to: <parent@example.org>\nmail-references:  <first@example.org>\t  <second@example.org>  ")).unwrap();
    assert_eq!(document.references, ["<first@example.org>", "<second@example.org>"]);
    assert_eq!(parse(&write(&document)).unwrap(), document);
}

#[test]
fn refuses_malformed_and_duplicate_headers_before_wire_can_discard_them() {
    for headers in ["mail-subject: first\nmail-subject: second", "mail-to: bad\nmail-to: good@example.org", " mail-subject: x", "mail-subject : x", "mail-subject:x", "mail-subject: x\n continuation"] {
        assert!(parse(&framed(headers)).is_err());
    }
    for text in ["no opening", "---\nmail-subject: x\n"] {
        assert!(matches!(parse(text), Err(Refusal::Framing(_))));
    }
    assert!(parse("---\nmail-subject: x\n---").is_ok());
}

#[test]
fn empty_header_blocks_work_with_or_without_body() {
    for text in ["---\n---\n", "---\n---", "---\n\n---\n", "---\n---\nbody", "---\n\n---\nbody"] {
        let document = parse(text).unwrap();
        assert_eq!(document.body, if text.ends_with("body") { "body" } else { "" });
        assert_eq!(parse(&write(&document)).unwrap(), document);
        assert!(wire::parse_strict(&write(&document)).is_ok());
    }
}

#[test]
fn headers_are_sorted_and_known_to_wire_validation() {
    let document = MailDocument {
        to: vec!["to@example.org".into()], cc: vec!["cc@example.org".into()], bcc: vec!["bcc@example.org".into()],
        from: Some("from@example.org".into()), reply_to: Some("reply@example.org".into()),
        subject: "subject".into(), in_reply_to: Some("<parent>".into()), references: vec!["<parent>".into()],
        attachments: vec!["/nonexistent".into()], body: "body".into(),
    };
    let text = write(&document);
    let message = wire::parse_strict(&text).unwrap();
    assert!(wire::validate(&message).is_empty());
    assert_eq!(message.headers.len(), 9);
    let names: Vec<_> = text.lines().skip(1).take(9).map(|line| line.split_once(':').unwrap().0).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert_eq!(parse(&text).unwrap(), document);
}

proptest! {
    #[test]
    fn canonical_documents_round_trip(
        subject in "[^\r\n]{0,96}",
        body in "[^\r]{0,256}",
        recipients in prop::collection::vec("[a-z]{1,12}@[a-z]{1,12}\\.example", 0..5),
        optional in prop::option::of("[^\r\n]{1,48}"),
        references in prop::collection::vec("<[a-z]{1,12}@example.org>", 0..5),
        paths in prop::collection::vec("/[^\r\n]{0,48}", 0..5),
    ) {
        let document = MailDocument {
            to: recipients.clone(), cc: recipients.clone(), bcc: recipients,
            from: Some("Name <from@example.org>".into()), reply_to: Some("reply@example.org".into()),
            subject, in_reply_to: optional, references,
            attachments: paths.into_iter().map(PathBuf::from).collect(),
            body: body.trim_end().to_owned(),
        };
        let text = write(&document);
        prop_assert!(wire::parse_strict(&text).is_ok());
        prop_assert_eq!(parse(&text).unwrap(), document);
    }
}
