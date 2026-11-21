use crate::Store;
use crate::error::AtStep as _;
use crate::error::{StorageError, operation_error};
use artisan_api::{DeviceId, Timestamp};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rusqlite::{Connection, OptionalExtension as _, params};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{broadcast, oneshot, watch};

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
    fn as_str(&self) -> &str {
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

pub(crate) struct Devices {
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
            .query_row(include_str!("sql/pairing/data_version.sql"), [], |row| {
                row.get(0)
            })
            .at("pairing/data_version.sql")?;
        if self
            .seen
            .is_some_and(|seen| seen.data_version == data_version)
        {
            return Ok(Vec::new());
        }
        let revision: i64 = self
            .conn
            .query_row(include_str!("sql/pairing/revision.sql"), [], |row| {
                row.get(0)
            })
            .at("pairing/revision.sql")?;
        if let Some(seen) = self.seen.as_mut().filter(|seen| seen.revision == revision) {
            seen.data_version = data_version;
            return Ok(Vec::new());
        }
        let keys = self
            .conn
            .prepare_cached(include_str!("sql/pairing/keys.sql"))
            .at("pairing/keys.sql")?
            .query_map([], |row| Ok((row.get(0)?, DeviceId(row.get(1)?))))
            .at("pairing/keys.sql")?
            .collect::<rusqlite::Result<HashMap<String, DeviceId>>>()
            .at("pairing/keys.sql")?;
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

pub(crate) fn device_signal(database: &Path) -> PathBuf {
    database.with_file_name("devices.changed")
}

pub(crate) fn announce_device_change(database: &Path, now: Timestamp) -> std::io::Result<()> {
    std::fs::write(device_signal(database), now.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum WatchDevicesError {
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("watch {}", path.display())]
    Watch {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
}

pub(crate) struct DeviceWatch {
    _watcher: notify::RecommendedWatcher,
    changes: watch::Sender<()>,
}

impl DeviceWatch {
    pub(crate) fn start(database: &Path) -> Result<Self, WatchDevicesError> {
        use notify::Watcher as _;
        let path = device_signal(database);
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| WatchDevicesError::Create {
                path: path.clone(),
                source,
            })?;
        let changes = watch::Sender::new(());
        let notify = changes.clone();
        let watching = |source| WatchDevicesError::Watch {
            path: path.clone(),
            source,
        };
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event) => {
                    if event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove() {
                        notify.send_replace(());
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "watching for device changes from other processes");
                    notify.send_replace(());
                }
            })
            .map_err(watching)?;
        watcher
            .watch(&path, notify::RecursiveMode::NonRecursive)
            .map_err(watching)?;
        Ok(Self {
            _watcher: watcher,
            changes,
        })
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
                        let _no_subscribers = revocations.send(revoked);
                    }
                })
            };
            fresh = answer.is_ok();
            let _requester_gone = check
                .reply
                .send(answer.map(|()| devices.keys.get(check.certificate.as_str()).cloned()));
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

    pub fn subscribe_device_changes(&self) -> watch::Receiver<()> {
        self.inner.device_watch.changes.subscribe()
    }

    pub async fn pair_device(
        &self,
        code: PairCodeHash,
        name: String,
        certificate: DeviceCertificate,
    ) -> Result<Option<DeviceId>, PairDeviceError> {
        let device = name.clone();
        self.write_acknowledged(move |tx| {
            let redeemable: bool = tx
                .query_row(
                    include_str!("sql/pairing/redeemable.sql"),
                    params![code.hex(), tx.now()],
                    |row| row.get(0),
                )
                .at("pairing/redeemable.sql")?;
            if !redeemable {
                return Ok(None);
            }
            let id: String = tx
                .query_row(
                    include_str!("sql/pairing/authorize.sql"),
                    params![
                        DeviceId::new().as_str(),
                        name,
                        certificate.as_str(),
                        tx.now()
                    ],
                    |row| row.get(0),
                )
                .at("pairing/authorize.sql")?;
            tx.execute(
                include_str!("sql/pairing/redeem_code.sql"),
                params![code.hex(), id],
            )
            .at("pairing/redeem_code.sql")?;
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
            let id: String = tx
                .query_row(
                    include_str!("sql/pairing/authorize.sql"),
                    params![
                        DeviceId::new().as_str(),
                        name,
                        certificate.as_str(),
                        tx.now()
                    ],
                    |row| row.get(0),
                )
                .at("pairing/authorize.sql")?;
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
                Ok(tx
                    .execute(include_str!("sql/pairing/revoke.sql"), [key.as_str()])
                    .at("pairing/revoke.sql")?
                    > 0)
            })
            .await
            .map_err(|cause| RevokeDeviceError {
                device: id.clone(),
                cause: Box::new(cause),
            })?;
        if revoked {
            let _no_subscribers = self.inner.revocations.send(id.clone());
            let database = self.inner.database.clone();
            let now = self.now();
            tokio::task::spawn_blocking(move || announce_device_change(&database, now))
                .await
                .map_err(|source| RevokeDeviceError {
                    device: id.clone(),
                    cause: Box::new(StorageError::Task(source)),
                })?
                .map_err(|source| RevokeDeviceError {
                    device: id.clone(),
                    cause: Box::new(StorageError::Announce(source)),
                })?;
        }
        Ok(revoked)
    }

    pub async fn touch_device(&self, id: &DeviceId) -> Result<(), TouchDeviceError> {
        let device = id.clone();
        let id = id.clone();
        self.write(move |tx| {
            tx.execute(
                include_str!("sql/pairing/touch.sql"),
                params![id.as_str(), tx.now()],
            )
            .at("pairing/touch.sql")?;
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
        self.write_acknowledged(move |tx| add_pair_code(tx, &code, expires_at, tx.now()))
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
    now: Timestamp,
) -> Result<()> {
    conn.prepare_cached(include_str!("sql/pairing/expire_codes.sql"))
        .at("pairing/expire_codes.sql")?
        .execute([now])
        .at("pairing/expire_codes.sql")?;
    conn.prepare_cached(include_str!("sql/pairing/add_code.sql"))
        .at("pairing/add_code.sql")?
        .execute(params![code.hex(), expires_at])
        .at("pairing/add_code.sql")?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairCodeState {
    Waiting,
    Paired(Device),
    PairedThenRemoved,
    Gone,
}

pub(crate) fn pair_code_state(conn: &Connection, code: &PairCodeHash) -> Result<PairCodeState> {
    conn.query_row(
        include_str!("sql/pairing/code_state.sql"),
        [code.hex()],
        |row| {
            let redeemed: Option<String> = row.get(0)?;
            let device: Option<String> = row.get(1)?;
            Ok(match (redeemed, device) {
                (None, _) => PairCodeState::Waiting,
                (Some(_), None) => PairCodeState::PairedThenRemoved,
                (Some(_), Some(id)) => PairCodeState::Paired(Device {
                    id: DeviceId(id),
                    name: row.get(2)?,
                    public_key: row.get(3)?,
                    created_at: row.get(4)?,
                    last_seen: row.get(5)?,
                }),
            })
        },
    )
    .optional()
    .map(|state| state.unwrap_or(PairCodeState::Gone))
    .at("pairing/code_state.sql")
}
