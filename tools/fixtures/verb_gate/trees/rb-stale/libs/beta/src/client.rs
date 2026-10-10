// No match on a verb here: the request builder entry matches nothing and is stale.
fn plain(verb: &str) -> bool {
    verb.is_empty()
}
