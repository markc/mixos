# mesh-sign

Signs and verifies the mesh inventory, the SPEC 13 trust root. It reads the
authored `inventory.mix`, builds the signed payload, signs its canonical bytes
with the genesis Ed25519 key, and checks `inventory.signed` against a genesis
public key. The manual is in [docs/mesh-sign/README.md](../../docs/mesh-sign/README.md).

Build and test on a cbc worker, never locally:

    mix ~/.mix/_bin/cbc-build.mix --worker cbc2 --repo mixos --commit <sha> -- cargo test -p mesh-sign --locked

The canonical bytes come from the `mesh-trust` crate under `services/noded`,
the same code the verifier in `noded` uses. The JSON verify report keeps its
`cosmix-mesh-sign.verify.v1` format string: it is a frozen wire constant.
