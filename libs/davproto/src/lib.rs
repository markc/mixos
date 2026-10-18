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

/// Reject retired raw representation keys, even when their value is null or
/// the replacement key is also present. Call before reading or changing data.
pub fn validate_raw_keys(data: &serde_json::Value) -> anyhow::Result<()> {
    for (retired, current) in [
        (ical::RETIRED_ICAL_KEY, ical::RAW_ICAL_KEY),
        (vcard::RETIRED_VCARD_KEY, vcard::RAW_VCARD_KEY),
    ] {
        if data.get(retired).is_some() {
            anyhow::bail!(
                "retired wire key {retired:?} present: migrate the stored object to {current:?}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ical, vcard};

    #[test]
    fn new_raw_keys_round_trip_losslessly() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:event\r\n\
                   SUMMARY:Meeting\r\nRRULE:FREQ=WEEKLY\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let event = ical::parse_ics(ics).unwrap();
        assert_eq!(event.data["mixos:rawICalendar"], ics);
        assert!(event.data.get(ical::RETIRED_ICAL_KEY).is_none());
        assert_eq!(ical::raw_ical(&event.data).unwrap(), Some(ics));
        assert_eq!(
            ical::event_to_ics("event", None, None, None, None, &event.data).unwrap(),
            ics
        );

        let vcf = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:contact\r\nFN:Person\r\n\
                   X-CUSTOM:preserved\r\nEND:VCARD\r\n";
        let contact = vcard::parse_vcf(vcf).unwrap();
        assert_eq!(contact.data["mixos:rawVCard"], vcf);
        assert!(contact.data.get(vcard::RETIRED_VCARD_KEY).is_none());
        assert_eq!(vcard::raw_vcard(&contact.data).unwrap(), Some(vcf));
        assert_eq!(
            vcard::contact_to_vcf("contact", None, None, None, &contact.data).unwrap(),
            vcf
        );
    }
}
