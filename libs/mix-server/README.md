# mix-server (lab)

The inbound family of Mix builtins. The embedding program calls
`mix_server::register()` before evaluating source. Cargo features do not
register anything: a workspace build cannot give the shell inbound capability.

TCP listeners, accept readers, UDP listener creation and the blocking HTTP/UDP
servers live here. Connected sockets, bounded native queues and opaque
generation ownership stay in core through `mix::server_api`. The provisional
runtime API is `mix::extensions::{BuiltinFamily, SocketExtension, register}`.

Existing core tests use a dev dependency on this crate and explicit test-only
registration. Production `system` has no normal dependency on it.
