pub fn run(verb: &str) -> u8 {
    if verb == r#"alpha.ping"# {
        return 1;
    }
    match verb {
        "alpha.other" => 2,
        _ => 0,
    }
}
