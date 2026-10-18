# maild

Mail service foundation. This phase provides the private `mds` storage crate;
there is no daemon binary or registered Bus verb yet.

See the [manual](../../docs/maild/README.md) and
[storage API](crates/mds/README.md). Test with `cargo test -p mds` on a build worker.
