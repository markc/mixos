// SPDX-License-Identifier: MIT OR Apache-2.0
//! Request-scoped creation references, including dependencies within one /set.
use super::{AppState, StateChange, references, types::JmapMethodError};
use anyhow::Result;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub(super) async fn call(
    state: &AppState,
    account: i32,
    method: &str,
    mut args: Value,
    ids: &mut HashMap<String, String>,
) -> Result<(Value, Option<(String, Value)>)> {
    if !method.ends_with("/set") {
        references::resolve_mail_creation_references(method, &mut args, ids);
        return Ok((invoke(state, account, method, args).await?, None));
    }
    validate_arguments(method, &args)?;
    let creates = args
        .get("create")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let updates = args
        .get("update")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let destroys = args
        .get("destroy")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut base = args.clone();
    for key in [
        "create",
        "update",
        "destroy",
        "onSuccessUpdateEmail",
        "onSuccessDestroyEmail",
    ] {
        base.as_object_mut()
            .ok_or_else(|| JmapMethodError {
                error_type: "invalidArguments".into(),
                description: Some("arguments must be an object".into()),
            })?
            .remove(key);
    }
    // Read the initial state before any mutation stage. Some handlers enforce
    // ifInState themselves; enforce it here too for every implemented /set.
    let mut response = invoke(state, account, method, base.clone()).await?;
    if let Some(expected) = args["ifInState"].as_str()
        && response["oldState"].as_str() != Some(expected)
    {
        return Err(JmapMethodError {
            error_type: "stateMismatch".into(),
            description: None,
        }
        .into());
    }
    // Later stages retain the initial oldState and supply the final newState.
    // This guard is a start-of-call precondition, not a transaction spanning
    // stages: another request may write between them.
    base.as_object_mut().unwrap().remove("ifInState");
    let mut pending = creates;
    let mut submission_emails = HashMap::new();
    while !pending.is_empty() {
        // Mailbox creates may depend on a sibling that sorts later in JSON.
        let ready = pending
            .iter()
            .find(|(_, entry)| {
                method != "Mailbox/set"
                    || entry
                        .get("parentId")
                        .and_then(Value::as_str)
                        .and_then(|s| s.strip_prefix('#'))
                        .is_none_or(|p| !pending.contains_key(p))
            })
            .map(|(key, _)| key.clone());
        let Some(key) = ready else {
            for key in pending.keys() {
                failure(
                    &mut response,
                    "notCreated",
                    key,
                    "invalidProperties",
                    Some("parentId"),
                );
            }
            break;
        };
        let mut entry = pending.remove(&key).unwrap();
        if let Err(property) = record_references(method, &mut entry, ids) {
            failure(
                &mut response,
                "notCreated",
                &key,
                "invalidProperties",
                Some(&property),
            );
            continue;
        }
        let mut call = base.clone();
        call["create"] = json!({&key: entry});
        if method == "EmailSubmission/set" {
            // Explicit client actions replace the legacy automatic Sent move.
            // Keep the arguments visible to submission's single-entry handler.
            for property in ["onSuccessUpdateEmail", "onSuccessDestroyEmail"] {
                if let Some(value) = args.get(property) {
                    call[property] = value.clone();
                }
            }
        }
        let result = invoke(state, account, method, call).await?;
        if let Some(id) = result["created"][&key]["id"].as_str() {
            ids.insert(key.clone(), id.to_owned());
            if method == "EmailSubmission/set" {
                submission_emails.insert(id.to_owned(), entry["emailId"].clone());
            }
        }
        merge(&mut response, result);
    }
    let mut resolved = Map::new();
    for (key, mut patch) in updates {
        let Some(id) = resolve_id(&key, ids) else {
            failure(&mut response, "notUpdated", &key, "notFound", None);
            continue;
        };
        if let Err(property) = record_references(method, &mut patch, ids) {
            failure(
                &mut response,
                "notUpdated",
                &id,
                "invalidProperties",
                Some(&property),
            );
            continue;
        }
        resolved.insert(id, patch);
    }
    if !resolved.is_empty() {
        let mut call = base.clone();
        call["update"] = resolved.into();
        merge(&mut response, invoke(state, account, method, call).await?);
    }
    let mut resolved = Vec::new();
    for key in destroys {
        let key = key.as_str().expect("validated destroy id");
        if let Some(id) = resolve_id(key, ids) {
            resolved.push(Value::String(id));
        } else {
            failure(&mut response, "notDestroyed", key, "notFound", None);
        }
    }
    if !resolved.is_empty() {
        let mut call = base;
        call["destroy"] = resolved.into();
        merge(&mut response, invoke(state, account, method, call).await?);
    }
    let extra = if method == "EmailSubmission/set" {
        // RFC 8621 §7.5: these keys name submissions, not emails. Only a
        // successful submission operation in this call authorises changes to
        // its email. A failed/unmatched action is ignored, including unresolved
        // creation references; implicit Email/set errors must name Email ids.
        let mut email_args = json!({"accountId": account.to_string()});
        let mut update = Map::new();
        let mut destroy = Vec::new();
        let mut errors = json!({});
        if let Some(entries) = args.get("onSuccessUpdateEmail").and_then(Value::as_object) {
            for (key, patch) in entries {
                if let Some(id) = resolve_id(key, ids)
                    && let Some(email) = submission_emails.get(&id).and_then(Value::as_str)
                {
                    let mut patch = patch.clone();
                    match record_references("Email/set", &mut patch, ids) {
                        Ok(()) => {
                            update.insert(email.to_owned(), patch);
                        }
                        Err(property) => failure(
                            &mut errors,
                            "notUpdated",
                            email,
                            "invalidProperties",
                            Some(&property),
                        ),
                    }
                }
            }
        }
        if let Some(entries) = args.get("onSuccessDestroyEmail").and_then(Value::as_array) {
            for key in entries.iter().filter_map(Value::as_str) {
                if let Some(id) = resolve_id(key, ids)
                    && let Some(email) = submission_emails.get(&id)
                {
                    destroy.push(email.clone());
                }
            }
        }
        if !update.is_empty()
            || !destroy.is_empty()
            || errors.as_object().is_some_and(|o| !o.is_empty())
        {
            email_args["update"] = update.into();
            email_args["destroy"] = destroy.into();
            match invoke(state, account, "Email/set", email_args).await {
                Ok(mut value) => {
                    merge(&mut value, errors);
                    if let Some(new_state) = value["newState"].as_str() {
                        let _ = state.state_tx.send(StateChange {
                            account_id: account,
                            object_type: "Email".into(),
                            new_state: new_state.into(),
                        });
                    }
                    Some(("Email/set".into(), value))
                }
                Err(error) => Some((
                    "error".into(),
                    json!({"type": "serverFail", "description": error.to_string()}),
                )),
            }
        } else {
            None
        }
    } else {
        None
    };
    Ok((response, extra))
}

/// RFC 8620 §§3.6.2/5.3: malformed method arguments fail the whole call,
/// before extracting stages could turn a wrongly typed argument into a no-op.
fn validate_arguments(method: &str, args: &Value) -> Result<()> {
    let invalid = |description: String| JmapMethodError {
        error_type: "invalidArguments".into(),
        description: Some(description),
    };
    let args = args
        .as_object()
        .ok_or_else(|| invalid("arguments must be an object".into()))?;
    for property in ["create", "update", "onSuccessUpdateEmail"] {
        if property == "onSuccessUpdateEmail" && method != "EmailSubmission/set" {
            continue;
        }
        if let Some(value) = args.get(property).filter(|value| !value.is_null()) {
            let valid = value
                .as_object()
                .is_some_and(|entries| entries.values().all(Value::is_object));
            if !valid {
                return Err(invalid(format!("{property} must be a map of objects or null")).into());
            }
        }
    }
    for property in ["destroy", "onSuccessDestroyEmail"] {
        if property == "onSuccessDestroyEmail" && method != "EmailSubmission/set" {
            continue;
        }
        if let Some(value) = args.get(property).filter(|value| !value.is_null()) {
            let valid = value
                .as_array()
                .is_some_and(|entries| entries.iter().all(Value::is_string));
            if !valid {
                return Err(invalid(format!("{property} must be an array of Id strings or null")).into());
            }
        }
    }
    if args
        .get("ifInState")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        return Err(invalid("ifInState must be a string or null".into()).into());
    }
    Ok(())
}

async fn invoke(state: &AppState, account: i32, method: &str, args: Value) -> Result<Value> {
    super::dispatch(
        &state.db,
        &state.mailstore,
        &state.domains_runtime,
        &state.aliases_runtime,
        account,
        method,
        args,
        &state.classifier,
    )
    .await
}

fn resolve_id(key: &str, ids: &HashMap<String, String>) -> Option<String> {
    match key.strip_prefix('#') {
        Some(key) => ids.get(key).cloned(),
        None => Some(key.to_owned()),
    }
}

/// Resolve only fields whose implemented schema declares an Id. In particular,
/// keyword names, subjects and arbitrary calendar/contact data remain literal.
fn record_references(
    method: &str,
    entry: &mut Value,
    ids: &HashMap<String, String>,
) -> std::result::Result<(), String> {
    let scalar = match method {
        "Mailbox/set" => &["parentId"][..],
        "Email/set" => &["blobId"][..],
        "EmailSubmission/set" => &["emailId", "identityId"][..],
        "CalendarEvent/set" => &["calendarId"][..],
        "Contact/set" => &["addressBookId"][..],
        _ => &[][..],
    };
    for property in scalar {
        if let Some(value) = entry.get_mut(property)
            && let Some(key) = value.as_str()
        {
            *value = resolve_id(key, ids)
                .ok_or_else(|| property.to_string())?
                .into();
        }
    }
    if method == "Email/set" {
        if let Some(mailboxes) = entry.get_mut("mailboxIds").and_then(Value::as_object_mut) {
            let original = std::mem::take(mailboxes);
            for (key, value) in original {
                let id = resolve_id(&key, ids).ok_or("mailboxIds")?;
                if mailboxes.insert(id, value).is_some() {
                    return Err("mailboxIds".into());
                }
            }
        }
        if let Some(patch) = entry.as_object_mut() {
            let keys = patch
                .keys()
                .filter(|k| k.starts_with("mailboxIds/#"))
                .cloned()
                .collect::<Vec<_>>();
            for key in keys {
                let id = resolve_id(&key[11..], ids).ok_or("mailboxIds")?;
                let value = patch.remove(&key).unwrap();
                if patch.insert(format!("mailboxIds/{id}"), value).is_some() {
                    return Err("mailboxIds".into());
                }
            }
        }
    }
    Ok(())
}

fn failure(response: &mut Value, bucket: &str, key: &str, kind: &str, property: Option<&str>) {
    if !response[bucket].is_object() {
        response[bucket] = json!({});
    }
    let mut error = json!({"type": kind});
    if let Some(property) = property {
        error["properties"] = json!([property]);
    }
    response[bucket][key] = error;
}

fn merge(response: &mut Value, next: Value) {
    for field in [
        "created",
        "updated",
        "notCreated",
        "notUpdated",
        "notDestroyed",
    ] {
        if let Some(map) = next[field].as_object() {
            if !response[field].is_object() {
                response[field] = json!({});
            }
            response[field].as_object_mut().unwrap().extend(map.clone());
        }
    }
    if let Some(array) = next["destroyed"].as_array() {
        if !response["destroyed"].is_array() {
            response["destroyed"] = json!([]);
        }
        response["destroyed"]
            .as_array_mut()
            .unwrap()
            .extend(array.clone());
    }
    if let Some(new) = next.get("newState") {
        response["newState"] = new.clone();
    }
}
