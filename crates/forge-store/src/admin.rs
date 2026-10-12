//! Offline administration of a Forge database, safe beside a running Forge.
//!
//! `ae forge pair` and `ae forge devices` use this instead of [`Store`]:
//! opening a `Store` recovers runs left live, which would cancel the running
//! Forge's work. Nothing here takes a seq while Forge may be running: pair
//! codes and devices carry none, and a setting is only ever added when
//! absent (Forge writes its identity on its first start, so a running Forge
//! already has it).
//!
//! [`Store`]: crate::Store

use crate::{BUSY_TIMEOUT, Device, read, schema};
use anyhow::{Context, Result};
use artisan_api::{DeviceId, Timestamp};
use rusqlite::{Connection, TransactionBehavior, params};
use std::path::Path;

/// A plain connection to `forge.sqlite3` in a data directory.
pub struct Admin {
    conn: Connection,
}

impl Admin {
    /// Opens (creating and migrating if absent) the database in `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let path = dir.join("forge.sqlite3");
        let mut conn =
            Connection::open(&path).with_context(|| format!("open database {}", path.display()))?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("enable WAL")?;
        schema::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn setting(&self, key: &str) -> Result<Option<serde_json::Value>> {
        read::setting(&self.conn, key).with_context(|| format!("read setting {key}"))
    }

    /// Stores `value` under `key` unless a value is there already, and
    /// returns whichever value is stored. The seq is taken in the same
    /// immediate transaction, after every other writer's.
    /// [`crate::Store::secret_or_insert`] without a running Forge.
    pub fn secret_or_insert(&mut self, key: &str, value: &str) -> Result<String> {
        self.conn.execute(
            "INSERT INTO secrets (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO NOTHING",
            rusqlite::params![key, value],
        )?;
        Ok(self
            .conn
            .query_row("SELECT value FROM secrets WHERE key = ?1", [key], |row| {
                row.get(0)
            })?)
    }

    pub fn setting_or_insert(
        &mut self,
        key: &str,
        value: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) =
            read::setting(&tx, key).with_context(|| format!("read setting {key}"))?
        {
            return Ok(existing);
        }
        let seq = read::max_seq(&tx)? + 1;
        tx.execute(
            "INSERT INTO kv (key, seq, value) VALUES (?1, ?2, ?3)",
            params![key, seq, read::to_body(&value)?],
        )
        .with_context(|| format!("save setting {key}"))?;
        tx.commit()
            .with_context(|| format!("commit setting {key}"))?;
        Ok(value)
    }

    pub fn add_pair_code(&self, code_sha256: &str, expires_at: Timestamp) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO pair_codes (code_sha256, expires_at) VALUES (?1, ?2)",
                params![code_sha256, expires_at],
            )
            .context("save pair code")?;
        Ok(())
    }

    pub fn devices(&self) -> Result<Vec<Device>> {
        let mut statement = self.conn.prepare(
            "SELECT id, name, public_key, created_at, last_seen FROM authorized_keys ORDER BY created_at",
        )?;
        let devices = statement
            .query_map([], |row| {
                Ok(Device {
                    id: DeviceId(row.get(0)?),
                    name: row.get(1)?,
                    public_key: row.get(2)?,
                    created_at: row.get(3)?,
                    last_seen: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()
            .context("list devices")?;
        Ok(devices)
    }

    /// Revokes the device whose id is `id` or starts with it (ids are long).
    /// Returns the revoked device, `None` when nothing matched.
    pub fn revoke_device(&self, id: &str) -> Result<Option<Device>> {
        let matches: Vec<Device> = self
            .devices()?
            .into_iter()
            .filter(|device| device.id.as_str().starts_with(id))
            .collect();
        let device = match matches.as_slice() {
            [] => return Ok(None),
            [device] => device.clone(),
            _ => anyhow::bail!(
                "{id:?} matches {} devices; give more of the id",
                matches.len()
            ),
        };
        self.conn
            .execute(
                "DELETE FROM authorized_keys WHERE id = ?1",
                [device.id.as_str()],
            )
            .with_context(|| format!("revoke device {}", device.id))?;
        Ok(Some(device))
    }
}
