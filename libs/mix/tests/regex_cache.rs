// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Compiled-regex cache (0.117.0) — behaviour pins.
//!
//! The re_* family and grep_lines now share a bounded LRU of compiled
//! patterns. These tests pin that the cache is invisible: the first
//! (cold) and second (warm) call with a pattern give the same answer,
//! an invalid pattern raises the same error every time, and a script
//! that churns through far more unique patterns than the cache holds
//! still gets correct results. The size bounds themselves are unit
//! tests in `src/regex_cache.rs`.

use mix::run_capturing;

async fn out(source: &str) -> String {
    let (_, stdout, stderr) = run_capturing(source).await.expect("source should run");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
    stdout
}

async fn err(source: &str) -> String {
    match run_capturing(source).await {
        Ok((v, o, e)) => panic!("expected an error, got {v:?} (stdout={o:?} stderr={e:?})"),
        Err(e) => e.to_string(),
    }
}

#[tokio::test]
async fn cold_and_warm_calls_agree_for_every_builtin() {
    // Each line runs twice: the first compiles (cold), the second hits
    // the cache (warm). Patterns are unique to this test.
    let src = r#"
$t = "alpha 12 beta 345\ngamma 6\n"
for $i in [1, 2]
    print(re_match($t, 'cache-pin-a|\d{3}'))
    print(json_encode(re_find($t, 'cache-pin-b|(\w+) (\d+)')))
    print(re_replace($t, 'cache-pin-c|(\d+)', '<$1>'))
    print(json_encode(re_split($t, 'cache-pin-d|\s+')))
    print(json_encode(grep_lines($t, 'cache-pin-e|^g')))
    print(re_replace_must($t, 'cache-pin-f|beta', 'BETA'))
end
"#;
    let got = out(src).await;
    let lines: Vec<&str> = got.lines().collect();
    let half = lines.len() / 2;
    assert_eq!(lines.len(), half * 2, "{got}");
    assert_eq!(lines[..half], lines[half..], "cold and warm differ:\n{got}");
    assert_eq!(lines[0], "true");
    assert_eq!(
        lines[1],
        r#"[{"end":8,"groups":["alpha","12"],"match":"alpha 12","start":0},{"end":17,"groups":["beta","345"],"match":"beta 345","start":9},{"end":25,"groups":["gamma","6"],"match":"gamma 6","start":18}]"#
    );
}

#[tokio::test]
async fn inline_flags_are_part_of_the_key() {
    // (?i) changes compilation; it must not share an entry with the
    // case-sensitive form.
    let src = r#"
print(re_match("ABC", 'cache-pin-flag|abc'))
print(re_match("ABC", '(?i)cache-pin-flag|abc'))
print(re_match("ABC", 'cache-pin-flag|abc'))
"#;
    assert_eq!(out(src).await, "false\ntrue\nfalse\n");
}

#[tokio::test]
async fn invalid_pattern_error_is_identical_every_time() {
    let short = r#"re_match("x", 'cache-pin-bad(')"#;
    let first = err(short).await;
    assert!(first.contains("invalid regex 'cache-pin-bad('"), "{first}");
    assert_eq!(err(short).await, first);

    // The long-pattern (swapped-argument) report is rebuilt the same way.
    let long = format!(r#"re_find("x", '{}(')"#, "cache-pin-long ".repeat(10));
    let first = err(&long).await;
    assert!(first.contains("truncated"), "{first}");
    assert_eq!(err(&long).await, first);

    // Still catchable, and a valid call right after is unaffected.
    let src = r#"
for $i in [1, 2]
    try
        re_split("a,b", 'cache-pin-split[')
    catch $e
        print("caught")
    end
    print(json_encode(re_split("a,b", 'cache-pin-split|,')))
end
"#;
    assert_eq!(out(src).await, "caught\n[\"a\",\"b\"]\ncaught\n[\"a\",\"b\"]\n");
}

#[tokio::test]
async fn churn_past_the_bound_stays_correct() {
    // 500 unique patterns overflows the 64-entry cache many times over;
    // every match must still be right, and the early pattern recompiles
    // correctly after its eviction.
    let src = r#"
$bad = 0
for $i in range(0, 500)
    if !re_match("row-" .. $i .. "-end", '^row-' .. $i .. '-end$') then
        $bad = $bad + 1
    end
    if re_match("row-" .. $i .. "-end", '^row-' .. ($i + 1) .. '-end$') then
        $bad = $bad + 1
    end
end
print($bad)
print(re_match("row-0-end", '^row-0-end$'))
"#;
    assert_eq!(out(src).await, "0\ntrue\n");
}
