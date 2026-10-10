// Fixture: a multi-line cfg_attr path to the listed file. The attribute is read as nothing, and
// fake.rs is scanned in full, so its beta.ghost is unregistered. Not compiled.

#[cfg_attr(
    all(),
    path = "fake.rs"
)]
pub mod fake;
