use crate::Store;
use crate::error::{StorageError, operation_error};
use artisan_api::{DeviceId, Timestamp, now};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{broadcast, oneshot};

type Result<T, E = StorageError> = std::result::Result<T, E>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairCodeHash([u8; 32]);

impl PairCodeHash {
    #[must_use]
    pub fn of(code: &str) -> Self {
        let normalised: String = code
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_uppercase())
            .collect();
        Self(Sha256::digest(normalised.as_bytes()).into())
    }

    fn hex(&self) -> String {
        hex::encode(self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeviceCertificate(String);

#[derive(Debug, thiserror::Error)]
pub enum CertificateError {
    #[error("the certificate is not base64")]
    NotBase64(#[source] base64::DecodeError),
    #[error("the certificate is empty")]
    Empty,
}

impl DeviceCertificate {
    #[must_use]
    pub fn from_der(der: &[u8]) -> Self {
        Self(STANDARD.encode(der))
    }

    pub fn parse(encoded: &str) -> Result<Self, CertificateError> {
        let der = STANDARD
            .decode(encoded.trim())
            .map_err(CertificateError::NotBase64)?;
        if der.is_empty() {
            return Err(CertificateError::Empty);
        }
        Ok(Self::from_der(&der))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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
    seen: Option<Seen>,
    keys: HashMap<String, DeviceId>,
}

#[derive(Clone, Copy)]
struct Seen {
    data_version: i64,
    revision: i64,
}

impl Devices {
    pub(crate) fn open(conn: Connection) -> Result<Self> {
        let mut devices = Self {
            conn,
            seen: None,
            keys: HashMap::new(),
        };
        devices.refresh()?;
        Ok(devices)
    }

    fn refresh(&mut self) -> Result<Vec<DeviceId>> {
        let data_version: i64 = self
            .conn
            .query_row("PRAGMA data_version", [], |row| row.get(0))?;
        if self
            .seen
            .is_some_and(|seen| seen.data_version == data_version)
        {
            return Ok(Vec::new());
        }
        let revision: i64 =
            self.conn
                .query_row(include_str!("sql/pairing/revision.sql"), [], |row| {
                    row.get(0)
                })?;
        if let Some(seen) = self.seen.as_mut().filter(|seen| seen.revision == revision) {
            seen.data_version = data_version;
            return Ok(Vec::new());
        }
        let keys = self
            .conn
            .prepare_cached(include_str!("sql/pairing/keys.sql"))?
            .query_map([], |row| Ok((row.get(0)?, DeviceId(row.get(1)?))))?
            .collect::<rusqlite::Result<HashMap<String, DeviceId>>>()?;
        let gone = self
            .keys
            .values()
            .filter(|id| !keys.values().any(|kept| kept == *id))
            .cloned()
            .collect();
        self.keys = keys;
        self.seen = Some(Seen {
            data_version,
            revision,
        });
        Ok(gone)
    }
}

pub(crate) struct Authorizer {
    queue: Arc<Mutex<Queue>>,
    revocations: broadcast::Sender<DeviceId>,
}

struct Check {
    certificate: DeviceCertificate,
    reply: oneshot::Sender<Result<Option<DeviceId>>>,
}

enum Queue {
    Idle(Devices),
    Checking(Vec<Check>),
}

impl Authorizer {
    pub(crate) fn new(devices: Devices, revocations: broadcast::Sender<DeviceId>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(Queue::Idle(devices))),
            revocations,
        }
    }

    fn check(&self, check: Check) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        match std::mem::replace(&mut *queue, Queue::Checking(Vec::new())) {
            Queue::Checking(mut pending) => {
                pending.push(check);
                *queue = Queue::Checking(pending);
            }
            Queue::Idle(devices) => {
                *queue = Queue::Checking(vec![check]);
                let (queue, revocations) = (Arc::clone(&self.queue), self.revocations.clone());
                drop(tokio::task::spawn_blocking(move || {
                    answer_checks(devices, &queue, &revocations);
                }));
            }
        }
    }
}

fn answer_checks(
    mut devices: Devices,
    queue: &Mutex<Queue>,
    revocations: &broadcast::Sender<DeviceId>,
) {
    loop {
        let batch = {
            let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            match &mut *queue {
                Queue::Checking(pending) if pending.is_empty() => {
                    *queue = Queue::Idle(devices);
                    return;
                }
                Queue::Checking(pending) => std::mem::take(pending),
                Queue::Idle(_) => {
                    unreachable!("invariant: only the authorization worker idles its queue")
                }
            }
        };
        let mut fresh = false;
        for check in batch {
            let answer = if fresh {
                Ok(())
            } else {
                devices.refresh().map(|gone| {
                    for revoked in gone {
                        revocations.send(revoked).ok();
                    }
                })
            };
            fresh = answer.is_ok();
            check
                .reply
                .send(answer.map(|()| devices.keys.get(check.certificate.as_str()).cloned()))
                .ok();
        }
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
        code: PairCodeHash,
        name: String,
        certificate: DeviceCertificate,
    ) -> Result<Option<DeviceId>, PairDeviceError> {
        let device = name.clone();
        self.write_acknowledged(move |tx| {
            if tx.execute(
                "DELETE FROM pair_codes WHERE code_sha256 = ?1 AND expires_at > ?2",
                params![code.hex(), now()],
            )? == 0
            {
                return Ok(None);
            }
            let id: String = tx.query_row(
                include_str!("sql/pairing/authorize.sql"),
                params![DeviceId::new().as_str(), name, certificate.as_str(), now()],
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
        certificate: DeviceCertificate,
    ) -> Result<DeviceId, AddDeviceError> {
        let device = name.clone();
        self.write_acknowledged(move |tx| {
            let id: String = tx.query_row(
                include_str!("sql/pairing/authorize.sql"),
                params![DeviceId::new().as_str(), name, certificate.as_str(), now()],
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
        code: PairCodeHash,
        expires_at: Timestamp,
    ) -> Result<(), AddPairCodeError> {
        self.write_acknowledged(move |tx| add_pair_code(tx, &code, expires_at))
            .await
            .map_err(|cause| AddPairCodeError {
                cause: Box::new(cause),
            })
    }

    pub async fn is_authorized(
        &self,
        certificate: &DeviceCertificate,
    ) -> Result<Option<DeviceId>, AuthorizeError> {
        let failed = |cause| AuthorizeError {
            cause: Box::new(cause),
        };
        let (reply, answer) = oneshot::channel();
        self.inner.authorizer.check(Check {
            certificate: certificate.clone(),
            reply,
        });
        answer
            .await
            .map_err(|dropped| failed(StorageError::AuthorizationDropped(dropped)))?
            .map_err(failed)
    }
}

pub(crate) fn add_pair_code(
    conn: &Connection,
    code: &PairCodeHash,
    expires_at: Timestamp,
) -> Result<()> {
    conn.prepare_cached(include_str!("sql/pairing/expire_codes.sql"))?
        .execute([now()])?;
    conn.prepare_cached(include_str!("sql/pairing/add_code.sql"))?
        .execute(params![code.hex(), expires_at])?;
    Ok(())
}

pub(crate) fn pair_code_pending(conn: &Connection, code: &PairCodeHash) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pair_codes WHERE code_sha256 = ?1)",
        [code.hex()],
        |row| row.get(0),
    )?)
}
