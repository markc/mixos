// SPDX-License-Identifier: MIT OR Apache-2.0
//! Operator/Email Bus editors share the ManageSieve store and compiler.
use crate::{
    db::{self, Db},
    sieve::store,
};
use ::bus::IncomingCommand;
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub account: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub source: String,
    pub expected_version: Option<i64>,
}

pub async fn dispatch(cmd: &IncomingCommand, db: &Db) -> (u8, String) {
    match execute(cmd, db).await {
        Ok(value) => (0, value.to_string()),
        Err(error) => (10, json!({"error":error.to_string()}).to_string()),
    }
}
async fn execute(cmd: &IncomingCommand, db: &Db) -> Result<serde_json::Value> {
    ensure!(
        cmd.body.len() <= super::super::sieve::MAX_SCRIPT_BYTES * 6 + 8192
            && cmd
                .header("args")
                .is_none_or(|a| a.len() <= super::super::sieve::MAX_SCRIPT_BYTES * 6 + 8192),
        "Sieve request limit"
    );
    let request: Request =
        serde_json::from_value(super::try_resolve_args(cmd).map_err(anyhow::Error::msg)?)?;
    let account = db::account::get_by_email(&db.conn, &request.account.to_ascii_lowercase())
        .await?
        .ok_or_else(|| anyhow::anyhow!("account not found"))?;
    Ok(match cmd.command.as_str() {
        "maild.sieve.list" => json!({"scripts":store::list(db,account.id).await?}),
        "maild.sieve.get" => {
            serde_json::to_value(store::get(db, account.id, &request.name).await?)?
        }
        "maild.sieve.put" => {
            json!({"version":store::put(db,account.id,&request.name,&request.source,request.expected_version).await?})
        }
        "maild.sieve.activate" => {
            store::activate(db, account.id, &request.name).await?;
            json!({"active":request.name})
        }
        _ => anyhow::bail!("unknown Sieve command"),
    })
}
