// Fixture: a raw-identifier alias of the listed file, under an unconditional cfg_attr path. The
// alias changes nothing: fake.rs is scanned in full, and its beta.ghost is unregistered. Not compiled.

#[cfg_attr(all(), path = "fake.rs")]
pub mod r#type;
