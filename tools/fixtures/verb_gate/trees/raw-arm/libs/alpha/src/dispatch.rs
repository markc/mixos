pub fn run(verb: &str) -> u8 {
    match verb {
        r#"alpha.ping"# => 1,
        "alpha.other" => 2,
        _ => 0,
    }
}
