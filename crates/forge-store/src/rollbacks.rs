use crate::Store;
use crate::error::AtStep as _;
use crate::error::StorageError;
use crate::writer::Tx;
use artisan_api::DatabaseRollback;
use rusqlite::{Connection, OptionalExtension, params};

crate::error::operation_error!(
    DismissRollbackError(StorageError),
    "dismiss the database rollback notice"
);

impl Tx<'_> {
    pub(crate) fn record_rollback(&self, rollback: &DatabaseRollback) -> Result<(), StorageError> {
        self.execute(
            include_str!("sql/rollbacks/record.sql"),
            params![
                rollback.found,
                rollback.restored,
                rollback.set_aside,
                rollback.at
            ],
        )
        .at("rollbacks/record.sql")?;
        Ok(())
    }
}

pub(crate) fn current(conn: &Connection) -> Result<Option<DatabaseRollback>, StorageError> {
    conn.query_row(include_str!("sql/rollbacks/current.sql"), [], |row| {
        Ok(DatabaseRollback {
            found: row.get(0)?,
            restored: row.get(1)?,
            set_aside: row.get(2)?,
            at: row.get(3)?,
        })
    })
    .optional()
    .at("rollbacks/current.sql")
}

impl Store {
    pub async fn dismiss_rollback(&self) -> Result<(), DismissRollbackError> {
        self.write_acknowledged(|tx| {
            tx.execute(include_str!("sql/rollbacks/dismiss.sql"), [])
                .at("rollbacks/dismiss.sql")?;
            Ok::<_, StorageError>(())
        })
        .await
        .map_err(|cause| DismissRollbackError {
            cause: Box::new(cause),
        })
    }
}
