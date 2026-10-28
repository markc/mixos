# clipd — clipboard service

`mix --serve <prefix>/lib/clipd/clipd.mix --name clipd` runs a clipboard Bus
citizen. There is no clipd Rust crate, GUI, Wayland dependency in Mix, or new
transport. Store mode holds data in memory. Wayland mode uses the session's
external `wl-paste` and `wl-copy` executables.

This implementation is **not ready for live desktop acceptance**: wl-copy
cannot advertise a remote lazy offer or run a fetch callback at paste time.
Remote password offers and uncached remote types are refused in Wayland mode
with `LAZY_WAYLAND_UNAVAILABLE`. They work lazily through the Bus in store mode.
Local Wayland password sets are also refused because wl-copy spools stdin to a
temporary file. Observing an existing password offer does not read its content.

## Commands

Requests are JSON objects. `selection` is `clipboard` (default) or `primary`.
The Rust prototype's `primary:true|false` alias remains accepted; conflicting
aliases are refused. Errors are rc 10 with `{error_code,message}`. Error messages
and service logs never echo request contents.

| Command | Request and reply |
| --- | --- |
| `clipboard.get` | `{selection?,mime?,offset?,node?}` → `{mime,data,encoding,size,offset,next_offset,eof,sensitive,stamp}` for text; a `blob` reference for binary/large data |
| `clipboard.set` | `{selection?,mime?,data,encoding?,sensitive?,stamp?}` or `{selection?,mime,blob}` → `{applied,stamp?}` |
| `clipboard.clear` | `{selection?}` → `{applied:true}` |
| `clipboard.info` | Both selection metadata, node/session, history count, limits, peers and mode |
| `clipboard.history` | Up to fifty text previews, newest first: `{id,mime,data,size,truncated}` |
| `clipboard.pick` | `{index,expected_id?,selection?}`; `n` remains an alias for index |
| `clipboard.push` | `{to,selection?}` pushes only to a configured peer key |
| `clipboard.peers` | Configured peer map |
| `desktop.clipboard.rotate` | Restore a history entry |
| `desktop.clipboard.menu` | `PICKER_UNAVAILABLE`; use history and indexed pick |

`clipboard.receive` and `clipboard.fetch` are member-only peer commands.
`clipboard.observe` is a private local watcher callback with a per-process
token. `clipboard.expire` and `clipboard.follow` are local continuations and
refuse Bus dispatch.

`clipboard.changed` contains metadata only:
`{node,session,selection,primary,generation,origin_node,origin_session,mime_types,
size,sensitive}`. A stamp is `{origin_node,origin_session,generation}`. UUIDs
survive process restarts without reusing generations. The last 256 stamps and
the current selection fence echoes, independently for both selections.

Small text retains the prototype's UTF-8/base64 reply shape, including byte
offsets and `eof`. Binary/large content uses blobd's existing reference and
transfer lane: get returns `data:"",encoding:"base64",eof:false,blob:{...}`.
Call `blob.fetch` and `blob.path` rather than advancing `next_offset` on a blob
reply. Set binary data through `blob.put` followed by `clipboard.set {blob}`;
inline non-text bytes are refused. This is a deliberate difference from the
unmerged Rust prototype's binary Bus chunks, required by the blob-only rule.

Mix's Bus-only builtins are `clip_get([selection,mime])` and
`clip_set(text[,selection,mime])`. A sole MIME argument remains accepted.
`clip_get` raises for a blob reference. The MCP tools are `clipboard_get` and
`clipboard_set`, with optional `selection` and the legacy `primary` alias.

## Configuration and membership

`MIXOS_CLIPD_CONFIG` selects a schema-1 strict-data file; the default is
`<etc>/clipboard.conf.mix`. Peer keys are exact node names. Nothing is sent
outside this configured group. A graphical member has `follow:true`; server
entries default to explicit push/pull. PRIMARY follow requires
`follow_primary:true` at the source. Images are enabled for follow members and
require `images:true` for servers.

```mix
{
  schema: 1,
  eager_text_bytes: 1048576,
  history_cap: 50,
  password_timeout: 45,
  follow_primary: false,
  peers: {
    beta: {target:"clipd.beta.example.bus", follow:true},
    archive: {target:"clipd.archive.example.bus", images:false}
  }
}
```

Configure reciprocal peers. Content APIs admit broker-stamped local callers
and configured mesh peers. Membership comes from the broker's `broker_peer`
stamp, never a body field or a client-supplied node claim. A bare service target
also admits that broker-stamped local service, for co-located citizens and tests.
Use native noded admission and the WireGuard mesh. Missing broker peer stamps
fail closed. No scope keys or parallel encryption protocol are introduced.

Follow forwards local changes once; imported changes are not re-exported.
Password offers contain metadata only, retain their source and deadline, never
enter history or blobd, are fetched on demand, and clear after 45 seconds by
default. Store servers do not cache fetched secret bytes. Generation fences
refuse stale lazy reads. Replaced or missing sources fail cleanly.

## Session integration and external programs

`MIXOS_CLIPD_MODE=auto|store|wayland` defaults to auto. No display selects store;
a display selects Wayland and requires both external tools. Store mode makes
no wl-clipboard calls. `MIXOS_CLIPD_SESSION` identifies the session. The node
identity comes from the native node config. Optional config `seat` selects the
Wayland seat through the tools' `--seat` option.

The **desktop role** requires `wl-paste` and `wl-copy` from wl-clipboard in PATH;
the **server role** needs neither. Both need Mix and noded; binary content also
needs blobd. wl-clipboard is an external system package and is not bundled,
compiled or linked into MixOS binaries.

Two generation-owned `spawn(argv,{exit_event:true})` children run
`wl-paste [--primary] --watch <prefix>/bin/mix watch.mix <selection>`. The callback
does not read stdin: it sends a local Bus notification, then closes the pipe.
The citizen lists types and reads bounded ordinary text. The KDE password hint
is treated conservatively as sensitive whenever present. On-demand reads use
`wl-paste --type MIME --no-newline`, with a five-second deadline and 64 MiB cap;
a Mix pipeline helper preserves raw bytes without a temporary read file.

Ordinary sets pass stdin to `wl-copy [--primary] --type MIME`, which forks and
owns the selection. Clears use `wl-copy [--primary] --clear`. Own eager-text
echoes are recognised by exact bytes, because wl-copy cannot add an arbitrary
origin marker MIME. Large/binary ownership and multi-MIME preservation need
further acceptance: the tool can supply one content payload plus text aliases.

`clipd@.service` reads the graphical session environment; `clipd.service` is
the server store unit. The component installer and release packages ship both
units and scripts without enabling them. No desk is restarted by installation
of this source change. inputd's existing target migration is a separate live
integration step; this branch does not alter user keymaps.

No generic Bus dialog/choice service exists in the current tree. Toolkit dialogs
are window-local, and files' chooser is specific to files. A clipboard picker
UI remains a follow-up; the service adds no GUI dependency.

## Validation

`mix services/clipd/tests/clipboard_test.mix` checks policy, history, stamps,
byte offsets and broker membership. `mix services/clipd/tests/clipd_test.mix`
runs two store citizens against an owned private noded and checks the public API,
follow, lazy password fetch, no history, expiry and spoof refusal.

Before merge: CBC tests for mix and mixos-mcp; a private headless Wayland test;
then live VT1/VT3/VT4 and cross-node acceptance by the session owner. The lazy
Wayland ownership limitation must be resolved or explicitly scoped before
desktop rollout. No gold install, desk restart or merge is authorised here.
