// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Calendars, on maild's shape: the type is `CalendarEvent` (not RFC 8984's
//! `Event`), with the indexed `title`/`start`/`end`/`updated` columns at
//! the top level and the full JSCalendar object nested under `data`. The
//! list renders from the indexed columns; detail reads into `data` as
//! needed. A `cosmix:rawICalendar` key inside `data` (verbatim iCal from a
//! CalDAV write) is ignored: the JMAP object is the truth.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{
    CREATE, Call, Client, check_confirmed, check_set_reply, non_empty, optional_text, require_uuid,
};
use crate::error::{Error, Result};

/// What a new event says, as a JSCalendar Event. `start` and `end` are
/// RFC 3339 instants with an offset: maild indexes only those for range
/// queries, so any other form is refused rather than stored unfindable.
/// They are sent normalised to UTC; `duration` is computed from them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventDraft {
    pub title: String,
    /// e.g. `2026-10-14T08:00:00+10:00`.
    pub start: String,
    /// An RFC 3339 instant with an offset, after `start`.
    pub end: String,
    /// An IANA zone name, stored as the event's `timeZone`.
    pub time_zone: Option<String>,
    /// All-day: the JSCalendar `showWithoutTime` flag.
    pub show_without_time: bool,
    pub location: Option<String>,
    pub description: Option<String>,
}

/// The changes to an existing event. Only the fields that are `Some` are
/// patched; an empty `time_zone`, `location` or `description` clears it.
///
/// maild currently persists only `title`, `start` and `end` on update: the
/// other fields are sent but silently ignored by the server. `start` and
/// `end` are RFC 3339 instants with an offset, as for [`EventDraft`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventPatch {
    pub title: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub time_zone: Option<String>,
    pub show_without_time: Option<bool>,
    pub location: Option<String>,
    pub description: Option<String>,
}

impl EventDraft {
    /// The JSCalendar Event this draft creates in `calendar`, under `uid`.
    /// `calendarId` is the singular top-level key maild requires.
    fn to_jscalendar(&self, uid: &str, calendar: &str) -> Result<Value> {
        let start = utc_instant("start", &self.start)?;
        let end = utc_instant("end", &self.end)?;
        let duration = duration_between(&start, &end).map_err(Error::Protocol)?;
        let mut event = json!({
            "@type": "Event",
            "uid": uid,
            "calendarId": calendar,
            "title": self.title,
            "start": start,
            "end": end,
            "duration": duration,
            "showWithoutTime": self.show_without_time,
        });
        if let Some(time_zone) = non_empty(self.time_zone.as_deref()) {
            event["timeZone"] = json!(time_zone);
        }
        if let Some(location) = non_empty(self.location.as_deref()) {
            event["locations"] = locations(location);
        }
        if let Some(description) = non_empty(self.description.as_deref()) {
            event["description"] = json!(description);
        }
        Ok(event)
    }
}

impl EventPatch {
    /// The JSCalendar patch: each set field, an empty text field as `null`.
    fn to_patch(&self) -> Result<Value> {
        let mut patch = json!({});
        if let Some(title) = &self.title {
            patch["title"] = json!(title);
        }
        if let Some(start) = &self.start {
            patch["start"] = json!(utc_instant("start", start)?);
        }
        if let Some(end) = &self.end {
            patch["end"] = json!(utc_instant("end", end)?);
        }
        if let Some(time_zone) = &self.time_zone {
            patch["timeZone"] = optional_text(time_zone);
        }
        if let Some(show_without_time) = self.show_without_time {
            patch["showWithoutTime"] = json!(show_without_time);
        }
        if let Some(location) = &self.location {
            patch["locations"] = match non_empty(Some(location)) {
                Some(location) => locations(location),
                None => Value::Null,
            };
        }
        if let Some(description) = &self.description {
            patch["description"] = optional_text(description);
        }
        Ok(patch)
    }
}

/// An RFC 3339 instant with an offset, normalised to UTC with a `Z` suffix.
/// Anything else is refused: a LocalDateTime is stored by maild but never
/// time-indexed, so a range query would miss it.
fn utc_instant(field: &str, text: &str) -> Result<String> {
    let instant = chrono::DateTime::parse_from_rfc3339(text).map_err(|e| {
        Error::Protocol(format!(
            "{field} {text:?} is not an RFC 3339 instant with an offset: {e}"
        ))
    })?;
    Ok(instant
        .with_timezone(&chrono::Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// The one-entry `locations` map JSCalendar gives a single location name.
fn locations(name: &str) -> Value {
    json!({ "l1": { "@type": "Location", "name": name } })
}

/// The ISO 8601 duration between two RFC 3339 instants (`PT1H`, `P1DT1H30M`,
/// `PT0S`). Offsets are honoured, so instants in different zones measure the
/// real elapsed time. An end before the start, or a malformed instant, is
/// refused with a message naming it.
pub fn duration_between(start: &str, end: &str) -> std::result::Result<String, String> {
    let parse = |text: &str| {
        chrono::DateTime::parse_from_rfc3339(text)
            .map_err(|e| format!("not an RFC 3339 instant {text:?}: {e}"))
    };
    let seconds = (parse(end)? - parse(start)?).num_seconds();
    if seconds < 0 {
        return Err(format!("the end {end:?} is before the start {start:?}"));
    }
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (hours, rest) = (rest / 3_600, rest % 3_600);
    let (minutes, secs) = (rest / 60, rest % 60);
    let mut out = String::from("P");
    if days > 0 {
        out.push_str(&format!("{days}D"));
    }
    if hours > 0 || minutes > 0 || secs > 0 {
        out.push('T');
        if hours > 0 {
            out.push_str(&format!("{hours}H"));
        }
        if minutes > 0 {
            out.push_str(&format!("{minutes}M"));
        }
        if secs > 0 {
            out.push_str(&format!("{secs}S"));
        }
    }
    if out == "P" {
        out.push_str("T0S");
    }
    Ok(out)
}

/// The CalendarEvent/set arguments creating `draft` in `calendar`.
fn event_create_args(
    account_id: &str,
    uid: &str,
    calendar: &str,
    draft: &EventDraft,
) -> Result<Value> {
    require_uuid("calendar", calendar)?;
    Ok(json!({
        "accountId": account_id,
        "create": { CREATE: draft.to_jscalendar(uid, calendar)? },
    }))
}

/// The CalendarEvent/set arguments patching one event.
fn event_update_args(account_id: &str, id: &str, patch: &EventPatch) -> Result<Value> {
    Ok(json!({ "accountId": account_id, "update": { id: patch.to_patch()? } }))
}

/// The CalendarEvent/set arguments destroying one event.
fn event_destroy_args(account_id: &str, id: &str) -> Value {
    json!({ "accountId": account_id, "destroy": [id] })
}

/// The CalendarEvent/query and CalendarEvent/get pair behind every event
/// listing: one round trip, the get chained to the query's ids.
fn event_query_calls(account_id: &str, filter: Value) -> Vec<Call> {
    vec![
        Call(
            "CalendarEvent/query".into(),
            json!({
                "accountId": account_id,
                "filter": filter,
                "limit": 500,
            }),
            "q0".into(),
        ),
        Call(
            "CalendarEvent/get".into(),
            json!({
                "accountId": account_id,
                "#ids": crate::client::backref("q0", "CalendarEvent/query", "/ids"),
            }),
            "e0".into(),
        ),
    ]
}

/// One calendar. maild: `color` is a JSCalendar ColorString or null.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Calendar {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "isVisible", default)]
    pub is_visible: Option<bool>,
    #[serde(rename = "sortOrder", default)]
    pub sort_order: i32,
}

/// One calendar event: indexed columns plus the full JSCalendar object.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CalendarEvent {
    pub id: String,
    #[serde(rename = "calendarId")]
    pub calendar_id: String,
    #[serde(default)]
    pub uid: Option<String>,
    /// The full JSCalendar Event as submitted.
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub title: Option<String>,
    /// RFC 3339, when set.
    #[serde(default)]
    pub start: Option<String>,
    /// RFC 3339, when set.
    #[serde(default)]
    pub end: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
}

impl CalendarEvent {
    /// The indexed title, else the JSCalendar `title` out of `data`.
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .or_else(|| {
                self.data
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    }

    /// The start as a datetime, when it is set and parses.
    pub fn starts_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        parse_rfc3339(self.start.as_deref())
    }

    /// The end as a datetime, when it is set and parses.
    pub fn ends_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        parse_rfc3339(self.end.as_deref())
    }
}

fn parse_rfc3339(value: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    value
        .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

impl Client {
    /// Every calendar on the account.
    pub async fn calendars(&self) -> Result<Vec<Calendar>> {
        let replies = self
            .call(&[Call(
                "Calendar/get".into(),
                json!({ "accountId": self.account_id() }),
                "cal0".into(),
            )])
            .await?;
        let data = replies
            .into_iter()
            .next()
            .ok_or_else(|| Error::Protocol("Calendar/get reply is empty".into()))?;
        let list = data
            .get("list")
            .cloned()
            .ok_or_else(|| Error::Protocol("Calendar/get reply has no list".into()))?;
        Ok(serde_json::from_value(list)?)
    }

    /// One calendar's events, newest-start first (sorted client-side; the
    /// server's default order is unspecified). Filters on `calendarId`, the
    /// key this listing has always sent; [`Client::calendar_events_between`]
    /// is the windowed form.
    pub async fn calendar_events(&self, calendar_id: &str) -> Result<Vec<CalendarEvent>> {
        self.query_events(json!({ "calendarId": calendar_id }))
            .await
    }

    /// One calendar's events, newest-start first, filtered to an optional
    /// window: `after` and `before` are UTCDate values (RFC 3339, UTC).
    pub async fn calendar_events_between(
        &self,
        calendar_id: &str,
        after: Option<&str>,
        before: Option<&str>,
    ) -> Result<Vec<CalendarEvent>> {
        let mut filter = json!({ "calendarId": calendar_id });
        if let Some(after) = after {
            filter["after"] = json!(utc_instant("after", after)?);
        }
        if let Some(before) = before {
            filter["before"] = json!(utc_instant("before", before)?);
        }
        self.query_events(filter).await
    }

    /// One event by id, its full JSCalendar object included.
    pub async fn calendar_event(&self, id: &str) -> Result<CalendarEvent> {
        let replies = self
            .call(&[Call(
                "CalendarEvent/get".into(),
                json!({ "accountId": self.account_id(), "ids": [id] }),
                "e0".into(),
            )])
            .await?;
        let data = replies
            .into_iter()
            .next()
            .ok_or_else(|| Error::Protocol("CalendarEvent/get reply is empty".into()))?;
        let list = data
            .get("list")
            .cloned()
            .ok_or_else(|| Error::Protocol("CalendarEvent/get reply has no list".into()))?;
        let events: Vec<CalendarEvent> = serde_json::from_value(list)?;
        events
            .into_iter()
            .next()
            .ok_or_else(|| Error::Protocol(format!("event {id:?} is gone")))
    }

    /// Create an event in `calendar` and return its id. The uid is minted
    /// here: JSCalendar requires one, and a fresh event has none.
    pub async fn create_event(&self, calendar: &str, draft: &EventDraft) -> Result<String> {
        let uid = uuid::Uuid::now_v7().to_string();
        let args = event_create_args(self.account_id(), &uid, calendar, draft)?;
        let data = self.set_call("CalendarEvent/set", args).await?;
        check_set_reply(&data, "notCreated", &[CREATE.to_owned()])?;
        check_confirmed(&data, "created", &[CREATE.to_owned()])?;
        Client::created_id(&data, CREATE)
    }

    /// Patch one event. Only the set fields change.
    pub async fn update_event(&self, id: &str, patch: &EventPatch) -> Result<()> {
        let args = event_update_args(self.account_id(), id, patch)?;
        let data = self.set_call("CalendarEvent/set", args).await?;
        check_set_reply(&data, "notUpdated", &[id.to_owned()])?;
        check_confirmed(&data, "updated", &[id.to_owned()])
    }

    /// Delete one event.
    pub async fn delete_event(&self, id: &str) -> Result<()> {
        let args = event_destroy_args(self.account_id(), id);
        let data = self.set_call("CalendarEvent/set", args).await?;
        check_set_reply(&data, "notDestroyed", &[id.to_owned()])?;
        check_confirmed(&data, "destroyed", &[id.to_owned()])
    }

    /// The query-then-get listing behind every events call, sorted newest
    /// start first.
    async fn query_events(&self, filter: Value) -> Result<Vec<CalendarEvent>> {
        let replies = self
            .call(&event_query_calls(self.account_id(), filter))
            .await?;
        let data = replies
            .into_iter()
            .last()
            .ok_or_else(|| Error::Protocol("CalendarEvent/get reply is empty".into()))?;
        let list = data
            .get("list")
            .cloned()
            .ok_or_else(|| Error::Protocol("CalendarEvent/get reply has no list".into()))?;
        let mut events: Vec<CalendarEvent> = serde_json::from_value(list)?;
        events.sort_by_key(|event| std::cmp::Reverse(event.starts_at()));
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_maild_event_shape() {
        let event: CalendarEvent = serde_json::from_value(json!({
            "id": "V1",
            "calendarId": "C1",
            "uid": "uid-1",
            "data": { "@type": "Event", "title": "Dentist", "start": "2026-10-14T08:00:00Z" },
            "title": "Dentist",
            "start": "2026-10-14T08:00:00Z",
            "end": "2026-10-14T08:30:00Z",
            "updated": "2026-10-13T06:00:00Z"
        }))
        .unwrap();
        assert_eq!(event.display_title(), "Dentist");
        assert!(event.starts_at().is_some());
        assert!(event.ends_at().is_some());
    }

    #[test]
    fn the_title_falls_back_to_the_jscalendar_object() {
        let event: CalendarEvent = serde_json::from_value(json!({
            "id": "V2",
            "calendarId": "C1",
            "data": { "title": "From data" }
        }))
        .unwrap();
        assert_eq!(event.display_title(), "From data");
        assert!(event.starts_at().is_none());
    }

    fn draft() -> EventDraft {
        EventDraft {
            title: "Dentist".into(),
            start: "2026-10-14T08:00:00+10:00".into(),
            end: "2026-10-14T08:30:00+10:00".into(),
            ..EventDraft::default()
        }
    }

    #[test]
    fn a_draft_serialises_to_a_utc_event_with_a_singular_calendar_id() {
        let event = draft().to_jscalendar("uid-1", "C1").unwrap();
        assert_eq!(
            event,
            json!({
                "@type": "Event",
                "uid": "uid-1",
                "calendarId": "C1",
                "title": "Dentist",
                "start": "2026-10-13T22:00:00Z",
                "end": "2026-10-13T22:30:00Z",
                "duration": "PT30M",
                "showWithoutTime": false
            })
        );
    }

    #[test]
    fn a_local_or_malformed_instant_is_refused_before_any_request() {
        let local = EventDraft {
            start: "2026-10-14T08:00:00".into(),
            ..draft()
        };
        assert!(matches!(
            local.to_jscalendar("uid-1", "C1"),
            Err(Error::Protocol(_))
        ));
        let bad_calendar = event_create_args("A1", "uid-1", "not-a-uuid", &draft());
        assert!(matches!(bad_calendar, Err(Error::Protocol(_))));
        assert!(
            event_create_args("A1", "uid-1", "0196c0de-0000-7000-8000-000000000001", &draft())
                .is_ok()
        );
    }

    #[test]
    fn the_optional_fields_appear_only_when_set() {
        let full = EventDraft {
            time_zone: Some("Australia/Brisbane".into()),
            location: Some("Clinic".into()),
            description: Some("Bring card".into()),
            show_without_time: true,
            ..draft()
        };
        let event = full.to_jscalendar("uid-1", "C1").unwrap();
        assert_eq!(event["timeZone"], json!("Australia/Brisbane"));
        assert_eq!(
            event["locations"],
            json!({ "l1": { "@type": "Location", "name": "Clinic" } })
        );
        assert_eq!(event["description"], json!("Bring card"));
        assert_eq!(event["showWithoutTime"], json!(true));

        let blank = EventDraft {
            location: Some("  ".into()),
            ..draft()
        }
        .to_jscalendar("uid-1", "C1")
        .unwrap();
        assert!(blank.get("locations").is_none());
        assert!(blank.get("timeZone").is_none());
    }

    #[test]
    fn a_create_wraps_the_event_under_the_client_id() {
        let args = event_create_args(
            "A1",
            "uid-1",
            "0196c0de-0000-7000-8000-000000000001",
            &draft(),
        )
        .unwrap();
        assert_eq!(args["create"]["c0"]["title"], json!("Dentist"));
        assert_eq!(args["accountId"], json!("A1"));
    }

    #[test]
    fn a_patch_sends_only_the_set_fields_normalised_to_utc() {
        let patch = EventPatch {
            start: Some("2026-10-15T09:00:00+10:00".into()),
            end: Some("2026-10-15T10:00:00+10:00".into()),
            location: Some(String::new()),
            description: Some("Moved".into()),
            ..EventPatch::default()
        };
        assert_eq!(
            event_update_args("A1", "V1", &patch).unwrap(),
            json!({
                "accountId": "A1",
                "update": {
                    "V1": {
                        "start": "2026-10-14T23:00:00Z",
                        "end": "2026-10-15T00:00:00Z",
                        "locations": null,
                        "description": "Moved"
                    }
                }
            })
        );
        assert_eq!(
            event_destroy_args("A1", "V1"),
            json!({ "accountId": "A1", "destroy": ["V1"] })
        );
    }

    #[test]
    fn a_window_filter_names_the_calendar_and_both_bounds() {
        let mut filter = json!({ "calendarId": "C1" });
        filter["after"] = json!("2026-10-01T00:00:00Z");
        filter["before"] = json!("2026-11-01T00:00:00Z");
        let calls = event_query_calls("A1", filter);
        let body = crate::client::request_body(&[], &calls);
        assert_eq!(
            body["methodCalls"][0],
            json!([
                "CalendarEvent/query",
                {
                    "accountId": "A1",
                    "filter": {
                        "calendarId": "C1",
                        "after": "2026-10-01T00:00:00Z",
                        "before": "2026-11-01T00:00:00Z"
                    },
                    "limit": 500
                },
                "q0"
            ])
        );
        assert_eq!(body["methodCalls"][1][2], json!("e0"));
    }

    #[test]
    fn a_created_event_reads_back_through_the_existing_struct() {
        let data = draft().to_jscalendar("uid-1", "C1").unwrap();
        let event: CalendarEvent = serde_json::from_value(json!({
            "id": "V9",
            "calendarId": "C1",
            "uid": "uid-1",
            "data": data,
            "title": "Dentist"
        }))
        .unwrap();
        assert_eq!(event.display_title(), "Dentist");
        assert_eq!(event.data["calendarId"], json!("C1"));
    }

    #[test]
    fn durations_are_iso_8601_between_rfc_3339_instants() {
        let at = |end: &str| duration_between("2026-10-14T09:00:00+10:00", end);
        assert_eq!(at("2026-10-14T10:00:00+10:00").unwrap(), "PT1H");
        assert_eq!(at("2026-10-14T09:45:00+10:00").unwrap(), "PT45M");
        assert_eq!(at("2026-10-14T09:00:30+10:00").unwrap(), "PT30S");
        assert_eq!(at("2026-10-15T10:30:00+10:00").unwrap(), "P1DT1H30M");
        assert_eq!(at("2026-10-16T09:00:00+10:00").unwrap(), "P2D");
        assert_eq!(at("2026-10-14T09:00:00+10:00").unwrap(), "PT0S");
    }

    #[test]
    fn offsets_are_honoured_across_zones() {
        // 09:00 at +10:00 is 23:00 UTC the day before, so midnight UTC is an hour on.
        assert_eq!(
            duration_between("2026-10-14T09:00:00+10:00", "2026-10-14T00:00:00Z").unwrap(),
            "PT1H"
        );
        assert_eq!(
            duration_between("2026-10-14T09:00:00+10:00", "2026-10-13T23:00:00Z").unwrap(),
            "PT0S"
        );
    }

    #[test]
    fn a_reversed_or_malformed_span_is_refused() {
        assert!(
            duration_between("2026-10-14T10:00:00+10:00", "2026-10-14T09:00:00+10:00").is_err()
        );
        assert!(duration_between("2026-10-14T09:00:00", "2026-10-14T10:00:00+10:00").is_err());
        assert!(duration_between("not a time", "2026-10-14T10:00:00+10:00").is_err());
    }
}
