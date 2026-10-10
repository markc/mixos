fn dispatch(verb: &str) -> u8 {
    match verb {
        "alpha.ping" => 1,
        "alpha.build" => 2,
        _ => 0,
    }
}
