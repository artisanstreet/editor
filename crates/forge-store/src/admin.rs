use crate::{BUSY_TIMEOUT, Device, schema};
use anyhow::{Context, Result};
use artisan_api::{DeviceId, Timestamp};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
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
        source: rusqlite::Error,
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
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

pub struct Admin {
    conn: Connection,
}

impl Admin {
    pub fn open(dir: &Path) -> Result<Self, AdminError> {
        let path = existing(dir)?;
        let conn = connect(&path, OpenFlags::default())?;
        require_current(&path, &conn)?;
        Ok(Self { conn })
    }

    pub fn open_read_only(dir: &Path) -> Result<Self, AdminError> {
        let path = existing(dir)?;
        let conn = connect(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        require_current(&path, &conn)?;
        Ok(Self { conn })
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
                source,
            })?;
        schema::migrate(&mut conn).map_err(|error| AdminError::Migrate {
            path: path.clone(),
            source: error.into(),
        })?;
        Ok(Self { conn })
    }

    pub fn secret(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM secrets WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    /// Inserts an unsequenced secret only when absent.
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

    pub fn pair_code_pending(&self, code_sha256: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM pair_codes WHERE code_sha256 = ?1",
                [code_sha256],
                |_row| Ok(()),
            )
            .optional()
            .context("look up pair code")?
            .is_some())
    }

    pub fn add_pair_code(&self, code_sha256: &str, expires_at: Timestamp) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("begin saving a pair code")?;
        crate::pairing::add_pair_code(&tx, code_sha256, expires_at)?;
        tx.commit().context("commit the pair code")
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
        source,
    };
    let conn = Connection::open_with_flags(path, flags).map_err(open)?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(open)?;
    Ok(conn)
}

fn version(path: &Path, conn: &Connection) -> Result<usize, AdminError> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|source| AdminError::Open {
            path: path.to_owned(),
            source,
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
