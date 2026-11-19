#[cfg(test)]
mod tests {
    use forge_store::admin::{Admin, AdminError};

    fn schema_of(dir: &std::path::Path) -> usize {
        rusqlite::Connection::open(dir.join("forge.sqlite3"))
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    fn older_database() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        rusqlite::Connection::open(dir.path().join("forge.sqlite3"))
            .unwrap()
            .execute_batch(include_str!("fixtures/schema-v3.sql"))
            .unwrap();
        dir
    }

    #[test]
    fn administration_beside_an_older_forge_leaves_its_database_alone() {
        let dir = older_database();
        let before = schema_of(dir.path());

        let live = Admin::open(dir.path());
        let read_only = Admin::open_read_only(dir.path());

        assert!(matches!(live, Err(AdminError::Schema { .. })));
        assert!(matches!(read_only, Err(AdminError::Schema { .. })));
        assert_eq!(schema_of(dir.path()), before);
        assert_eq!(
            forge_store::admin::schema_version(dir.path()).unwrap(),
            Some(before)
        );
    }

    #[test]
    fn administration_never_creates_a_database() {
        let dir = tempfile::tempdir().unwrap();

        assert!(matches!(
            Admin::open(dir.path()),
            Err(AdminError::Missing { .. })
        ));
        assert!(!dir.path().join("forge.sqlite3").exists());
    }
}
