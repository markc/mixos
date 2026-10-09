// SPDX-License-Identifier: MIT OR Apache-2.0
//! `ord()` / `chr()` — codepoint ↔ character (0.90.0).
//!
//! Before these, the only way to ask what a string held was
//! `bytes_to_hex(string_to_bytes($s))`, which answers in UTF-8 BYTES, not
//! codepoints — so `ord("é")` had no spelling at all. They take the
//! `\u{...}` literal's validity rule exactly: surrogates and >0x10FFFF are
//! not characters and must raise rather than round-trip to a wrong one.

use mix::evaluator::{Evaluator, SharedBuf};
use mix::lexer::Lexer;
use mix::parser::Parser;

async fn run(source: &str) -> Result<String, String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().map_err(|e| e.to_string())?;
    let mut parser = Parser::new(tokens, source);
    let stmts = parser.parse_program().map_err(|e| e.to_string())?;
    let stdout = SharedBuf::new();
    let stderr = SharedBuf::new();
    let mut eval = Evaluator::with_output(Box::new(stdout.clone()), Box::new(stderr.clone()));
    eval.execute(&stmts).await.map_err(|e| e.to_string())?;
    Ok(stdout.to_string_lossy())
}

#[tokio::test]
async fn ord_and_chr_cover_ascii_latin1_and_bmp() {
    let out = run("print(ord(\"A\"))\nprint(ord(\"é\"))\nprint(ord(\"❤\"))\n\
                   print(chr(65))\nprint(chr(233))\nprint(chr(10084))\n")
        .await
        .unwrap();
    assert_eq!(out, "65\n233\n10084\nA\né\n❤\n");
}

#[tokio::test]
async fn ord_reads_the_first_character_not_the_first_byte() {
    // The distinction the byte route could not make: "éa" is 3 bytes, and
    // its first BYTE is 0xC3, which is not a codepoint of anything.
    let out = run("print(ord(\"éa\"))\nprint(chr(ord(\"😀\")))\n")
        .await
        .unwrap();
    assert_eq!(out, "233\n😀\n");
}

#[tokio::test]
async fn chr_round_trips_with_the_unicode_escape() {
    // chr() is the runtime twin of `\u{...}` — the pair must agree, and
    // chr(0) must produce a real NUL (not "", which is why ord("") raises
    // rather than answering 0).
    let out = run("print(chr(0x27))\nprint(\"\\u{27}\")\nprint(len(chr(0)))\n")
        .await
        .unwrap();
    assert_eq!(out, "'\n'\n1\n");
}

#[tokio::test]
async fn ord_of_empty_raises_rather_than_answering_zero() {
    let err = run("print(ord(\"\"))\n")
        .await
        .expect_err("ord(\"\") must raise");
    assert!(err.contains("empty string"), "{err}");
}

#[tokio::test]
async fn chr_refuses_surrogates_and_out_of_range() {
    for (src, needle) in [
        ("print(chr(0xD800))\n", "surrogate"),
        ("print(chr(0xDFFF))\n", "surrogate"),
        ("print(chr(0x110000))\n", "0x10FFFF"),
        ("print(chr(-1))\n", "whole codepoint"),
        // f64 arithmetic makes a fractional codepoint reachable; `as u32`
        // would have truncated it to a plausible wrong character.
        ("print(chr(65.5))\n", "whole codepoint"),
    ] {
        let err = run(src).await.unwrap_err();
        assert!(err.contains(needle), "{src} -> {err}");
    }
}

#[test]
fn chr_and_ord_raise_structured_value_errors() {
    // The codes are part of the contract: a script's try/catch matches on
    // them, so a bare runtime error would be a silent change. Called
    // directly, not through the evaluator, so the structured payload is
    // visible rather than flattened into the message string.
    use mix::builtins::call_builtin;
    use mix::value::Value;
    for n in [-1.0, 65.5, f64::NAN, f64::INFINITY, 0xD800 as f64, 0x110000 as f64] {
        let Err(err) = call_builtin("chr", vec![Value::Number(n)]) else {
            panic!("chr({n}) must raise");
        };
        let info = err
            .info()
            .unwrap_or_else(|| panic!("chr({n}) must be structured: {err}"));
        assert_eq!(info.code, "VALUE_ERROR", "chr({n})");
    }
    let Err(err) = call_builtin("ord", vec![Value::String(String::new())]) else {
        panic!("ord(\"\") must raise");
    };
    assert_eq!(err.info().map(|i| i.code.as_str()), Some("VALUE_ERROR"));
}

#[test]
fn chr_accepts_the_whole_range_edges() {
    // The inclusive edges of the valid range, and the private-use glyph
    // the icon-font use case needs, all round-trip through ord.
    use mix::builtins::call_builtin;
    use mix::value::Value;
    for n in [0.0, 0xD7FF as f64, 0xE000 as f64, 0xE872 as f64, 0x10FFFF as f64] {
        // `ref`: Value implements Drop, so the String cannot be moved out.
        let Ok(Some(Value::String(ref s))) = call_builtin("chr", vec![Value::Number(n)]) else {
            panic!("chr({n}) must answer a string");
        };
        assert_eq!(s.chars().count(), 1, "chr({n})");
        let Ok(Some(Value::Number(back))) = call_builtin("ord", vec![Value::String(s.clone())]) else {
            panic!("ord(chr({n})) must answer a number");
        };
        assert_eq!(back, n);
    }
}
