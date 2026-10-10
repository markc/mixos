pub fn http(method: &str) -> u8 {
    match method {
        "GET" => 1,
        "HEAD" => 2,
        "alpha.unreg_match" => 3,
        _ => 0,
    }
}
