// SPDX-License-Identifier: MIT OR Apache-2.0
use maild::sieve::{Context, Plan, compile};

const MESSAGE: &[u8] = b"From: Alice <alice@example.org>\r\nTo: Bob <bob@example.com>\r\nSubject: Mixed CASE\r\nX-Test: first\r\nX-Test: last\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nHello=20world\r\n";
fn evaluate(script: &str) -> Plan {
    compile(script)
        .unwrap()
        .evaluate(Context {
            data: MESSAGE,
            from: "bounce@example.org",
            to: "bob@example.com",
        })
        .unwrap()
}
fn matches(test: &str) -> bool {
    evaluate(&format!(r#"require ["envelope","body","variables","imap4flags","relational","comparator-i;ascii-numeric","index"]; if {test} {{ discard; }}"#)).filings.is_empty()
}

#[test]
fn base_actions_and_implicit_keep() {
    assert_eq!(evaluate("").filings.len(), 1);
    assert_eq!(evaluate("keep; stop; discard;").filings.len(), 1);
    assert!(evaluate("discard;").filings.is_empty());
    assert!(
        evaluate("redirect \"target@example.com\";")
            .filings
            .is_empty()
    );
}
#[test]
fn if_elsif_else_and_boolean_tests() {
    assert!(evaluate("if false { keep; } elsif allof (true, not false, anyof(false,true)) { discard; } else { keep; }").filings.is_empty());
    assert_eq!(
        evaluate("if false { discard; } else { keep; }")
            .filings
            .len(),
        1
    );
}
#[test]
fn header_comparators_and_match_types() {
    assert!(matches(r#"header :is "subject" "mixed case""#));
    assert!(matches(r#"header :contains "Subject" "CASE""#));
    assert!(matches(r#"header :matches "subject" "M?xed *""#));
    assert!(!matches(
        r#"header :comparator "i;octet" :is "subject" "mixed case""#
    ));
    assert!(matches(
        r#"header :comparator "i;octet" :is "subject" "Mixed CASE""#
    ));
}
#[test]
fn address_and_envelope_parts() {
    assert!(matches(r#"address :is "from" "alice@example.org""#));
    assert!(matches(r#"address :domain "To" "example.com""#));
    assert!(matches(r#"address :localpart "From" "alice""#));
    assert!(matches(r#"envelope :localpart :is "from" "bounce""#));
    assert!(matches(r#"envelope :domain :is "to" "example.com""#));
}
#[test]
fn exists_size_and_numeric_quantifiers() {
    assert!(matches(r#"exists ["From","Subject"]"#));
    assert!(!matches(r#"exists ["From","Missing"]"#));
    assert!(matches("size :under 1K"));
    assert!(matches("size :over 1"));
    assert!(!matches(&format!("size :over {}", MESSAGE.len())));
    assert!(!matches(&format!("size :under {}", MESSAGE.len())));
}
#[test]
fn fileinto_copy_create_and_flags_snapshot() {
    let plan = evaluate(
        r#"require ["fileinto","mailbox","copy","imap4flags"]; setflag ["\\Seen","project"]; fileinto :create :copy "Projects"; removeflag "\\Seen"; addflag "\\Flagged"; keep;"#,
    );
    assert_eq!(plan.filings.len(), 2);
    assert!(plan.filings[0].create);
    assert_eq!(plan.filings[0].flags, vec!["\\Seen", "project"]);
    assert_eq!(plan.filings[1].flags, vec!["project", "\\Flagged"]);
    assert_eq!(plan.filings[0].mailbox.as_deref(), Some("Projects"));
}
#[test]
fn redirect_copy_keeps_and_deduplicates() {
    let plan = evaluate(
        r#"require "copy"; redirect :copy "target@example.com"; redirect :copy "target@example.com";"#,
    );
    assert_eq!(plan.filings.len(), 1);
    assert_eq!(plan.redirects.len(), 1);
}
#[test]
fn body_decodes_transfer_encoding_and_raw_preserves_it() {
    assert!(matches(r#"body :text :contains "Hello world""#));
    assert!(matches(r#"body :raw :contains "Hello=20world""#));
    assert!(matches(
        r#"body :content "text/plain" :contains "Hello world""#
    ));
    assert!(!matches(r#"body :content "image" :contains "Hello""#));
}
#[test]
fn body_wildcards_preserve_earlier_match_variables() {
    let plan = evaluate(r#"require ["variables","body","fileinto"]; if allof(header :matches "Subject" "* CASE",body :matches "Hello *") { fileinto "${1}"; }"#);
    assert_eq!(plan.filings[0].mailbox.as_deref(),Some("Mixed"));
}
#[test]
fn body_content_selectors_expand_variables() {
    assert!(evaluate(r#"require ["variables","body"]; set "type" "text/plain"; if body :content "${type}" :contains "Hello world" { discard; }"#).filings.is_empty());
}
#[test]
fn body_content_matches_transfer_decoded_binary_octets() {
    let script = compile(r#"require "body"; if allof(body :content "image/gif" :comparator "i;octet" :contains "GIF89a",body :content "" :comparator "i;octet" :matches "GIF89a?",not body :content "image/gif" :comparator "i;octet" :is "GIF89a�",body :raw :contains "R0lG") { discard; }"#).unwrap();
    let data = b"Content-Type: image/gif\r\nContent-Transfer-Encoding: base64\r\n\r\nR0lGODlh/w==\r\n";
    assert!(script.evaluate(Context{data,from:"a",to:"b"}).unwrap().filings.is_empty());
}
#[test]
fn variables_expansion_modifiers_and_string_test() {
    let plan = evaluate(
        r#"require ["variables","fileinto"]; set :upper "folder" "mixed"; if string :is "${folder}" "MIXED" { fileinto "${folder}"; }"#,
    );
    assert_eq!(plan.filings[0].mailbox.as_deref(), Some("MIXED"));
    let plan = evaluate(
        r#"require ["variables","fileinto"]; if header :matches "Subject" "* CASE" { fileinto "${1}"; }"#,
    );
    assert_eq!(plan.filings[0].mailbox.as_deref(), Some("Mixed"));
}
#[test]
fn relational_value_count_and_ascii_numeric() {
    assert!(matches(
        r#"header :count "eq" :comparator "i;ascii-numeric" "X-Test" "2""#
    ));
    assert!(matches(r#"header :value "gt" "Subject" "AAA""#));
    assert!(matches(
        r#"string :value "gt" :comparator "i;ascii-numeric" "10000000000000000000000000" "9""#
    ));
    assert!(matches(
        r#"string :is :comparator "i;ascii-numeric" "00100abc" "100""#
    ));
}
#[test]
fn variables_count_ignores_empty_strings() {
    assert!(matches(r#"string :count "eq" :comparator "i;ascii-numeric" "${unset}" "0""#));
    assert!(matches(r#"string :count "eq" :comparator "i;ascii-numeric" ["", "x", "${unset}", "y"] "2""#));
}
#[test]
fn mailboxexists_uses_snapshot_and_inbox_case_rules() {
    let script = compile(r#"require "mailbox"; if mailboxexists ["inbox/Projects", "Archive"] { discard; }"#).unwrap();
    let check = |mailboxes: &[String]| script.evaluate_with_mailboxes(Context {data:MESSAGE,from:"a",to:"b"},mailboxes).unwrap().filings.is_empty();
    assert!(check(&["INBOX/Projects".into(),"Archive".into()]));
    assert!(!check(&["INBOX/projects".into(),"Archive".into()]));
    assert!(!check(&["INBOX/Projects".into()]));
    assert!(compile(r#"if mailboxexists "INBOX" { keep; }"#).is_err());
}
#[test]
fn flag_updates_consume_work_budget() {
    let flags = (0..65).map(|n| format!("f{n}")).collect::<Vec<_>>().join(" ");
    let source = format!(r#"require ["imap4flags","variables"]; set "flags" "{flags}"; {}"#, "addflag \"${flags}\";".repeat(10));
    assert!(compile(&source).unwrap().evaluate(Context {data:MESSAGE,from:"a",to:"b"}).is_err());
}
#[test]
fn hasflag_splits_keys_and_counts_distinct_flags_per_variable() {
    let plan = evaluate(r#"require ["imap4flags","variables","relational","comparator-i;ascii-numeric"]; set "a" "Seen seen Flagged"; set "b" "Seen"; if allof(hasflag "a" "missing SEEN",hasflag :count "eq" :comparator "i;ascii-numeric" ["a","b"] "3") { discard; }"#);
    assert!(plan.filings.is_empty());
    let plan = evaluate(r#"require "imap4flags"; setflag ["\\Recent","\\Unknown","bad(flag)","project"]; if hasflag "project" { keep; stop; } discard;"#);
    assert_eq!(plan.filings[0].flags,vec!["project"]);
}
#[test]
fn index_selects_first_and_last_header() {
    assert!(matches(r#"header :index 1 :is "X-Test" "first""#));
    assert!(matches(r#"header :index 1 :last :is "X-Test" "last""#));
    assert!(!matches(r#"header :index 3 :is "X-Test" "last""#));
}
#[test]
fn mime_header_unfolding_decoding_and_address_index() {
    let data = b"From: a@example.org\r\nFrom: b@example.org\r\nX-Label: =?UTF-8?Q?caf=C3=A9?=\r\n\tshop\r\n\r\nbody";
    let plan = compile(r#"require "index"; if allof (header :is "X-Label" "café shop", address :index 1 :last "From" "b@example.org") { discard; }"#).unwrap().evaluate(Context {data,from:"a",to:"b"}).unwrap();
    assert!(plan.filings.is_empty());
}
#[test]
fn named_flag_variables_and_literal_substitution_without_extension() {
    let plan = evaluate(
        r#"require ["variables","imap4flags"]; setflag "myflags" "\\Seen"; addflag "myflags" "project"; if hasflag "myflags" "\\seen" { keep :flags "${myflags}"; stop; } discard;"#,
    );
    assert_eq!(plan.filings[0].flags, vec!["\\Seen project"]);
    let plan = evaluate(r#"require "fileinto"; fileinto "${literal}";"#);
    assert_eq!(plan.filings[0].mailbox.as_deref(), Some("${literal}"));
}
#[test]
fn vacation_options_and_reject_actions() {
    let plan = evaluate(
        r#"require "vacation"; vacation :days 2 :addresses ["alias@example.com"] :subject "Away" :from "bob@example.com" "Back tomorrow";"#,
    );
    let vacation = plan.vacation.unwrap();
    assert_eq!(vacation.days, 2);
    assert_eq!(vacation.subject.as_deref(), Some("Away"));
    assert_eq!(vacation.from.as_deref(), Some("bob@example.com"));
    assert_eq!(vacation.addresses, vec!["alias@example.com"]);
    assert_eq!(plan.filings.len(), 1);
    for action in ["reject", "ereject"] {
        assert_eq!(
            evaluate(&format!("require \"{action}\"; {action} \"Denied\";"))
                .reject
                .as_deref(),
            Some("Denied")
        );
    }
}
#[test]
fn vacation_response_identity_precedes_variable_expansion() {
    let source = r#"require ["vacation","variables"]; if header :matches "Subject" "*" { vacation :subject "Reply: ${1}" "Away"; }"#;
    let script = compile(source).unwrap();
    let a = script
        .evaluate(Context {
            data: b"Subject: first\r\n\r\nbody",
            from: "a",
            to: "b",
        })
        .unwrap()
        .vacation
        .unwrap();
    let b = script
        .evaluate(Context {
            data: b"Subject: second\r\n\r\nbody",
            from: "a",
            to: "b",
        })
        .unwrap()
        .vacation
        .unwrap();
    assert_ne!(a.subject, b.subject);
    assert_eq!(a.handle, b.handle);
}
#[test]
fn variable_invalid_references_ascii_modifiers_and_nongreedy_captures() {
    let plan = evaluate(
        r#"require ["variables","fileinto"]; set "company" "ACME"; set :upper "name" "éclair"; fileinto "${BAD${company}|${}|${doh!}|${name}";"#,
    );
    assert_eq!(
        plan.filings[0].mailbox.as_deref(),
        Some("${BADACME|${}|${doh!}|éCLAIR")
    );
    let script = compile(r#"require ["variables","fileinto"]; if header :matches "Subject" "* *" { fileinto "${1}|${2}"; }"#).unwrap();
    let plan = script
        .evaluate(Context {
            data: b"Subject: one two three\r\n\r\nbody",
            from: "a",
            to: "b",
        })
        .unwrap();
    assert_eq!(plan.filings[0].mailbox.as_deref(), Some("one|two three"));
}
#[test]
fn comments_multiline_and_dot_unstuffing() {
    let plan = evaluate(
        "require \"vacation\"; /* comment */ # line\nvacation text:\r\n..dot\r\nhello\r\n.\r\n;\n",
    );
    assert_eq!(plan.vacation.unwrap().reason, ".dot\r\nhello\r\n");
}
#[test]
fn invalid_scripts_are_rejected_at_upload_even_in_dead_branches() {
    for source in [
        "fileinto \"x\";",
        "if false { unknown; }",
        "require \"unknown\";",
        "if true {",
        "keep; require \"fileinto\";",
        "if size 10 { keep; }",
        "if header :is :contains \"Subject\" \"x\" { keep; }",
        "keep :unknown;",
        "if exists [] { keep; }",
        "require \"comparator-i;ascii-numeric\"; if header :comparator \"i;ascii-numeric\" :contains \"X-Number\" \"1\" { keep; }",
        "require \"comparator-i;ascii-numeric\"; if header :comparator \"i;ascii-numeric\" :matches \"X-Number\" \"*\" { keep; }",
    ] {
        assert!(compile(source).is_err(), "accepted {source}");
    }
}
#[test]
fn script_depth_size_and_execution_limits() {
    assert!(compile(&" ".repeat(maild::sieve::MAX_SCRIPT_BYTES + 1)).is_err());
    assert!(compile(&("if true {".repeat(40) + &"}".repeat(40))).is_err());
    let source = (0..5)
        .map(|n| format!("redirect \"r{n}@example.com\";"))
        .collect::<String>();
    assert!(
        compile(&source)
            .unwrap()
            .evaluate(Context {
                data: MESSAGE,
                from: "a@example.org",
                to: "bob@example.com"
            })
            .is_err()
    );
    let source = format!(
        "if header :matches \"Subject\" \"{}z\" {{ discard; }}",
        "*?".repeat(30)
    );
    assert!(
        compile(&source)
            .unwrap()
            .evaluate(Context {
                data: MESSAGE,
                from: "a",
                to: "b"
            })
            .is_err()
    );
}
#[test]
fn string_work_and_variable_storage_are_bounded() {
    let mut data = b"Subject: limits\r\n\r\n".to_vec();
    data.extend(vec![b'x'; 1024 * 1024]);
    let keys = (0..30)
        .map(|n| format!("\"never{n}\""))
        .collect::<Vec<_>>()
        .join(",");
    let script = compile(&format!(
        "require \"body\"; if body :contains [{keys}] {{ discard; }}"
    ))
    .unwrap();
    assert!(
        script
            .evaluate(Context {
                data: &data,
                from: "a",
                to: "b"
            })
            .is_err()
    );
    let mut source = format!(
        "require \"variables\"; set \"x\" \"{}\"; set \"x\" \"${{x}}${{x}}\";",
        "a".repeat(30000)
    );
    for n in 0..20 {
        source.push_str(&format!("set \"v{n}\" \"${{x}}\";"));
    }
    assert!(
        compile(&source)
            .unwrap()
            .evaluate(Context {
                data: MESSAGE,
                from: "a",
                to: "b"
            })
            .is_err()
    );
}
#[test]
fn body_transform_work_is_bounded_when_decoded_text_is_empty() {
    let data = format!("Content-Type: text/html\r\n\r\n<!--{}-->", "x".repeat(1024*1024));
    let source = format!("require \"body\"; {}", "if body :contains \"never\" { discard; }".repeat(30));
    assert!(compile(&source).unwrap().evaluate(Context {data:data.as_bytes(),from:"a",to:"b"}).is_err());
}
