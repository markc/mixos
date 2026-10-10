// Fixture: the router.rs membership check as written on main. The left operand is a deref of a
// closure parameter (`*v`), so the prefix must stay in the operand. The match gives the tree its
// registrations (the anchor); it is not compiled.
fn route(verb: &str) -> u8 {
    let Some(read_only) = VERBS.iter().find(|(v, _)| *v == verb).map(|(_, ro)| *ro) else {
        return 0;
    };
    match verb {
        "alpha.ping" => 1,
        "alpha.status" => 2,
        _ => 0,
    }
}
