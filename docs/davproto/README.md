---
title: davproto
description: CalDAV and CardDAV codecs that convert between JSCalendar or JSContact and iCalendar or vCard, with strong ETags, and no transport or storage.
---

# davproto

CalDAV and CardDAV codecs: the wire-format side of a DAV server or client,
with no transport and no storage.

- `ical`: JSCalendar (RFC 8984) to iCalendar VEVENT (RFC 5545), and the
  reverse mapping for a DAV PUT. A verbatim iCalendar body is kept under a
  data key so a GET returns it unchanged.
- `vcard`: JSContact (RFC 9553) to vCard 4.0 (RFC 6350), with the same
  verbatim round-trip.
- `etag`: strong ETags, a BLAKE3 hash of the content, so an ETag changes
  exactly when the content does.

The crate is a pure library. It does not open sockets, read files or touch
a database. Callers (maild's DAV server, a future DAV client) own those.

The source is [`libs/davproto`](https://github.com/markc/mixos/tree/main/libs/davproto).

## Test

Run the unit tests on a build worker, not locally:

    cargo test -p davproto --locked
    cargo clippy -p davproto --all-targets --locked -- -D warnings
    cargo fmt -p davproto -- --check
