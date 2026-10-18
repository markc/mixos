// Temporary probe: prints the sealed-format digests and length for the
// default desktop. Deleted once the constants in sealed_format.rs are re-pinned.
use settings::*;

fn fixed_snapshot() -> Snapshot {
    let desktop = Desktop::default();
    let effective = resolve(&desktop).unwrap();
    Snapshot {
        schema: SCHEMA,
        binding: Binding {
            instance: "fixed".into(),
            profile: "default".into(),
        },
        incarnation: "00000000-0000-0000-0000-000000000001".into(),
        revision: Revision(1),
        design_revision: Revision(1),
        source_digest: source_digest(EMBEDDED_DEFAULT_SOURCE),
        desktop,
        effective,
    }
}

#[test]
fn print_digests() {
    let snapshot = fixed_snapshot();
    eprintln!("EFFECTIVE={}", digest(&snapshot.effective).unwrap());
    eprintln!("SNAPSHOT={}", digest(&snapshot).unwrap());
    eprintln!("SNAPSHOT_LEN={}", serde_json::to_string(&snapshot).unwrap().len());
}
