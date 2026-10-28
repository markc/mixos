# clipd — session clipboard

`clipd --session vt1|vt3|vt4` owns the clipboard service on that graphical
session's broker, under the name `clipd`. It connects using `WAYLAND_DISPLAY`
and `XDG_RUNTIME_DIR`, prefers ext-data-control-v1 and uses wlr data control
v2 only when ext is absent. `--seat NAME` overrides the first non-agent seat.
It watches both clipboard and PRIMARY without requiring keyboard focus.

## Commands and events

| Command | Body and result |
| --- | --- |
| `clipboard.get` | `{mime?, primary?, offset?}` → `{mime,data,encoding,size,offset,next_offset,eof,sensitive,stamp}` |
| `clipboard.set` | `{mime,data,encoding?,primary?,sensitive?,stamp?}`; encoding is `utf8` (default) or `base64`, at most 1 MiB decoded |
| `clipboard.clear` | `{primary?}`; clears the selected clipboard |
| `clipboard.history` | `{}` → up to 50 text entries, newest first |
| `clipboard.pick` | `{n}`; sets the zero-based history entry |
| `clipboard.info` | `{}` → session, protocol, caps and configured peers |
| `clipboard.push` | `{to:"vt4"}`; the equivalent `desk`-prefixed alias is also accepted |
| `desktop.clipboard.menu` | Opens Klipper on Plasma, or the native history picker in a desk |
| `desktop.clipboard.rotate` | Chooses the next history entry |

`clipboard.changed` publishes `{session,generation,origin_session,mime_types,
size,sensitive,primary}`. It contains **metadata only**. Size is null when
unknown; for text it is the cached text size. `stamp` is
`{origin_session,generation}`; generations are UUIDs and are never reused
after a daemon restart. Replayed sets are ignored within the bounded loop
guard. A Wayland marker identifies the service's own selection echoes.

The desk history picker dispatches `clipboard.pick` through its command
registry and publishes `clipd.command.executed` with the row index and
invocation origin. These execution events contain no clipboard text.

`clipboard.get` returns at most 1 MiB per call; continue with `next_offset`
until `eof`. UTF-8 slices use `encoding:"utf8"`; arbitrary bytes, including
a slice that cuts a multibyte character, use `encoding:"base64"`.
Materialisation is capped at 64 MiB with a five-second Wayland read deadline.

History is in memory and holds only valid UTF-8 text of at most 1 MiB per
entry. `x-kde-passwordManagerHint` with value `secret`, unreadable hints and
explicit `sensitive:true` sets never enter history or cross sessions.
Content commands admit only broker-stamped local-session callers.

Mix provides `clip_get([mime])` (a UTF-8 string or binary bytes, at most 1 MiB)
and `clip_set(text[, mime])`. Binary decoding uses Mix's crypto feature,
included in the native shell. They use the local broker's `clipd`; failures raise.
The MCP tools are `clipboard_get` and `clipboard_set` and likewise target
the local session. Use the Bus commands for binary or chunked reads.

## Same-host push

Cross-session sharing is off by default. `--settings FILE` reads a schema-1
strict-data `.conf.mix` document. Provision one private random 32-byte file
for **one logical user on one host**, exposing a private copy to that user's
mapped account in each isolated desk. Each copy must be owned by its session
user with no group/other permissions. Do not distribute it to mesh nodes on
other hosts. This credential is the scope authority; hostnames and numeric
UID claims on the wire confer no authority.

```mix
{
  schema: 1,
  scope_key_file: "/home/user/.config/mixos/clipboard.scope",
  eager_text_bytes: 1048576,
  peers: {
    vt4: {target:"clipd.alpha.example.bus", images:false, follow:false}
  }
}
```

Configure the reciprocal peer on the other session. Targets are exact native
Bus routes; only vt1, vt3 and vt4 are admitted. Other nodes cannot receive or
fetch a transfer without this host/user credential. Transfer requests and
replies use authenticated encryption over the existing Bus, bind destination
and command, expire after 30 seconds and reject replayed nonces.

Cached small text travels eagerly, capped at 1 MiB total by default. The
target advertises the complete original list; uncached MIME types, larger
text and images use 64 KiB Bus chunks only when a client requests that MIME.
Both ends must allow `images:true` for images. The original source generation
must still exist until that type is materialised; the target can then serve
its bounded cache. A replaced selection,
missing service, timeout or cap failure closes the paste fd cleanly and logs
the failure. Forwarded lazy offers retain their origin stamp; fetches carry
a bounded session path and fail cleanly if a dependency cycle is detected.
Optional `follow:true` pushes local changes to that peer; imported
changes do not trigger follow. Nothing follows by default.

`clipboard.receive` and `clipboard.fetch` are internal encrypted peer
commands. Ordinary mesh calls to content commands are refused; explicit
push is the supported cross-session path.

## Units and keys

The component ships `units/clipd.service` for the host's systemd user manager
and `units/clipd@.service` for desk sessions, using the same environment file
as the compositor. Shared desks override `User=` like the other desk units.
The host user manager must inherit the graphical session's Wayland variables.
Use a service drop-in to add `--settings` for sharing. Units are templates for
`mix layout apply`; shipping them does not install or enable them.

inputd migrates the existing Right Ctrl+Down/Up clipboard rows from the old
`desktop-vt1` target to `clipd`, retaining their menu/rotate actions. Custom
targets and unrelated user bindings are preserved. Plasma keeps Klipper's
history UI through its existing session API; desks use the native picker.
clipd does not take ownership just to observe a selection.
