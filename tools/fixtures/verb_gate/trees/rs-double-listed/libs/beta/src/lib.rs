// Fixture: beta's crate root declares its fake editd only under test or the fake feature.
// Not compiled.

#[cfg(any(test, feature = "fake"))]
pub mod fake;
