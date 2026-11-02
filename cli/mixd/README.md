# mixd lab host

`mixd run <script.mix> [args]` runs the full Mix interpreter including inbound
TCP, UDP and HTTP servers. `mixd --personalities` lists the compiled Rust
services; `mixd <personality> [args]` or a symlink selects their existing mains.

Build independently with `cargo build -p mixd --release` on CBC. A workspace
build unifies its server features into `system` too. This prototype has no
reactor and does not change installed units or binaries.
