// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded metadata reads through the native MDS per-account connection. Sieve
//! needs names, parents and rights, never per-mailbox message counts.
use crate::mailstore::{MailboxRole, SqliteMailStore, account_id_to_setid};
use anyhow::{Result, ensure};
use mds::{ContainerAttrs, ContainerId};
use rusqlite::{Connection, types::ValueRef};

#[derive(Debug, Clone)]
pub(super) struct Mailbox {
    pub id: ContainerId,
    pub parent: Option<ContainerId>,
    pub name: String,
    pub inbox: bool,
    pub writable: bool,
}

pub(super) fn load(ms: &SqliteMailStore, account: i32) -> Result<Vec<Mailbox>> {
    let set = account_id_to_setid(account);
    match ms.mds().with_set_read(&set, |conn| Ok(read_on(conn))) {
        Ok(result) => result,
        Err(mds::Error::SetNotFound(_)) => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

fn read_on(conn: &Connection) -> Result<Vec<Mailbox>> {
    // The PK supplies the ordering; the cursor fetches at most cap+1 rows.
    let mut statement =
        conn.prepare("SELECT id,parent_id,name,attrs FROM container ORDER BY id LIMIT ?1")?;
    let mut rows = statement.query([(super::MAX_STEPS + 1) as i64])?;
    let mut result = Vec::new();
    let mut bytes = 0;
    let mut seen = 0;
    while let Some(row) = rows.next()? {
        seen += 1;
        ensure!(seen <= super::MAX_STEPS, "mailbox metadata count limit");
        let id_value = row.get_ref(0)?;
        let parent_value = row.get_ref(1)?;
        let name_value = row.get_ref(2)?;
        let attrs_value = row.get_ref(3)?;
        let id = id_value.as_str()?;
        let parent = match parent_value {
            ValueRef::Null => None,
            _ => Some(parent_value.as_str()?),
        };
        let name = name_value.as_str()?;
        let attrs = attrs_value.as_str()?;
        bytes += id.len() + parent.map_or(0, str::len) + name.len() + attrs.len();
        // Check borrowed SQLite values before copying names or decoding JSON.
        ensure!(bytes <= 1024 * 1024, "mailbox metadata byte limit");
        if name == mds::container::UPLOAD_STAGING_NAME {
            continue;
        }
        let attrs: ContainerAttrs = serde_json::from_str(attrs)?;
        result.push(Mailbox {
            id: ContainerId(uuid::Uuid::parse_str(id)?),
            parent: parent
                .map(uuid::Uuid::parse_str)
                .transpose()?
                .map(ContainerId),
            name: name.to_string(),
            inbox: attrs
                .special_use
                .as_deref()
                .and_then(MailboxRole::from_special_use)
                == Some(MailboxRole::Inbox),
            writable: attrs
                .extra
                .get("jmap_my_rights")
                .and_then(|rights| rights.get("may_add_items"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    #[test]
    fn reads_are_bounded_without_status_tables_and_before_json_decoding() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE container(id TEXT PRIMARY KEY,parent_id TEXT,name TEXT,attrs TEXT);",
        )?;
        let attrs = serde_json::to_string(&ContainerAttrs {
            special_use: None,
            subscribed: false,
            extra: serde_json::json!({}),
        })?;
        conn.execute("WITH RECURSIVE n(x) AS (SELECT 0 UNION ALL SELECT x+1 FROM n WHERE x<=?1) INSERT INTO container SELECT printf('00000000-0000-0000-0000-%012x',x),NULL,'m'||x,?2 FROM n",params![super::super::MAX_STEPS as i64,attrs])?;
        assert!(
            read_on(&conn)
                .unwrap_err()
                .to_string()
                .contains("count limit")
        );
        conn.execute(
            "UPDATE container SET name=?1",
            [mds::container::UPLOAD_STAGING_NAME],
        )?;
        assert!(
            read_on(&conn)
                .unwrap_err()
                .to_string()
                .contains("count limit")
        );
        conn.execute("DELETE FROM container", [])?;
        conn.execute(
            "INSERT INTO container VALUES('00000000-0000-0000-0000-000000000001',NULL,'large',?1)",
            ["x".repeat(1024 * 1024 + 1)],
        )?;
        assert!(
            read_on(&conn)
                .unwrap_err()
                .to_string()
                .contains("byte limit")
        );
        conn.execute("DELETE FROM container", [])?;
        conn.execute(
            "INSERT INTO container VALUES('00000000-0000-0000-0000-000000000001',NULL,'Inbox',?1)",
            [serde_json::to_string(&ContainerAttrs {
                special_use: Some("\\Inbox".into()),
                subscribed: true,
                extra: serde_json::json!({}),
            })?],
        )?;
        let entries = read_on(&conn)?;
        assert_eq!(entries.len(), 1);
        assert!(entries[0].inbox && entries[0].writable);
        Ok(())
    }
}
