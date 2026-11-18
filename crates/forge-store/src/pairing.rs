use crate::Store;
use crate::error::{StorageError, operation_error};
use artisan_api::{DeviceId, Timestamp, now};
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::sync::PoisonError;
use tokio::sync::broadcast;

type Result<T, E = StorageError> = std::result::Result<T, E>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub public_key: String,
    pub created_at: Timestamp,
    pub last_seen: Option<Timestamp>,
}

pub struct Devices {
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

    fn refresh(&mut self) -> Result<Vec<DeviceId>> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA data_version", [], |row| row.get(0))?;
        if version == self.data_version {
            return Ok(Vec::new());
        }
        let keys = self
            .conn
            .prepare_cached("SELECT public_key, id FROM authorized_keys")?
            .query_map([], |row| Ok((row.get(0)?, DeviceId(row.get(1)?))))?
            .collect::<rusqlite::Result<HashMap<String, DeviceId>>>()?;
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

operation_error!(PairDeviceError(StorageError), "pair device {name}", name: String);
operation_error!(AddDeviceError(StorageError), "add device {name}", name: String);
operation_error!(RevokeDeviceError(StorageError), "revoke device {device}", device: DeviceId);
operation_error!(TouchDeviceError(StorageError), "record that device {device} was seen", device: DeviceId);
operation_error!(AddPairCodeError(StorageError), "save a pair code");
operation_error!(AuthorizeError(StorageError), "check a client certificate");

impl Store {
    pub fn subscribe_revocations(&self) -> broadcast::Receiver<DeviceId> {
        self.inner.revocations.subscribe()
    }

    pub async fn pair_device(
        &self,
        code: String,
        name: String,
        certificate: String,
    ) -> Result<Option<DeviceId>, PairDeviceError> {
        let device = name.clone();
        self.write_acknowledged(move |tx| {
            if tx.execute(
                "DELETE FROM pair_codes WHERE code_sha256 = ?1 AND expires_at > ?2",
                params![code, now()],
            )? == 0
            {
                return Ok(None);
            }
            let id: String = tx.query_row(
                include_str!("sql/pairing/authorize.sql"),
                params![DeviceId::new().as_str(), name, certificate, now()],
                |row| row.get(0),
            )?;
            Ok(Some(DeviceId(id)))
        })
        .await
        .map_err(|cause| PairDeviceError {
            name: device,
            cause: Box::new(cause),
        })
    }

    pub async fn add_device(
        &self,
        name: String,
        certificate: String,
    ) -> Result<DeviceId, AddDeviceError> {
        let device = name.clone();
        self.write_acknowledged(move |tx| {
            let id: String = tx.query_row(
                include_str!("sql/pairing/authorize.sql"),
                params![DeviceId::new().as_str(), name, certificate, now()],
                |row| row.get(0),
            )?;
            Ok(DeviceId(id))
        })
        .await
        .map_err(|cause| AddDeviceError {
            name: device,
            cause: Box::new(cause),
        })
    }

    pub async fn revoke_device(&self, id: &DeviceId) -> Result<bool, RevokeDeviceError> {
        let key = id.clone();
        let revoked = self
            .write_acknowledged(move |tx| {
                Ok(tx.execute("DELETE FROM authorized_keys WHERE id = ?1", [key.as_str()])? > 0)
            })
            .await
            .map_err(|cause| RevokeDeviceError {
                device: id.clone(),
                cause: Box::new(cause),
            })?;
        if revoked {
            self.inner.revocations.send(id.clone()).ok();
        }
        Ok(revoked)
    }

    pub async fn touch_device(&self, id: &DeviceId) -> Result<(), TouchDeviceError> {
        let device = id.clone();
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
        .map_err(|cause| TouchDeviceError {
            device,
            cause: Box::new(cause),
        })
    }

    pub async fn add_pair_code(
        &self,
        code: String,
        expires_at: Timestamp,
    ) -> Result<(), AddPairCodeError> {
        self.write_acknowledged(move |tx| add_pair_code(tx, &code, expires_at))
            .await
            .map_err(|cause| AddPairCodeError {
                cause: Box::new(cause),
            })
    }

    pub fn is_authorized(
        &self,
        certificate: &str,
    ) -> Result<Option<DeviceId>, AuthorizeError> {
        let mut devices = self
            .inner
            .devices
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for revoked in devices.refresh().map_err(|cause| AuthorizeError {
            cause: Box::new(cause),
        })? {
            self.inner.revocations.send(revoked).ok();
        }
        Ok(devices.keys.get(certificate).cloned())
    }
}

pub(crate) fn add_pair_code(
    conn: &Connection,
    code: &str,
    expires_at: Timestamp,
) -> Result<()> {
    conn.prepare_cached(include_str!("sql/pairing/expire_codes.sql"))?
        .execute([now()])?;
    conn.prepare_cached(include_str!("sql/pairing/add_code.sql"))?
        .execute(params![code, expires_at])?;
    Ok(())
}

pub(crate) fn pair_code_pending(conn: &Connection, code: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pair_codes WHERE code_sha256 = ?1)",
        [code],
        |row| row.get(0),
    )?)
}
