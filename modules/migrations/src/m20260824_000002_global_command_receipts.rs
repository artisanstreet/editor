//! Adds globally unique, exact command receipts for durable idempotency.

use sea_orm_migration::prelude::*;

/// Adds the receipt authority without rewriting the initial native schema.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(CommandReceipts::Table)
                    .col(
                        ColumnDef::new(CommandReceipts::RequestId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(CommandReceipts::CommandKind)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(CommandReceipts::DirectoryId).string().null())
                    .col(ColumnDef::new(CommandReceipts::ProjectId).string().null())
                    .col(ColumnDef::new(CommandReceipts::ThreadId).string().null())
                    .col(ColumnDef::new(CommandReceipts::Title).text().null())
                    .col(ColumnDef::new(CommandReceipts::MessageId).string().null())
                    .col(ColumnDef::new(CommandReceipts::Body).text().null())
                    .col(
                        ColumnDef::new(CommandReceipts::AcceptedAtMs)
                            .big_integer()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_command_receipts_project_id_attached_projects")
                            .from(CommandReceipts::Table, CommandReceipts::ProjectId)
                            .to(AttachedProjects::Table, AttachedProjects::ProjectId)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Restrict),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_command_receipts_thread_id_threads")
                            .from(CommandReceipts::Table, CommandReceipts::ThreadId)
                            .to(Threads::Table, Threads::ThreadId)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Restrict),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_command_receipts_message_id_messages")
                            .from(CommandReceipts::Table, CommandReceipts::MessageId)
                            .to(Messages::Table, Messages::MessageId)
                            .on_delete(ForeignKeyAction::Restrict)
                            .on_update(ForeignKeyAction::Restrict),
                    )
                    .check((
                        "ck_command_receipts_exact_shape",
                        Expr::cust(
                            "(command_kind = 'attach_project' AND directory_id IS NOT NULL AND project_id IS NOT NULL AND thread_id IS NULL AND title IS NULL AND message_id IS NULL AND body IS NULL) OR \
                             (command_kind = 'create_thread' AND directory_id IS NULL AND project_id IS NOT NULL AND thread_id IS NOT NULL AND title IS NOT NULL AND message_id IS NULL AND body IS NULL) OR \
                             (command_kind = 'queue_first_message' AND directory_id IS NULL AND project_id IS NULL AND thread_id IS NOT NULL AND title IS NULL AND message_id IS NOT NULL AND body IS NOT NULL)",
                        ),
                    ))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_command_receipts_kind_thread_id")
                    .table(CommandReceipts::Table)
                    .col(CommandReceipts::CommandKind)
                    .col(CommandReceipts::ThreadId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_command_receipts_message_id")
                    .table(CommandReceipts::Table)
                    .col(CommandReceipts::MessageId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_command_receipts_kind_project_id")
                    .table(CommandReceipts::Table)
                    .col(CommandReceipts::CommandKind)
                    .col(CommandReceipts::ProjectId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(CommandReceipts::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum CommandReceipts {
    Table,
    RequestId,
    CommandKind,
    DirectoryId,
    ProjectId,
    ThreadId,
    Title,
    MessageId,
    Body,
    AcceptedAtMs,
}

#[derive(DeriveIden)]
enum AttachedProjects {
    Table,
    ProjectId,
}

#[derive(DeriveIden)]
enum Threads {
    Table,
    ThreadId,
}

#[derive(DeriveIden)]
enum Messages {
    Table,
    MessageId,
}
