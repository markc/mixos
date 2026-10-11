# vendor/calloop: local patches

calloop 0.14.5 from crates.io, plus a composed-channel wakeup fix. Wired
through `[patch.crates-io] calloop = { path = "vendor/calloop" }`.

## Upstream base: crates.io `calloop` 0.14.5

- Archive: `https://static.crates.io/crates/calloop/calloop-0.14.5.crate`
- SHA-256: `43b32313e937c8721c9d17ee9bf60699a6402b4b7c2cb6ac1bee0ffcaa4b4e5e`
- Upstream commit: see `.cargo_vcs_info.json`.

Method: download the archive, check its SHA-256, untar, `diff -r` against
this directory. Exactly two files differ: `src/sources/channel.rs` and this
`PATCHES.md`. `Cargo.toml`, `Cargo.toml.orig` and `Cargo.lock` are
byte-identical, so dependency resolution is the same as stock 0.14.5.

## Local edits

| file | +/− | what |
|---|---|---|
| `src/sources/channel.rs` | +57/−2 | Runtime fix plus its regression test, described below |

The fix is in `<Channel<T> as EventSource>::process_events`. It adds a
`matched` flag that is set inside the ping callback. The bounded-batch re-ping
(`self.ping.ping()`) now fires only when `matched && !clear_readiness`, which
means this channel serviced its own token and hit the batch limit.

Stock 0.14.5 (like 0.14.4) also re-pings when the readiness token is not its
own. A composite source forwards every token to every child, so two child
channels ping each other forever. That is a busy loop in an event-driven
compositor. The rest of the hunk (+51) is the regression test.

## Guard

The regression test ships inside the patched file:
`sources::channel::tests::composed_channels_do_not_ping_each_other_for_unrelated_tokens`.
It wraps two channels in one composite source, sends one message and runs
eight non-blocking dispatches. It asserts one message and exactly one
composite dispatch. Stock 0.14.5 produces eight dispatches.

```
cargo test --manifest-path vendor/calloop/Cargo.toml composed_channels
```

A wholesale re-vendor removes the test along with the fix. After any
re-vendor, check from the repo root that the fix is still there. It must
print `true`:

```
mix -c 'print(contains(read_file("vendor/calloop/src/sources/channel.rs"), "} else if !matched || clear_readiness {"))'
```

Also check that `cargo tree -i calloop@0.14.5` resolves to `vendor/calloop`.
Drop the patch once upstream's channel stops re-pinging on a token it does not
own.
