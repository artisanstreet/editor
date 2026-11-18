use rusqlite::{Connection, OptionalExtension, params};

pub(crate) fn secret(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(include_str!("sql/secrets/get.sql"), [key], |row| row.get(0))
        .optional()
}

pub(crate) fn secret_or_insert(
    conn: &Connection,
    key: &str,
    value: &str,
) -> rusqlite::Result<String> {
    conn.execute(
        include_str!("sql/secrets/insert_absent.sql"),
        params![key, value],
    )?;
    conn.query_row(include_str!("sql/secrets/get.sql"), [key], |row| row.get(0))
}

pub(crate) fn set_secret(
    conn: &Connection,
    key: &str,
    value: Option<&str>,
) -> rusqlite::Result<()> {
    match value {
        Some(value) => conn.execute(include_str!("sql/secrets/set.sql"), params![key, value])?,
        None => conn.execute(include_str!("sql/secrets/delete.sql"), [key])?,
    };
    Ok(())
}
