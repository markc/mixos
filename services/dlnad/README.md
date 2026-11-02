# dlnad

A DLNA / UPnP AV media server for MixOS, written in Mix: it serves one media directory to TVs and other renderers, and answers on the Bus.

Documentation: <https://mixos.dev/dlnad/> — source [`docs/dlnad/README.md`](../../docs/dlnad/README.md).

Requires Mix 0.118.0 or later for offset file reads and discovery sockets.

Offline tests: `mix services/dlnad/tests/dlna_test.mix`,
`mix services/dlnad/tests/oracle_test.mix` (sanitised VuIO captures),
`mix services/dlnad/tests/limits_test.mix` (resource policy), and
`mix services/dlnad/tests/gena_test.mix` (eventing).

Change events: an inotify watch on the media directory raises
`SystemUpdateID` and sends a GENA `NOTIFY` to each subscribed renderer
(`lib/gena.mix`); hidden paths such as syncthing temp files are ignored.
Sorting is fixed ascending (folders first), with empty `SortCaps`.
`max_conns` (default 64) evicts the oldest idle connection when full, or closes
the newcomer if all are busy. Renderer history keeps the 256 most recently
seen peers. `announce_s`, `max_age` and `max_conns` must be positive numbers;
`max_conns` must also be an integer.
