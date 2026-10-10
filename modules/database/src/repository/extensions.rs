//! Managed extension installations.
//!
//! One row per managed skill: Artisan's own skills once the user changed
//! them from their enabled default, and every skill the user adopted. The
//! Forge's extension sync reads them to decide what each engine gets.

use sea_orm::{ConnectionTrait, DbBackend, Statement, Value};

use artisan_domain::UnixMillis;

use super::{Repository, RepositoryError, corrupt_data, database_error};

const SELECT: &str = "SELECT extension_id, origin, name, digest, enabled FROM extension_installations ORDER BY name ASC";

/// Where a managed extension comes from.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StoredExtensionOrigin {
    /// Ships with Artisan.
    FirstParty,
    /// Adopted by the user into the content store.
    Adopted,
}

impl StoredExtensionOrigin {
    const fn as_str(self) -> &'static str {
        match self {
            Self::FirstParty => "first_party",
            Self::Adopted => "adopted",
        }
    }
}

/// One managed extension as stored.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StoredExtension {
    /// Stable identifier.
    pub extension_id: String,
    /// Origin.
    pub origin: StoredExtensionOrigin,
    /// Skill name; unique among managed extensions.
    pub name: String,
    /// Content digest of the stored skill (for Artisan's own skills, the
    /// digest last placed).
    pub digest: String,
    /// Whether engines get it.
    pub enabled: bool,
}

impl Repository {
    /// Reads every managed extension, by name.
    ///
    /// # Errors
    ///
    /// Returns corrupt-data or database failures.
    pub async fn read_extensions(&self) -> Result<Vec<StoredExtension>, RepositoryError> {
        let rows = self
            .database
            .query_all_raw(Statement::from_string(DbBackend::Sqlite, SELECT))
            .await
            .map_err(|source| database_error("read extensions", source))?;
        rows.iter().map(extension_from_row).collect()
    }

    /// Inserts or replaces one managed extension.
    ///
    /// # Errors
    ///
    /// Returns a database failure, including when another managed extension
    /// already holds the name.
    pub async fn save_extension(
        &self,
        extension: &StoredExtension,
        at: UnixMillis,
    ) -> Result<(), RepositoryError> {
        let _write_gate = self.acquire_write_gate("save_extension").await;
        self.database
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "INSERT INTO extension_installations (extension_id, origin, name, digest, enabled, updated_at_ms) VALUES (?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(extension_id) DO UPDATE SET origin = excluded.origin, name = excluded.name, digest = excluded.digest, enabled = excluded.enabled, updated_at_ms = excluded.updated_at_ms",
                [
                    text(&extension.extension_id),
                    text(extension.origin.as_str()),
                    text(&extension.name),
                    text(&extension.digest),
                    Value::BigInt(Some(i64::from(extension.enabled))),
                    Value::BigInt(Some(at.as_millis())),
                ],
            ))
            .await
            .map(|_| ())
            .map_err(|source| database_error("save extension", source))
    }
}

fn extension_from_row(row: &sea_orm::QueryResult) -> Result<StoredExtension, RepositoryError> {
    let column = |index: usize, field: &'static str| {
        row.try_get_by_index::<String>(index)
            .map_err(|error| corrupt_data("extension_installations", field, error))
    };
    let origin = match column(1, "origin")?.as_str() {
        "first_party" => StoredExtensionOrigin::FirstParty,
        "adopted" => StoredExtensionOrigin::Adopted,
        other => {
            return Err(corrupt_data(
                "extension_installations",
                "origin",
                format!("unknown origin {other:?}"),
            ));
        }
    };
    let enabled = row
        .try_get_by_index::<i64>(4)
        .map_err(|error| corrupt_data("extension_installations", "enabled", error))?;
    Ok(StoredExtension {
        extension_id: column(0, "extension_id")?,
        origin,
        name: column(2, "name")?,
        digest: column(3, "digest")?,
        enabled: enabled != 0,
    })
}

fn text(value: &str) -> Value {
    Value::String(Some(value.to_owned()))
}
