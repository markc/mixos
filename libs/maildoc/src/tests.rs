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
        "to",
        "command",
        "id",
        "type",
        "from",
        "rc",
        "reply-to",
        "args",
        "json",
        "recipient_connection",
        "broker_uid",
        "broker_pid",
        "broker_node",
        "broker_mesh",
        "broker_user",
        "mesh_from",
        "topic",
        "topic_seq",
        "topic_node",
        "title",
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
        let document = MailDocument {
            subject: subject.into(),
            ..Default::default()
        };
        let text = write(&document).unwrap();
        assert_eq!(parse(&text).unwrap(), document);
        assert!(wire::parse_strict(&text).is_ok());
        if !subject.starts_with('"') {
            assert_eq!(
                parse(&framed(&format!("mail-subject: {subject}")))
                    .unwrap()
                    .subject,
                subject
            );
        }
    }
}

#[test]
fn significant_subject_whitespace_round_trips() {
    for subject in ["  leading", "trailing  ", "   ", "\u{2003}subject\u{2003}"] {
        let document = MailDocument {
            subject: subject.into(),
            ..Default::default()
        };
        assert_eq!(parse(&write(&document).unwrap()).unwrap(), document);
    }
}

#[test]
fn body_is_written_verbatim_and_noncanonical_whitespace_is_refused() {
    let text = framed("mail-subject: body") + "  first  \nsecond \n\n \t\u{2003}";
    let parsed = parse(&text).unwrap();
    assert_eq!(parsed.body, "  first  \nsecond");
    assert!(write(&parsed).unwrap().ends_with(&parsed.body));
    assert_eq!(parse(&write(&parsed).unwrap()).unwrap(), parsed);
    let plain = MailDocument {
        body: "no newline".into(),
        ..Default::default()
    };
    assert!(write(&plain).unwrap().ends_with("no newline"));
    assert_eq!(parse(&write(&plain).unwrap()).unwrap(), plain);
}

#[test]
fn carriage_return_anywhere_has_its_own_refusal() {
    for text in [
        "---\r\nmail-subject: x\r\n---\r\n",
        "---\nmail-subject: x\r\n---\n",
        "---\n---\nbody\r\n",
        "---\nmail-subject: x\ry\n---\n",
        "---\n---\nbody\rtext",
        "---\n---\nbody\r",
        "---\r---",
    ] {
        assert_eq!(parse(text), Err(Refusal::CarriageReturn));
    }
    let message = Refusal::CarriageReturn.to_string();
    assert!(message.contains("CRLF"));
    assert!(message.contains("use LF"));
}

#[test]
fn empty_headers_are_omitted_and_empty_values_refused() {
    let document = MailDocument::default();
    let text = write(&document).unwrap();
    assert!(!text.contains("mail-"));
    assert_eq!(parse(&text).unwrap(), MailDocument::default());
    for name in HEADERS {
        for value in ["", " ", "\"\""] {
            if *name == "mail-attachments" && value == "\"\"" {
                assert!(matches!(
                    parse(&framed(&format!("{name}: {value}"))),
                    Err(Refusal::InvalidValue { header, reason })
                        if header == *name && reason.starts_with("expected a JSON array of paths:")
                ));
                continue;
            }
            assert_eq!(
                parse(&framed(&format!("{name}: {value}"))),
                Err(Refusal::EmptyValue((*name).into()))
            );
        }
        assert_eq!(
            parse(&framed(&format!("{name}:"))),
            Err(Refusal::MalformedHeader(format!("{name}:")))
        );
    }
}

#[test]
fn attachments_are_absolute_paths_only() {
    for value in [
        r#"["relative.pdf"]"#,
        r#"["/absolute", "../relative"]"#,
        r#"[1]"#,
        r#"{"path":"/x"}"#,
        r#""[\"/x\"]""#,
        "[broken",
        "null",
    ] {
        assert!(matches!(
            parse(&framed(&format!("mail-attachments: {value}"))),
            Err(Refusal::InvalidValue { .. })
        ));
    }
    let document = parse(&framed(
        r#"mail-attachments: ["/nonexistent/report.pdf", "/space and \"quote\".txt"]"#,
    ))
    .unwrap();
    assert_eq!(
        document.attachments[0],
        PathBuf::from("/nonexistent/report.pdf")
    );
    assert_eq!(parse(&write(&document).unwrap()).unwrap(), document);
    assert!(
        parse(&framed("mail-attachments: []"))
            .unwrap()
            .attachments
            .is_empty()
    );
}

#[test]
fn refuses_cr_lf_in_decoded_values_and_raw_headers() {
    for name in HEADERS {
        for escaped in [r#""x\ny@example.org""#, r#""x\ry@example.org""#] {
            let value = if *name == "mail-attachments" {
                format!("[{escaped}]")
            } else {
                escaped.into()
            };
            assert_eq!(
                parse(&framed(&format!("{name}: {value}"))),
                Err(invalid(name, "CR or LF in a value"))
            );
        }
        assert_eq!(
            parse(&framed(&format!("{name}: x\ry@example.org"))),
            Err(Refusal::CarriageReturn)
        );
        assert_eq!(
            parse(&framed(&format!("{name}: x\ny@example.org"))),
            Err(Refusal::MalformedHeader("y@example.org".into()))
        );
    }
}

#[test]
fn relay_cap_counts_utf8_bytes_including_framing() {
    let mut text = String::from("---\nmail-subject: x\n---\n");
    text.push_str(&"x".repeat(MAX_DOCUMENT_BYTES - text.len()));
    assert!(parse(&text).is_ok());
    text.push('x');
    assert_eq!(
        parse(&text),
        Err(Refusal::TooLarge {
            bytes: MAX_DOCUMENT_BYTES + 1
        })
    );
    let text = "é".repeat(MAX_DOCUMENT_BYTES / 2 + 1);
    assert_eq!(parse(&text), Err(Refusal::TooLarge { bytes: text.len() }));
}

#[test]
fn address_lists_trim_entries_and_only_lightly_validate() {
    let document = parse(&framed("mail-to: Alice <alice@example.org>,  bob@example.org\nmail-cc: cc@example.org\nmail-bcc: bcc@example.org\nmail-from: Name <from@example.org>\nmail-reply-to: reply@example.org")).unwrap();
    assert_eq!(
        document.to,
        ["Alice <alice@example.org>", "bob@example.org"]
    );
    assert_eq!(parse(&write(&document).unwrap()).unwrap(), document);
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
    let document = parse(&framed("mail-in-reply-to: <parent@example.org>\nmail-references:  <first@example.org>   <second@example.org>  ")).unwrap();
    assert_eq!(
        document.references,
        ["<first@example.org>", "<second@example.org>"]
    );
    assert_eq!(parse(&write(&document).unwrap()).unwrap(), document);
}

#[test]
fn refuses_malformed_and_duplicate_headers_before_wire_can_discard_them() {
    for (headers, name) in [
        ("mail-subject: first\nmail-subject: second", "mail-subject"),
        ("mail-to: bad\nmail-to: good@example.org", "mail-to"),
    ] {
        assert_eq!(parse(&framed(headers)), Err(Refusal::DuplicateHeader(name.into())));
    }
    for line in [
        " mail-subject: x",
        "mail-subject : x",
        "mail-subject:x",
        " continuation",
        "bare-header",
        "   ",
        "\t",
    ] {
        assert_eq!(parse(&framed(&format!("mail-to: a@b\n{line}"))), Err(Refusal::MalformedHeader(line.into())));
    }
    for text in ["no opening", "---\nmail-subject: x\n"] {
        assert!(matches!(parse(text), Err(Refusal::Framing(_))));
    }
    assert!(parse("---\nmail-subject: x\n---").is_ok());
}

#[test]
fn empty_header_blocks_work_with_or_without_body() {
    for text in [
        "---\n---\n",
        "---\n---",
        "---\n\n---\n",
        "---\n---\nbody",
        "---\n\n---\nbody",
    ] {
        let document = parse(text).unwrap();
        assert_eq!(
            document.body,
            if text.ends_with("body") { "body" } else { "" }
        );
        assert_eq!(parse(&write(&document).unwrap()).unwrap(), document);
        assert!(wire::parse_strict(&write(&document).unwrap()).is_ok());
    }
}

#[test]
fn headers_are_sorted_and_known_to_wire_validation() {
    let document = MailDocument {
        to: vec!["to@example.org".into()],
        cc: vec!["cc@example.org".into()],
        bcc: vec!["bcc@example.org".into()],
        from: Some("from@example.org".into()),
        reply_to: Some("reply@example.org".into()),
        subject: "subject".into(),
        in_reply_to: Some("<parent>".into()),
        references: vec!["<parent>".into()],
        attachments: vec!["/nonexistent".into()],
        body: "body".into(),
    };
    let text = write(&document).unwrap();
    let message = wire::parse_strict(&text).unwrap();
    assert!(wire::validate(&message).is_empty());
    assert_eq!(message.headers.len(), 9);
    let names: Vec<_> = text
        .lines()
        .skip(1)
        .take(9)
        .map(|line| line.split_once(':').unwrap().0)
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert_eq!(parse(&text).unwrap(), document);
}

fn with_header(name: &str, value: &str) -> MailDocument {
    let mut document = MailDocument::default();
    match name {
        "mail-to" => document.to.push(value.into()),
        "mail-cc" => document.cc.push(value.into()),
        "mail-bcc" => document.bcc.push(value.into()),
        "mail-from" => document.from = Some(value.into()),
        "mail-reply-to" => document.reply_to = Some(value.into()),
        "mail-subject" => document.subject = value.into(),
        "mail-in-reply-to" => document.in_reply_to = Some(value.into()),
        "mail-references" => document.references.push(value.into()),
        "mail-attachments" => document.attachments.push(value.into()),
        _ => panic!("unknown test header: {name}"),
    }
    document
}

fn assert_refused(document: &MailDocument, refusal: Refusal) {
    assert_eq!(validate(document), Err(refusal.clone()));
    assert_eq!(write(document), Err(refusal));
}

#[test]
fn writer_and_validator_refuse_injection_in_every_header_field() {
    for name in HEADERS {
        for injection in ["\n", "\r", "\n---\n", "\nmail-bcc: eve@example.org"] {
            let document = with_header(name, &format!("/x{injection}y@example.org"));
            assert_refused(&document, invalid(name, "CR or LF in a value"));
        }
    }
}

#[test]
fn header_controls_are_refused_raw_decoded_and_by_writer() {
    for name in HEADERS {
        for control in ['\0', '\t', '\u{1f}', '\u{7f}', '\u{85}'] {
            let value = format!("/x{control}y@example.org");
            let expected = invalid(name, "control character in a value");
            assert_refused(&with_header(name, &value), expected.clone());
            assert_eq!(parse(&framed(&format!("{name}: {value}"))), Err(expected.clone()));
            let encoded = if *name == "mail-attachments" {
                serde_json::to_string(&[value]).unwrap()
            } else {
                serde_json::to_string(&value).unwrap()
            };
            assert_eq!(parse(&framed(&format!("{name}: {encoded}"))), Err(expected));
        }
    }
}

#[test]
fn writer_and_validator_refuse_noncanonical_fields() {
    for name in ["mail-to", "mail-cc", "mail-bcc", "mail-from", "mail-reply-to"] {
        for value in ["", "no-address"] {
            let expected = if value.is_empty() && matches!(name, "mail-from" | "mail-reply-to") {
                Refusal::EmptyValue(name.into())
            } else {
                invalid(name, "address must be non-empty and contain @")
            };
            assert_refused(&with_header(name, value), expected);
        }
        for value in [" a@b", "a@b ", "a@b,c@d"] {
            assert_refused(&with_header(name, value), invalid(name, "address must be trimmed and contain no comma"));
        }
    }
    assert_refused(&with_header("mail-in-reply-to", ""), Refusal::EmptyValue("mail-in-reply-to".into()));
    for value in ["", "two tokens", "trailing ", "\u{2003}"] {
        assert_refused(&with_header("mail-references", value), invalid("mail-references", "reference must be a non-empty token without whitespace"));
    }
    assert_refused(&with_header("mail-attachments", "relative"), invalid("mail-attachments", "attachment path must be absolute"));
    for body in ["body\rtext", "body\r\n", "body\r"] {
        assert_refused(&MailDocument { body: body.into(), ..Default::default() }, Refusal::CarriageReturn);
    }
    for body in ["body ", "body\n", "body\u{2003}"] {
        assert_refused(&MailDocument { body: body.into(), ..Default::default() }, invalid("body", "trailing whitespace is not canonical"));
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_attachment_is_a_typed_refusal() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let document = MailDocument {
        attachments: vec![PathBuf::from(OsStr::from_bytes(b"/file-\xff"))],
        ..Default::default()
    };
    assert_refused(&document, invalid("mail-attachments", "path is not UTF-8"));
}

#[test]
fn writer_and_validator_bound_the_complete_encoded_utf8_size() {
    let mut document = MailDocument::default();
    let framing_bytes = write(&document).unwrap().len();
    document.body = "x".repeat(MAX_DOCUMENT_BYTES - framing_bytes);
    assert_eq!(validate(&document), Ok(()));
    assert_eq!(write(&document).unwrap().len(), MAX_DOCUMENT_BYTES);
    document.body.push('é');
    assert_refused(&document, Refusal::TooLarge { bytes: MAX_DOCUMENT_BYTES + 2 });

    // Quoting/escaping expands a document whose field bytes alone fit the cap.
    let document = MailDocument { subject: format!("\"{}", "\\".repeat(MAX_DOCUMENT_BYTES / 2)), ..Default::default() };
    let refusal = validate(&document).unwrap_err();
    assert!(matches!(refusal, Refusal::TooLarge { bytes } if bytes > MAX_DOCUMENT_BYTES));
    assert_eq!(write(&document), Err(refusal));
}

fn arbitrary_document() -> impl Strategy<Value = MailDocument> {
    let text = || prop_oneof![any::<String>(), Just("a@example.org".into()), Just("".into())];
    (
        prop::collection::vec(text(), 0..4),
        prop::collection::vec(text(), 0..4),
        prop::collection::vec(text(), 0..4),
        prop::option::of(text()),
        prop::option::of(text()),
        text(),
        prop::option::of(text()),
        prop::collection::vec(text(), 0..4),
        prop::collection::vec(prop_oneof![any::<String>(), Just("/file".into())], 0..4),
        text(),
    ).prop_map(|(to, cc, bcc, from, reply_to, subject, in_reply_to, references, paths, body)| MailDocument {
        to, cc, bcc, from, reply_to, subject, in_reply_to, references,
        attachments: paths.into_iter().map(PathBuf::from).collect(), body,
    })
}

proptest! {
    #[test]
    fn every_document_is_refused_or_round_trips(document in arbitrary_document()) {
        match write(&document) {
            Ok(text) => {
                prop_assert_eq!(validate(&document), Ok(()));
                prop_assert_eq!(parse(&text), Ok(document));
            }
            Err(refusal) => prop_assert_eq!(validate(&document), Err(refusal)),
        }
    }

    #[test]
    fn canonical_documents_round_trip(
        subject in r"[^\p{Cc}]{0,96}",
        body in "[^\r]{0,256}",
        recipients in prop::collection::vec("[a-z]{1,12}@[a-z]{1,12}\\.example", 0..5),
        optional in prop::option::of(r"[^\p{Cc}]{1,48}"),
        references in prop::collection::vec("<[a-z]{1,12}@example\\.org>", 0..5),
        paths in prop::collection::vec(r"/[^\p{Cc}]{0,48}", 0..5),
    ) {
        let document = MailDocument {
            to: recipients.clone(), cc: recipients.clone(), bcc: recipients,
            from: Some("Name <from@example.org>".into()), reply_to: Some("reply@example.org".into()),
            subject, in_reply_to: optional, references,
            attachments: paths.into_iter().map(PathBuf::from).collect(),
            body: body.trim_end().to_owned(),
        };
        let text = write(&document).unwrap();
        prop_assert!(wire::parse_strict(&text).is_ok());
        prop_assert_eq!(parse(&text).unwrap(), document);
    }
}
