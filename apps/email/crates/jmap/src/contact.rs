// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Contacts, on maild's shape: `AddressBook` and `Contact` with the
//! indexed `fullName`/`email`/`company`/`phone` columns at the top level
//! and the full JSContact card nested under `data`. `phone` is derived by
//! the server at read time; `cosmix:rawVCard` inside `data` (a verbatim
//! vCard from a CardDAV write) is ignored.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{CREATE, Call, Client, check_set_reply, non_empty, optional_text};
use crate::error::{Error, Result};

/// What a new contact says, as a JSContact Card. Each non-blank email and
/// phone becomes a numbered entry (`e1`, `e2`, `p1`…), in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContactDraft {
    /// The full name, as `name.full`.
    pub name: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub organization: Option<String>,
    pub notes: Option<String>,
}

/// The changes to an existing contact. Only the set fields are patched. A
/// list replaces its whole map (`Some(vec![])` clears it); an empty
/// `organization` or `notes` clears it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContactPatch {
    pub name: Option<String>,
    pub emails: Option<Vec<String>>,
    pub phones: Option<Vec<String>>,
    pub organization: Option<String>,
    pub notes: Option<String>,
}

impl ContactDraft {
    /// The JSContact Card this draft creates in `address_book`, under `uid`.
    fn to_jscontact(&self, uid: &str, address_book: &str) -> Value {
        let mut card = json!({
            "@type": "Card",
            "uid": uid,
            "addressBookIds": { address_book: true },
            "name": { "full": self.name },
            "emails": numbered("e", &self.emails, |address| json!({ "address": address })),
            "phones": numbered("p", &self.phones, |number| json!({ "number": number })),
        });
        if let Some(organization) = non_empty(self.organization.as_deref()) {
            card["organizations"] = organization_map(organization);
        }
        if let Some(notes) = non_empty(self.notes.as_deref()) {
            card["notes"] = json!(notes);
        }
        card
    }
}

impl ContactPatch {
    /// The JSContact patch: each set field, an empty text field as `null`.
    fn to_patch(&self) -> Value {
        let mut patch = json!({});
        if let Some(name) = &self.name {
            patch["name"] = json!({ "full": name });
        }
        if let Some(emails) = &self.emails {
            patch["emails"] = numbered("e", emails, |address| json!({ "address": address }));
        }
        if let Some(phones) = &self.phones {
            patch["phones"] = numbered("p", phones, |number| json!({ "number": number }));
        }
        if let Some(organization) = &self.organization {
            patch["organizations"] = match non_empty(Some(organization)) {
                Some(organization) => organization_map(organization),
                None => Value::Null,
            };
        }
        if let Some(notes) = &self.notes {
            patch["notes"] = optional_text(notes);
        }
        patch
    }
}

/// A map of the non-blank values, keyed `prefix` plus a 1-based number.
fn numbered(prefix: &str, values: &[String], entry: impl Fn(&str) -> Value) -> Value {
    let entries: serde_json::Map<String, Value> = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .enumerate()
        .map(|(index, value)| (format!("{prefix}{}", index + 1), entry(value)))
        .collect();
    Value::Object(entries)
}

/// The one-entry `organizations` map JSContact gives a single name.
fn organization_map(name: &str) -> Value {
    json!({ "o1": { "name": name } })
}

/// The Contact/set arguments creating `draft` in `address_book`.
fn contact_create_args(
    account_id: &str,
    uid: &str,
    address_book: &str,
    draft: &ContactDraft,
) -> Value {
    json!({
        "accountId": account_id,
        "create": { CREATE: draft.to_jscontact(uid, address_book) },
    })
}

/// The Contact/set arguments patching one contact.
fn contact_update_args(account_id: &str, id: &str, patch: &ContactPatch) -> Value {
    json!({ "accountId": account_id, "update": { id: patch.to_patch() } })
}

/// The Contact/set arguments destroying one contact.
fn contact_destroy_args(account_id: &str, id: &str) -> Value {
    json!({ "accountId": account_id, "destroy": [id] })
}

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

    /// One contact by id, its full JSContact card included.
    pub async fn contact(&self, id: &str) -> Result<Contact> {
        let replies = self
            .call(&[Call(
                "Contact/get".into(),
                json!({ "accountId": self.account_id(), "ids": [id] }),
                "c0".into(),
            )])
            .await?;
        let data = replies
            .into_iter()
            .next()
            .ok_or_else(|| Error::Protocol("Contact/get reply is empty".into()))?;
        let list = data
            .get("list")
            .cloned()
            .ok_or_else(|| Error::Protocol("Contact/get reply has no list".into()))?;
        let contacts: Vec<Contact> = serde_json::from_value(list)?;
        contacts
            .into_iter()
            .next()
            .ok_or_else(|| Error::Protocol(format!("contact {id:?} is gone")))
    }

    /// Create a contact in `address_book` and return its id. The uid is
    /// minted here: JSContact requires one, and a fresh card has none.
    pub async fn create_contact(&self, address_book: &str, draft: &ContactDraft) -> Result<String> {
        let uid = uuid::Uuid::now_v7().to_string();
        let args = contact_create_args(self.account_id(), &uid, address_book, draft);
        let data = self.set_call("Contact/set", args).await?;
        check_set_reply(&data, "notCreated", &[CREATE.to_owned()])?;
        Client::created_id(&data, CREATE)
    }

    /// Patch one contact. Only the set fields change.
    pub async fn update_contact(&self, id: &str, patch: &ContactPatch) -> Result<()> {
        let args = contact_update_args(self.account_id(), id, patch);
        let data = self.set_call("Contact/set", args).await?;
        check_set_reply(&data, "notUpdated", &[id.to_owned()])
    }

    /// Delete one contact.
    pub async fn delete_contact(&self, id: &str) -> Result<()> {
        let args = contact_destroy_args(self.account_id(), id);
        let data = self.set_call("Contact/set", args).await?;
        check_set_reply(&data, "notDestroyed", &[id.to_owned()])
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

    fn draft() -> ContactDraft {
        ContactDraft {
            name: "Ann Example".into(),
            emails: vec![
                "ann@example.invalid".into(),
                "  ".into(),
                "a2@example.invalid".into(),
            ],
            phones: vec!["+61 7 5555 0000".into()],
            organization: Some("Example Pty Ltd".into()),
            notes: Some("Met at the fair".into()),
        }
    }

    #[test]
    fn a_draft_serialises_to_a_jscontact_card() {
        assert_eq!(
            draft().to_jscontact("uid-1", "B1"),
            json!({
                "@type": "Card",
                "uid": "uid-1",
                "addressBookIds": { "B1": true },
                "name": { "full": "Ann Example" },
                "emails": {
                    "e1": { "address": "ann@example.invalid" },
                    "e2": { "address": "a2@example.invalid" }
                },
                "phones": { "p1": { "number": "+61 7 5555 0000" } },
                "organizations": { "o1": { "name": "Example Pty Ltd" } },
                "notes": "Met at the fair"
            })
        );
    }

    #[test]
    fn a_blank_draft_has_empty_maps_and_no_optional_fields() {
        let card = ContactDraft {
            name: "Bob".into(),
            organization: Some(" ".into()),
            ..ContactDraft::default()
        }
        .to_jscontact("uid-2", "B1");
        assert_eq!(card["emails"], json!({}));
        assert_eq!(card["phones"], json!({}));
        assert!(card.get("organizations").is_none());
        assert!(card.get("notes").is_none());
    }

    #[test]
    fn a_create_wraps_the_card_under_the_client_id() {
        let args = contact_create_args("A1", "uid-1", "B1", &draft());
        assert_eq!(args["accountId"], json!("A1"));
        assert_eq!(
            args["create"]["c0"]["name"],
            json!({ "full": "Ann Example" })
        );
    }

    #[test]
    fn a_patch_replaces_whole_maps_and_clears_blank_text() {
        let patch = ContactPatch {
            emails: Some(Vec::new()),
            phones: Some(vec!["0400 000 000".into()]),
            organization: Some(String::new()),
            notes: Some("Moved".into()),
            ..ContactPatch::default()
        };
        assert_eq!(
            contact_update_args("A1", "P1", &patch),
            json!({
                "accountId": "A1",
                "update": {
                    "P1": {
                        "emails": {},
                        "phones": { "p1": { "number": "0400 000 000" } },
                        "organizations": null,
                        "notes": "Moved"
                    }
                }
            })
        );
    }

    #[test]
    fn a_created_card_reads_back_through_the_existing_struct() {
        let data = draft().to_jscontact("uid-1", "B1");
        let contact: Contact = serde_json::from_value(json!({
            "id": "P9",
            "addressBookId": "B1",
            "uid": "uid-1",
            "data": data,
            "fullName": "Ann Example",
            "email": "ann@example.invalid"
        }))
        .unwrap();
        assert_eq!(contact.display_name(), "Ann Example");
        assert_eq!(contact.display_email(), "ann@example.invalid");
        assert_eq!(contact.data["addressBookIds"]["B1"], json!(true));
    }
}
