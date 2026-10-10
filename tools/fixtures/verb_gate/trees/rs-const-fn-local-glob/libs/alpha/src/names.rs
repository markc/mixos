// Fixture (round 25, sol's case): names.rs re-exports a glob and a helper fn holds a local const of
// the same name. Rust reads crate::names::ALPHA_PING through the glob (other::ALPHA_PING), never the
// fn-local const, so the path is unreadable (fail closed). Not compiled.
pub use crate::other::*;

fn helper() -> &'static str {
    const ALPHA_PING: &str = "alpha.ping";
    ALPHA_PING
}
