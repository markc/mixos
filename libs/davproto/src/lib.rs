// SPDX-FileCopyrightText: 2026 Mark Constable <mc@mixos.dev>
// SPDX-License-Identifier: MIT OR Apache-2.0

//! CalDAV/CardDAV protocol codecs, shared by the maild DAV server and a
//! future DAV client.
//!
//! v1 scope:
//! * **emit** JSCalendar (RFC 8984) → iCalendar VEVENT (RFC 5545) and
//!   JSContact (RFC 9553) → vCard 4.0 (RFC 6350) — the read path.
//! * **strong ETags** — a content hash so a resource's ETag is stable
//!   across reads and changes iff its content changes.
//!
//! The reverse direction (iCal/vCard → JSCalendar/JSContact, the write
//! path) lands in a later change.

pub mod etag;
pub mod ical;
pub mod vcard;
