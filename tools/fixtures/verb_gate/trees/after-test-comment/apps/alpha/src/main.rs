pub fn route(verb: &str) -> u8 {
    match verb {
        "alpha.ok" => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    /* outer /* inner comment */ unmatched brace: { */
    #[test]
    fn routes_test_only_verb() {
        let verb = "alpha.test_only";
        let multi = "open {
close } still one string";
        match verb {
            "alpha.test_only" => assert!(true),
            _ => {}
        }
    }
}

// The brace inside the block comment above must not keep the skip open: this
// production handler after the test module is still read.
pub fn late(verb: &str) -> u8 {
    match verb {
        "alpha.ok" => 1,
        "alpha.late" => 2,
        _ => 0,
    }
}
