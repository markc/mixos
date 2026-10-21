// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Stage 7 supplies token IDs produced by the pinned predecessor tokenizer.

#[test]
#[cfg_attr(
    not(indexd_test_tokenizer),
    ignore = "INDEXD_TEST_TOKENIZER is unset; stage 7 supplies the tokenizer and golden"
)]
fn tokenizer_ids_match_predecessor_golden() {
    let Some(path) = std::env::var_os("INDEXD_TEST_TOKENIZER") else {
        eprintln!("skipped: INDEXD_TEST_TOKENIZER is unset");
        return;
    };
    let tokenizer = tokenizers::Tokenizer::from_file(path).expect("read local tokenizer.json");
    let corpus = include_str!("tokenizer_corpus.txt");
    let actual: Vec<Vec<u32>> = corpus
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|text| {
            tokenizer
                .encode(text, true)
                .expect("tokenize corpus")
                .get_ids()
                .to_vec()
        })
        .collect();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tokenizer_golden.json");
    let golden: Vec<Vec<u32>> = serde_json::from_str(&std::fs::read_to_string(path).expect(
        "stage 7 must generate tests/tokenizer_golden.json with pinned predecessor token IDs",
    ))
    .expect("golden is a JSON array of token-ID arrays");
    assert_eq!(actual, golden, "tokenizer ID parity changed");
}
