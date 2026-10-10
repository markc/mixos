// Fixture (round 25, the variant without the glob): the only ALPHA_PING in names.rs is a fn-local
// const, which is not an item of the module, so crate::names::ALPHA_PING is unreadable (fail
// closed). Not compiled.

fn helper() -> &'static str {
    const ALPHA_PING: &str = "alpha.ping";
    ALPHA_PING
}
