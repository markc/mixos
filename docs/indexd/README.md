---
title: The semantic index service
description: Local semantic search over the MixOS Bus, with an ssh bridge and an in-process CLI.
---

# The semantic index service

> **Status: Bus wired.** The engine, reconnecting daemon, bridge, CLI and
> system unit are implemented. Corpus parity and production cutover are
> separate steps. The tokenizer golden records the legacy 0.21.4/onig token IDs.

`indexd` stores text chunks with their embeddings and returns the nearest
matches for a query. Embeddings are computed locally on the CPU with
`nomic-ai/nomic-embed-text-v1.5` (768 dimensions), through the pure-Rust
`candle` stack. The model files are loaded from a local directory and never
downloaded at run time. Vectors live in SQLite (`sqlite-vec`), and search runs
over an in-memory copy, re-ranked by feedback and staleness.

It runs on one workstation host as a system service, registered on that
host's own node. Agents usually reach it through `mixos-mcp`
(`context_search`, `index_workspace`). A desk on the
same machine reaches it over the Bus, or over ssh through `indexd bridge`,
which reads newline-delimited JSON on stdin and makes each request as a local
Bus call.

## Asking it

On the Bus, the service is `indexd`:

```mix
send indexd indexd.stats
send indexd indexd.search query="how are service dependencies declared" limit=3
```

Each body is a JSON object. Clients may include an `action` equal to the command
suffix; a conflicting value is rejected. Replies contain the command's JSON
directly. Errors use rc 10 and a string `error` field. Metadata is always a
JSON-encoded string, including each element of `store.metadata`.

| Command | Request fields | Reply |
|---|---|---|
| `indexd.embed` | `texts`, optional `prefix` | `embeddings` |
| `indexd.store` | `texts`, `source`, `metadata` | `stored`, `duplicates`, `ids` |
| `indexd.search` | `query`, `limit`, optional `source`, `metadata_filter` | `results` |
| `indexd.update` | `id`, optional `content`, `metadata`, `source` | `updated`, `re_embedded` |
| `indexd.delete` | optional `ids`, optional `filter` (at least one non-empty) | `deleted` |
| `indexd.list` | optional `source`, `limit`, `offset` | `items`, `total` |
| `indexd.feedback` | `id`, `useful` | `id`, `feedback_score` |
| `indexd.supersede` | `old_id`, `new_id`, optional `reason` | `updated` |
| `indexd.stale` | optional `source`, `never_retrieved_age_days`, `low_value_min_retrievals`, `long_dormant_days`, `per_bucket_limit` | three buckets, `total_chunks` |
| `indexd.stats` | none | counts, model/cache/queue statistics, `by_source` |
| `indexd.index_file` | absolute `path`, optional `content`, `source`, `domain`, `background` | `indexed`, `sections`; background acknowledgement when queued |

`search` excludes superseded chunks before applying its limit. Filter entries
have `field`, `op` (`eq`, `gt`, `lt`, `gte`, `lte`, `contains`) and `value`.

Delete filters use the same predicate shape, for example
`{"filter":[{"field":"path","op":"eq","value":"/srv/project/a.md"}]}`.
Predicates are ANDed; when IDs are also supplied they restrict the matching set.
DELETE accepts only `eq`, with a non-empty string, number or boolean value.
Each predicate also requires the matching JSON type (integers and reals share
the numeric type); null, arrays and objects are refused. Search retains all
operators listed above.
Deletion updates the durable vectors and in-memory index in one operation.

### Purging retired CMM observations

Observations are ordinary rows in SQLite's `chunks` table, with
`source = "observation"`, text in `content` and a JSON string in `metadata`.
Their embeddings live in `vec_chunks` and the in-memory search index. The
source policy requires metadata fields `observer`, `tool`, `observed_at` and
`domain`. The retired CMM clippy producer used observer `cmm_cargo_clippy`.
There is no remaining observation writer in this repository; the source type
remains readable for the legacy corpus. `context_search` continues to return
these rows in its `observations` group until an operator purges them.

The existing `indexd.delete` command supports exact metadata predicates for
observer and domain. For the `mixos` domain, the exact Bus call is:

```mix
send indexd indexd.delete body='{"filter":[{"field":"observer","op":"eq","value":"cmm_cargo_clippy"},{"field":"domain","op":"eq","value":"mixos"}]}'
print($reply)
```

The same call through mixos-mcp is:

```json
{"to":"indexd","command":"indexd.delete","args":"{\"filter\":[{\"field\":\"observer\",\"op\":\"eq\",\"value\":\"cmm_cargo_clippy\"},{\"field\":\"domain\",\"op\":\"eq\",\"value\":\"mixos\"}]}"}
```

Pass that object to `bus_call` (`mcp__mixos__bus_call` in Claude Code), with
`to` set to the indexd Bus address reachable from the caller. `bus_call` uses
the Bus connection; it does not use the knowledge tools' configured SSH bridge.
The reply contains `deleted`. Other observers and domains survive, and repeating
the deletion returns zero once matching rows are gone. These are operator calls;
the retirement change does not execute them or change stored data.

`filter.field` always names a metadata field: `source` is a separate column,
so a filter on `source` does **not** select the row's source type. To restrict
deletion to observations as well, first enumerate `indexd.list` with
`{"source":"observation","limit":1000,"offset":0}`, advancing `offset` until
all pages are collected. Then pass the selected IDs together with the observer
and domain predicates to `indexd.delete`. IDs and predicates are intersected;
collect pages before deleting so offset shifts cannot skip rows. No new command
or database access is needed. The existing delete updates `chunks`, `vec_chunks`
and the in-memory index together.

## CLI and bridge

`indexd -c <config>/indexd/config.conf.mix call indexd.stats` opens the
configured engine in-process and prints one JSON reply, with exit status 0
for success or 1 for an error. Bare command names also work. For example:

```text
indexd -c <config>/indexd/config.conf.mix call indexd.search '{"query":"service dependencies","limit":3}'
```

Writes (`store`, `update`, `delete`, `feedback`, `supersede`, `index_file`)
require `call --offline` with the daemon stopped: otherwise its in-memory
index would desynchronise. Read commands need no flag. `call` always refuses
`background:true` because no process remains to run the queued job.

`indexd bridge` connects to the local node's Bus once and forwards one call
per stdin line, flushing each reply immediately. It does not open the index
or require model configuration. Keep one process alive for repeated requests,
including through `ssh -T user@example.test <prefix>/bin/indexd bridge`.
The bridge honours non-empty `MIXOS_NODED_URL` first. Otherwise it reads
`--node-config PATH`, then non-empty `MIXOS_NODE_CONFIG`, defaulting to
`<prefix>/etc/node.conf.mix`, the same file pinned by the system unit.
It derives the broker address from that file rather than user config discovery.

```json
{"id":1,"command":"indexd.search","body":{"query":"service dependencies","limit":3}}
```

Each stdout line is `{"id":1,"rc":0,"body":{...}}`, with the original JSON
`id` preserved. Application errors retain rc 10 and the JSON error body.
Transport failures use `{"id":1,"rc":-1,"error":"..."}` and then exit non-zero,
including initial connection failure, so a supervisor can respawn it. EOF exits 0;
stdout contains only protocol replies. The bridge never retries a call.

Requests may carry `timeout_secs`, default 60 and capped at 600 seconds.
A timeout returns `{"id":1,"rc":-1,"kind":"timeout","error":"..."}` and
leaves the bridge alive. The operation's outcome is unknown. Empty Bus bodies
become JSON `null`; Bus error headers are forwarded as the top-level `error`.

## Operating the system service

`indexd serve` registers service `indexd` using the standard Bus connection
helper, advertises the eleven commands above, and reconnects with backoff.
`MIXOS_NODE_CONFIG` selects the system broker, as for dnsd. The supplied unit
pins `<prefix>/etc/node.conf.mix`; no broker URL is embedded in indexd.
SIGTERM stops admission and completes accepted Bus calls before exit.
The model loads lazily on the blocking pool under the inference permit and
unloads after the configured idle interval (300 seconds by default).

Configuration lookup is `-c/--config`, then
`<config>/indexd/config.conf.mix`, then
`$XDG_CONFIG_HOME/mixos/indexd.conf.mix` (or the equivalent under `$HOME/.config`).
`indexd -c FILE check-config` validates the local `config.json`,
`tokenizer.json` and `model.safetensors` before loading any weights. HF snapshot
symlinks are supported. There is no database environment override;
`service.vectors_db` is authoritative. `MIXOS_INDEXD_SEARCH` remains the
engine's one-release rollback switch (`sqlite` selects SQLite search).

The default domain map is empty, so unmapped files use `general`. Configure
absolute prefixes; the daemon does not expand `~` in domain keys or file paths.
The sample at [`services/indexd/examples/config.conf.mix`](https://github.com/markc/mixos/blob/main/services/indexd/examples/config.conf.mix) includes:

```text
domains: {map: {
  "/home/user/Projects/mixos": "mixos",
  "/home/user/.mix": "mix",
  "/home/user/.ns": "ns",
  "/home/user/.mc": "mc"
}}
```

Replace `/home/user` with the operator's absolute home path. Set `model_dir`
to the pre-seeded snapshot; no model download happens at runtime.
`ProtectHome=yes` hides home directories from the service: callers must pass
`index_file.content` for those paths, or use `store`. `index_file` rejects a
tilde-prefixed path even when content is supplied.

The component owns `indexd.service` and `sysusers.conf`; the desktop release
bundle carries the standalone binary and these files under `share/indexd/units/`,
with the sample config under `share/indexd/examples/config.conf.mix`,
following the component-owned maild packaging pattern. Install the binary
under `<prefix>/bin`, the unit under `/etc/systemd/system`, sysusers under
`/usr/lib/sysusers.d/indexd.conf`, and a customised config under
`<config>/indexd/config.conf.mix`. Apply sysusers before starting the service.
The unit wants system `noded.service`, runs as `indexd:indexd`, limits memory
to 2 GiB, sets `MALLOC_ARENA_MAX=2`, and hardens filesystem, devices and kernel
access. TCP address families remain available for the configured broker.
`MemoryDenyWriteExecute=yes` follows the former candle CPU service.
systemd creates the state and configuration directories, so no tmpfiles entry
is needed. Before cutover, back up existing data and explicitly change its
ownership from the legacy account; the new account does not reuse its UID.

## Tests

On a CBC worker, `cargo test -p indexd` runs the engine tests, registry checks,
real daemon/bridge pipe tests, and `services/indexd/tests/bus_test.mix` against
`libs/test-broker`. Set `MIXOS_TEST_MIX` if Mix is not installed at
`<prefix>/bin/mix`. Embeddings are deterministic cache fixtures; no model
is needed. The tokenizer parity test compares 0.22.2 token IDs against the
committed legacy 0.21.4/onig golden; it stays ignored unless
`INDEXD_TEST_TOKENIZER` names the staged tokenizer file.

## Indexd is not exhaustive

indexd answers "what is relevant to X", quickly and approximately, from an
index that can lag the files. To prove that something does **not** occur, use
an exact search over the current files instead.
