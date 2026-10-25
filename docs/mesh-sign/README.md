---
title: mesh-sign
description: Signs and verifies the mesh inventory, the trust root of a MixOS mesh.
---

# mesh-sign

`mesh-sign` turns the authored `inventory.mix` into the signed trust root,
`inventory.signed`, and checks a signed inventory against the genesis public
key. The signing and verifying code is the same code the mesh daemon `noded`
uses, so the two cannot disagree about the bytes a signature covers.

Install the binary at `<prefix>/bin/mesh-sign`.

## Subcommands

| Command | What it does |
|---|---|
| `genesis` | Create the genesis Ed25519 key and store its signing half in the secrets database. It refuses to overwrite an existing key unless `--force` is given. |
| `sign INVENTORY --out PATH` | Sign an authored inventory and verify the result before reporting success. `--valid-days` sets an advisory horizon. `--recovery N` and `--recovery-generation N` set the recovery fields and are mutually exclusive. |
| `verify SIGNED` | Check a signed inventory against the genesis key. It takes `--genesis-pub PATH` or `--genesis-pub-base64 B64`, or else reads the key from the secrets database. `--expected-mesh NAME` pins the mesh name, and `--against-authored PATH` checks the signed payload against an authored file. `--json` prints one versioned JSON object. |
| `pubkey` | Print the genesis public key, which is provisioned to each node as `<prefix>/etc/noded/genesis.pub`. |
| `d2-gen NODE` / `d2-pubkey NODE` | Create or print a node's admission key. |
| `wg-gen --private-file PATH` | Create a node-local WireGuard key in a new mode-0600 file and print only the public key. |
| `wg-pubkey --private-file PATH` | Print the public key of an existing protected node key. |

Global options: `--secrets-db PATH` (default `~/.ns/_etc/secrets/secrets.db`)
and `--mesh NAME` (default `bus`). `mesh-sign --version` prints the build
provenance.

## Verify report

`verify --json` prints an object whose `format` is `cosmix-mesh-sign.verify.v1`.
That string is a frozen wire constant. It keeps its original spelling so that
existing consumers still match it.

## Testing

Build and test on a cbc worker, not on the desk:

    mix ~/.mix/_bin/cbc-build.mix --worker cbc2 --repo mixos --commit <sha> -- cargo test -p mesh-sign --locked

The integration suite (`cli/mesh-sign/tests/verify_cli.rs`) runs the binary
against generated keys and inventories in a temporary directory.
