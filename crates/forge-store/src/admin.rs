use crate::error::StorageError;
use crate::{BUSY_TIMEOUT, Device, PairCodeHash, schema};
use artisan_api::{DeviceId, Timestamp};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

const DATABASE: &str = "forge.sqlite3";

#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("there is no Forge database in {}; start Forge once to create it", dir.display())]
    Missing { dir: PathBuf },
    #[error("create {}", dir.display())]
    Create {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("open {}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("{} is at schema {found} but this ae works with schema {expected}; update or restart Forge so they match", path.display())]
    Schema {
        path: PathBuf,
        found: usize,
        expected: usize,
    },
    #[error("upgrade {} to schema {}", path.display(), schema::version())]
    Migrate {
        path: PathBuf,
        #[source]
        source: schema::SchemaError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RevokeError {
    #[error("{id:?} matches {matches} devices; give more of the id")]
    Ambiguous { id: String, matches: usize },
    #[error("revoke device {id:?}")]
    Storage {
        id: String,
        #[source]
        source: StorageError,
    },
    #[error("tell the running Forge that device {id:?} was revoked through {}", path.display())]
    Announce {
        id: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

crate::error::operation_error!(ReadSecretError(StorageError), "read secret {key}", key: String);
crate::error::operation_error!(SaveSecretError(StorageError), "save secret {key}", key: String);
crate::error::operation_error!(PairCodeError(StorageError), "look up or save a pair code");
crate::error::operation_error!(DevicesError(StorageError), "list the paired devices");

#[derive(Debug)]
pub struct Admin {
    conn: Connection,
    database: PathBuf,
}

impl Admin {
    pub fn open(dir: &Path) -> Result<Self, AdminError> {
        let path = existing(dir)?;
        let conn = connect(&path, OpenFlags::default())?;
        require_current(&path, &conn)?;
        Ok(Self {
            conn,
            database: path,
        })
    }

    pub fn open_read_only(dir: &Path) -> Result<Self, AdminError> {
        let path = existing(dir)?;
        let conn = connect(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        require_current(&path, &conn)?;
        Ok(Self {
            conn,
            database: path,
        })
    }

    pub fn open_exclusive(dir: &Path, _data_lock: &std::fs::File) -> Result<Self, AdminError> {
        std::fs::create_dir_all(dir).map_err(|source| AdminError::Create {
            dir: dir.to_owned(),
            source,
        })?;
        let path = dir.join(DATABASE);
        let mut conn = connect(&path, OpenFlags::default())?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|source| AdminError::Open {
                path: path.clone(),
                source: Box::new(source),
            })?;
        schema::migrate(&mut conn).map_err(|source| AdminError::Migrate {
            path: path.clone(),
            source,
        })?;
        Ok(Self {
            conn,
            database: path,
        })
    }

    pub fn secret(&self, key: &str) -> Result<Option<String>, ReadSecretError> {
        crate::secrets::secret(&self.conn, key).map_err(|source| ReadSecretError {
            key: key.to_owned(),
            cause: Box::new(source.into()),
        })
    }

    pub fn secret_or_insert(&self, key: &str, value: &str) -> Result<String, SaveSecretError> {
        crate::secrets::secret_or_insert(&self.conn, key, value).map_err(|source| SaveSecretError {
            key: key.to_owned(),
            cause: Box::new(source.into()),
        })
    }

    pub fn pair_code_pending(&self, code: &PairCodeHash) -> Result<bool, PairCodeError> {
        crate::pairing::pair_code_pending(&self.conn, code).map_err(|cause| PairCodeError {
            cause: Box::new(cause),
        })
    }

    pub fn add_pair_code(
        &self,
        code: &PairCodeHash,
        expires_at: Timestamp,
    ) -> Result<(), PairCodeError> {
        let saved = || -> Result<(), StorageError> {
            let tx = self.conn.unchecked_transaction()?;
            crate::pairing::add_pair_code(&tx, code, expires_at)?;
            tx.commit()?;
            Ok(())
        };
        saved().map_err(|cause| PairCodeError {
            cause: Box::new(cause),
        })
    }

    pub fn devices(&self) -> Result<Vec<Device>, DevicesError> {
        let listed = || -> Result<Vec<Device>, rusqlite::Error> {
            let mut statement = self.conn.prepare(include_str!("sql/pairing/devices.sql"))?;
            statement
                .query_map([], |row| {
                    Ok(Device {
                        id: DeviceId(row.get(0)?),
                        name: row.get(1)?,
                        public_key: row.get(2)?,
                        created_at: row.get(3)?,
                        last_seen: row.get(4)?,
                    })
                })?
                .collect()
        };
        listed().map_err(|source| DevicesError {
            cause: Box::new(source.into()),
        })
    }

    pub fn revoke_device(&self, id: &str) -> Result<Option<Device>, RevokeError> {
        let storage = |source: StorageError| RevokeError::Storage {
            id: id.to_owned(),
            source,
        };
        let matches: Vec<Device> = self
            .devices()
            .map_err(|error| storage(*error.cause))?
            .into_iter()
            .filter(|device| device.id.as_str().starts_with(id))
            .collect();
        let device = match matches.as_slice() {
            [] => return Ok(None),
            [device] => device.clone(),
            _ => {
                return Err(RevokeError::Ambiguous {
                    id: id.to_owned(),
                    matches: matches.len(),
                });
            }
        };
        self.conn
            .execute(include_str!("sql/pairing/revoke.sql"), [device.id.as_str()])
            .map_err(|source| storage(source.into()))?;
        crate::pairing::announce_device_change(&self.database).map_err(|source| {
            RevokeError::Announce {
                id: id.to_owned(),
                path: crate::pairing::device_signal(&self.database),
                source,
            }
        })?;
        Ok(Some(device))
    }
}

pub fn schema_version(dir: &Path) -> Result<Option<usize>, AdminError> {
    let path = dir.join(DATABASE);
    if !path.is_file() {
        return Ok(None);
    }
    let conn = connect(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    version(&path, &conn).map(Some)
}

fn existing(dir: &Path) -> Result<PathBuf, AdminError> {
    let path = dir.join(DATABASE);
    if path.is_file() {
        Ok(path)
    } else {
        Err(AdminError::Missing {
            dir: dir.to_owned(),
        })
    }
}

fn connect(path: &Path, flags: OpenFlags) -> Result<Connection, AdminError> {
    let open = |source| AdminError::Open {
        path: path.to_owned(),
        source: Box::new(source),
    };
    let conn = Connection::open_with_flags(path, flags).map_err(open)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(open)?;
    Ok(conn)
}

fn version(path: &Path, conn: &Connection) -> Result<usize, AdminError> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|source| AdminError::Open {
            path: path.to_owned(),
            source: Box::new(source),
        })
}

fn require_current(path: &Path, conn: &Connection) -> Result<(), AdminError> {
    let found = version(path, conn)?;
    if found == schema::version() {
        Ok(())
    } else {
        Err(AdminError::Schema {
            path: path.to_owned(),
            found,
            expected: schema::version(),
        })
    }
}
