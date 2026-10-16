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

use crate::client::{Call, Client};
use crate::error::{Error, Result};

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
    /// server's default order is unspecified).
    pub async fn calendar_events(&self, calendar_id: &str) -> Result<Vec<CalendarEvent>> {
        let replies = self
            .call(&[
                Call(
                    "CalendarEvent/query".into(),
                    json!({
                        "accountId": self.account_id(),
                        "filter": { "calendarId": calendar_id },
                        "limit": 500,
                    }),
                    "q0".into(),
                ),
                Call(
                    "CalendarEvent/get".into(),
                    json!({
                        "accountId": self.account_id(),
                        "#ids": crate::client::backref("q0", "CalendarEvent/query", "/ids"),
                    }),
                    "e0".into(),
                ),
            ])
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
}
