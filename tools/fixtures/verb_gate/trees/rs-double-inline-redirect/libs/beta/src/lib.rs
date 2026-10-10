// Fixture: Sol's inline redirect (round 21). The attribute's path would name fake.rs, but the
// gate resolves no module path, so fake.rs is scanned as any file: its answer to alpha.ping is
// registered and is not an owner, and nothing here fails. Not compiled.

#[cfg_attr(all(), path = ".")]
mod production {
    pub mod fake;
}
