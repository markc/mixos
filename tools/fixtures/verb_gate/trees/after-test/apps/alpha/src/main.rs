pub fn route(verb: &str) -> u8 {
    match verb {
        "alpha.ok" => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn routes_test_only_verb() {
        let verb = "alpha.test_only";
        // Braces in literals and comments must not move the skip's end.
        let open = "{";
        let close = '}';
        match verb {
            "alpha.test_only" => assert!(true),
            _ => {}
        }
    }
}

// A production handler after the test module: the gate must still read it.
pub fn late(verb: &str) -> u8 {
    match verb {
        "alpha.ok" => 1,
        "alpha.late" => 2,
        _ => 0,
    }
}
