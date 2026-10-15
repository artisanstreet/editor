//! Paired Editors: pairing codes, device keys and revocation.

use crate::Store;
use anyhow::{Context, Result};
use artisan_api::{DeviceId, Timestamp, now};
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::sync::PoisonError;
use tokio::sync::broadcast;

/// A paired Editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub public_key: String,
    pub created_at: Timestamp,
    pub last_seen: Option<Timestamp>,
}

/// The paired keys, in memory so authorizing a request reads no table. The
/// set reloads only when SQLite's `data_version` says another connection
/// committed: this Forge's writer, or `ae forge devices revoke` in another
/// process, whose revocation must refuse the device's next request.
pub(crate) struct Devices {
    conn: Connection,
    data_version: i64,
    keys: HashMap<String, DeviceId>,
}

impl Devices {
    pub(crate) fn open(conn: Connection) -> Result<Self> {
        let mut devices = Self {
            conn,
            data_version: -1,
            keys: HashMap::new(),
        };
        devices.refresh()?;
        Ok(devices)
    }

    /// Reloads the keys if the database changed; returns devices that are gone.
    fn refresh(&mut self) -> Result<Vec<DeviceId>> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .context("read the database version")?;
        if version == self.data_version {
            return Ok(Vec::new());
        }
        let keys = self
            .conn
            .prepare_cached("SELECT public_key, id FROM authorized_keys")?
            .query_map([], |row| Ok((row.get(0)?, DeviceId(row.get(1)?))))?
            .collect::<rusqlite::Result<HashMap<String, DeviceId>>>()
            .context("load the paired device keys")?;
        let gone = self
            .keys
            .values()
            .filter(|id| !keys.values().any(|kept| kept == *id))
            .cloned()
            .collect();
        self.keys = keys;
        self.data_version = version;
        Ok(gone)
    }
}

impl Store {
    pub fn subscribe_revocations(&self) -> broadcast::Receiver<DeviceId> {
        self.inner.revocations.subscribe()
    }

    /// Consuming the invitation and authorizing its device are one decision.
    pub async fn pair_device(
        &self,
        code: String,
        name: String,
        certificate: String,
    ) -> Result<Option<DeviceId>> {
        self.write(move |tx| {
            if tx.execute(
                "DELETE FROM pair_codes WHERE code_sha256 = ?1 AND expires_at > ?2",
                params![code, now()],
            )? == 0
            {
                return Ok(None);
            }
            let id: String = tx.query_row(
                "INSERT INTO authorized_keys (id, name, public_key, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(public_key) DO UPDATE SET name = excluded.name RETURNING id",
                params![DeviceId::new().as_str(), name, certificate, now()],
                |row| row.get(0),
            )?;
            Ok(Some(DeviceId(id)))
        })
        .await
        .context("pair device")
    }

    /// Pairing the same key again renames the device and keeps its id.
    pub async fn add_device(&self, name: String, public_key: String) -> Result<DeviceId> {
        self.write(move |tx| {
            let id: String = tx.query_row(
                "INSERT INTO authorized_keys (id, name, public_key, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (public_key) DO UPDATE SET name = excluded.name
                 RETURNING id",
                params![DeviceId::new().as_str(), name, public_key, now()],
                |row| row.get(0),
            )?;
            Ok(DeviceId(id))
        })
        .await
        .context("add device")
    }

    /// Whether a device was revoked.
    pub async fn revoke_device(&self, id: &DeviceId) -> Result<bool> {
        let key = id.clone();
        let revoked = self
            .write(move |tx| {
                Ok(tx.execute("DELETE FROM authorized_keys WHERE id = ?1", [key.as_str()])? > 0)
            })
            .await?;
        if revoked {
            let _ = self.inner.revocations.send(id.clone());
        }
        Ok(revoked)
    }

    pub async fn touch_device(&self, id: &DeviceId) -> Result<()> {
        let id = id.clone();
        self.write(move |tx| {
            tx.execute(
                "UPDATE authorized_keys SET last_seen = ?2 WHERE id = ?1
                 AND (last_seen IS NULL OR last_seen < ?2 - 60000)",
                params![id.as_str(), now()],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn add_pair_code(&self, code_sha256: String, expires_at: Timestamp) -> Result<()> {
        self.write(move |tx| {
            tx.execute(
                "INSERT OR REPLACE INTO pair_codes (code_sha256, expires_at) VALUES (?1, ?2)",
                params![code_sha256, expires_at],
            )?;
            Ok(())
        })
        .await
    }

    /// The device a client certificate belongs to. Answered from memory;
    /// see [`Devices`].
    pub fn is_authorized(&self, public_key: &str) -> Result<Option<DeviceId>> {
        let mut devices = self
            .inner
            .devices
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for revoked in devices.refresh()? {
            let _ = self.inner.revocations.send(revoked);
        }
        Ok(devices.keys.get(public_key).cloned())
    }
}
