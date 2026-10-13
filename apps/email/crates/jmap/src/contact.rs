// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Contacts, on maild's shape: `AddressBook` and `Contact` with the
//! indexed `fullName`/`email`/`company`/`phone` columns at the top level
//! and the full JSContact card nested under `data`. `phone` is derived by
//! the server at read time; `cosmix:rawVCard` inside `data` (a verbatim
//! vCard from a CardDAV write) is ignored.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Call, Client};
use crate::error::{Error, Result};

/// One address book.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AddressBook {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "sortOrder", default)]
    pub sort_order: i32,
}

/// One contact: indexed columns plus the full JSContact card.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Contact {
    pub id: String,
    #[serde(rename = "addressBookId")]
    pub address_book_id: String,
    #[serde(default)]
    pub uid: Option<String>,
    /// The full JSContact Card as submitted.
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
}

impl Contact {
    /// The indexed full name, else the JSContact `name.full` out of `data`.
    pub fn display_name(&self) -> String {
        self.full_name
            .clone()
            .or_else(|| {
                self.data
                    .pointer("/name/full")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    }

    /// The indexed email, else the first `emails` address out of `data`.
    pub fn display_email(&self) -> String {
        self.email
            .clone()
            .or_else(|| {
                self.data
                    .get("emails")
                    .and_then(Value::as_object)
                    .and_then(|emails| emails.values().next())
                    .and_then(|entry| entry.get("address"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    }
}

impl Client {
    /// Every address book on the account.
    pub async fn address_books(&self) -> Result<Vec<AddressBook>> {
        let replies = self
            .call(&[Call(
                "AddressBook/get".into(),
                json!({ "accountId": self.account_id() }),
                "ab0".into(),
            )])
            .await?;
        let data = replies
            .into_iter()
            .next()
            .ok_or_else(|| Error::Protocol("AddressBook/get reply is empty".into()))?;
        let list = data
            .get("list")
            .cloned()
            .ok_or_else(|| Error::Protocol("AddressBook/get reply has no list".into()))?;
        Ok(serde_json::from_value(list)?)
    }

    /// One address book's contacts, full-name first, with an optional text
    /// filter over name, email and company. One round trip: `Contact/query`
    /// chains into `Contact/get`.
    pub async fn contacts(&self, address_book_id: &str, text: &str) -> Result<Vec<Contact>> {
        let mut filter = json!({ "addressBookId": address_book_id });
        if !text.trim().is_empty() {
            filter["text"] = json!(text.trim());
        }
        let replies = self
            .call(&[
                Call(
                    "Contact/query".into(),
                    json!({
                        "accountId": self.account_id(),
                        "filter": filter,
                        "sort": [{ "property": "fullName", "isAscending": true }],
                        "limit": 500,
                    }),
                    "q0".into(),
                ),
                Call(
                    "Contact/get".into(),
                    json!({
                        "accountId": self.account_id(),
                        "#ids": crate::client::backref("q0", "Contact/query", "/ids"),
                    }),
                    "c0".into(),
                ),
            ])
            .await?;
        let data = replies
            .into_iter()
            .last()
            .ok_or_else(|| Error::Protocol("Contact/get reply is empty".into()))?;
        let list = data
            .get("list")
            .cloned()
            .ok_or_else(|| Error::Protocol("Contact/get reply has no list".into()))?;
        Ok(serde_json::from_value(list)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_maild_contact_shape() {
        let contact: Contact = serde_json::from_value(json!({
            "id": "P1",
            "addressBookId": "B1",
            "uid": "uid-1",
            "data": {
                "@type": "Card",
                "name": { "full": "Ann Example" },
                "emails": { "e1": { "address": "ann@example.invalid" } }
            },
            "fullName": "Ann Example",
            "email": "ann@example.invalid",
            "company": "Example Pty Ltd",
            "phone": "+61 7 5555 0000",
            "updated": "2026-10-13T06:00:00Z"
        }))
        .unwrap();
        assert_eq!(contact.display_name(), "Ann Example");
        assert_eq!(contact.display_email(), "ann@example.invalid");
        assert_eq!(contact.company.as_deref(), Some("Example Pty Ltd"));
    }

    #[test]
    fn the_display_fields_fall_back_to_the_jscontact_card() {
        let contact: Contact = serde_json::from_value(json!({
            "id": "P2",
            "addressBookId": "B1",
            "data": {
                "name": { "full": "Bob Sample" },
                "emails": { "e1": { "address": "bob@example.invalid" } }
            }
        }))
        .unwrap();
        assert_eq!(contact.display_name(), "Bob Sample");
        assert_eq!(contact.display_email(), "bob@example.invalid");
    }
}
