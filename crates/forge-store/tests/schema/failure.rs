use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

#[test]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every other statement is authorised as usual"
)]
fn a_migration_that_fails_names_the_sql_file_that_failed() {
    let upgrade = Upgrade::of(&crate::Environment::system());
    let mut conn = Connection::open_in_memory().expect("open the database");
    migrate_to(&mut conn, 28, &upgrade).expect("migrate to the schema before 29");
    conn.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::CreateTable {
            table_name: "authorized_keys_revision",
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }))
    .expect("watch what the migration runs");

    let failed = migrate_to(&mut conn, 29, &upgrade).expect_err("migration 29 is refused");

    let said = diagnostics::chain(&failed);
    assert!(said.contains("apply schema migration 29"), "{said}");
    assert!(said.contains("migrations/v29.sql"), "{said}");
}
