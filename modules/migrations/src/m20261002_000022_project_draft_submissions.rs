//! Makes sending a project's new-task draft idempotent.
//!
//! Submitting a project's composer draft creates the thread its message is
//! queued in. `project_draft_submissions` records each project draft
//! revision the Forge sent, with the thread it created and the message it
//! queued, so submitting the same revision again answers that thread and
//! message instead of creating another. Thread drafts keep their record in
//! `composer_draft_submissions`, whose key names an existing thread.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE project_draft_submissions (\
                    project_id TEXT NOT NULL,\
                    draft_revision INTEGER NOT NULL,\
                    thread_id TEXT NOT NULL UNIQUE,\
                    message_id TEXT NOT NULL UNIQUE,\
                    cleared_revision INTEGER NOT NULL,\
                    submitted_at_ms INTEGER NOT NULL,\
                    PRIMARY KEY (project_id, draft_revision),\
                    FOREIGN KEY (project_id) REFERENCES attached_projects(project_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    FOREIGN KEY (thread_id) REFERENCES threads(thread_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    FOREIGN KEY (message_id) REFERENCES messages(message_id) ON UPDATE RESTRICT ON DELETE RESTRICT,\
                    CHECK (typeof(draft_revision) = 'integer' AND draft_revision > 0),\
                    CHECK (typeof(cleared_revision) = 'integer' AND cleared_revision > draft_revision),\
                    CHECK (typeof(submitted_at_ms) = 'integer')\
                )",
            )
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS project_draft_submissions")
            .await
            .map(|_| ())
    }
}
