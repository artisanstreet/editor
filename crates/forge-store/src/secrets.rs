use crate::error::{AtStep as _, StorageError};
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) fn secret(conn: &Connection, key: &str) -> Result<Option<String>, StorageError> {
    conn.query_row(include_str!("sql/secrets/get.sql"), [key], |row| row.get(0))
        .optional()
        .at("secrets/get.sql")
}

pub(crate) fn secret_or_insert(
    conn: &Connection,
    key: &str,
    value: &str,
) -> Result<String, StorageError> {
    conn.execute(
        include_str!("sql/secrets/insert_absent.sql"),
        params![key, value],
    )
    .at("secrets/insert_absent.sql")?;
    conn.query_row(include_str!("sql/secrets/get.sql"), [key], |row| row.get(0))
        .at("secrets/get.sql")
}

pub(crate) fn set_secret(
    conn: &Connection,
    key: &str,
    value: Option<&str>,
) -> Result<(), StorageError> {
    match value {
        Some(value) => conn
            .execute(include_str!("sql/secrets/set.sql"), params![key, value])
            .at("secrets/set.sql")?,
        None => conn
            .execute(include_str!("sql/secrets/delete.sql"), [key])
            .at("secrets/delete.sql")?,
    };
    Ok(())
}
