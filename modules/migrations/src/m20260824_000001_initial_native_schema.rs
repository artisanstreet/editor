//! Initial schema for attached projects, threads, and queued messages.

use sea_orm_migration::prelude::*;

/// Creates the first native schema.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        create_attached_projects(manager).await?;
        create_threads(manager).await?;
        create_messages(manager).await?;
        create_message_dispatches(manager).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(MessageDispatches::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Messages::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Threads::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(AttachedProjects::Table).to_owned())
            .await
    }
}

async fn create_attached_projects(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(AttachedProjects::Table)
                .col(
                    ColumnDef::new(AttachedProjects::ProjectId)
                        .string()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(AttachedProjects::RootPath).text().not_null())
                .col(
                    ColumnDef::new(AttachedProjects::DisplayName)
                        .string()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(AttachedProjects::AttachedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .to_owned(),
        )
        .await?;

    create_attached_project_indexes(manager).await
}

async fn create_attached_project_indexes(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_index(
            Index::create()
                .name("uq_attached_projects_root_path")
                .table(AttachedProjects::Table)
                .col(AttachedProjects::RootPath)
                .unique()
                .to_owned(),
        )
        .await
}

async fn create_threads(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(Threads::Table)
                .col(
                    ColumnDef::new(Threads::ThreadId)
                        .string()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(Threads::ProjectId).string().not_null())
                .col(ColumnDef::new(Threads::Title).text().not_null())
                .col(
                    ColumnDef::new(Threads::CreatedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(Threads::UpdatedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_threads_project_id_attached_projects")
                        .from(Threads::Table, Threads::ProjectId)
                        .to(AttachedProjects::Table, AttachedProjects::ProjectId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .check((
                    "ck_threads_title_nonblank",
                    Expr::cust("length(trim(title)) > 0"),
                ))
                .check((
                    "ck_threads_updated_not_before_created",
                    Expr::col(Threads::UpdatedAtMs).gte(Expr::col(Threads::CreatedAtMs)),
                ))
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("idx_threads_project_id")
                .table(Threads::Table)
                .col(Threads::ProjectId)
                .to_owned(),
        )
        .await
}

async fn create_messages(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(Messages::Table)
                .col(
                    ColumnDef::new(Messages::MessageId)
                        .string()
                        .not_null()
                        .primary_key(),
                )
                .col(ColumnDef::new(Messages::ThreadId).string().not_null())
                .col(ColumnDef::new(Messages::Ordinal).big_integer().not_null())
                .col(ColumnDef::new(Messages::Body).text().not_null())
                .col(
                    ColumnDef::new(Messages::AcceptedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_messages_thread_id_threads")
                        .from(Messages::Table, Messages::ThreadId)
                        .to(Threads::Table, Threads::ThreadId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .check((
                    "ck_messages_ordinal_nonnegative",
                    Expr::col(Messages::Ordinal).gte(0),
                ))
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("uq_messages_thread_id_ordinal")
                .table(Messages::Table)
                .col(Messages::ThreadId)
                .col(Messages::Ordinal)
                .unique()
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("idx_messages_thread_id")
                .table(Messages::Table)
                .col(Messages::ThreadId)
                .to_owned(),
        )
        .await
}

async fn create_message_dispatches(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(MessageDispatches::Table)
                .col(
                    ColumnDef::new(MessageDispatches::MessageId)
                        .string()
                        .not_null()
                        .primary_key(),
                )
                .col(
                    ColumnDef::new(MessageDispatches::CorrelationId)
                        .string()
                        .not_null(),
                )
                .col(ColumnDef::new(MessageDispatches::State).string().not_null())
                .col(
                    ColumnDef::new(MessageDispatches::AttemptCount)
                        .integer()
                        .not_null()
                        .default(0),
                )
                .col(
                    ColumnDef::new(MessageDispatches::QueuedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(MessageDispatches::AvailableAtMs)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(MessageDispatches::LeaseOwner)
                        .string()
                        .null(),
                )
                .col(
                    ColumnDef::new(MessageDispatches::LeaseExpiresAtMs)
                        .big_integer()
                        .null(),
                )
                .col(ColumnDef::new(MessageDispatches::LastError).text().null())
                .col(
                    ColumnDef::new(MessageDispatches::UpdatedAtMs)
                        .big_integer()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_message_dispatches_message_id_messages")
                        .from(MessageDispatches::Table, MessageDispatches::MessageId)
                        .to(Messages::Table, Messages::MessageId)
                        .on_delete(ForeignKeyAction::Restrict)
                        .on_update(ForeignKeyAction::Restrict),
                )
                .check((
                    "ck_message_dispatches_state",
                    Expr::col(MessageDispatches::State).is_in([
                        "queued",
                        "leased",
                        "running",
                        "completed",
                        "failed",
                    ]),
                ))
                .check((
                    "ck_message_dispatches_attempt_count_nonnegative",
                    Expr::col(MessageDispatches::AttemptCount).gte(0),
                ))
                .check((
                    "ck_message_dispatches_lease_metadata",
                    Cond::any()
                        .add(
                            Cond::all()
                                .add(
                                    Expr::col(MessageDispatches::State)
                                        .is_in(["leased", "running"]),
                                )
                                .add(Expr::col(MessageDispatches::LeaseOwner).is_not_null())
                                .add(Expr::col(MessageDispatches::LeaseExpiresAtMs).is_not_null()),
                        )
                        .add(
                            Cond::all()
                                .add(
                                    Expr::col(MessageDispatches::State)
                                        .is_not_in(["leased", "running"]),
                                )
                                .add(Expr::col(MessageDispatches::LeaseOwner).is_null())
                                .add(Expr::col(MessageDispatches::LeaseExpiresAtMs).is_null()),
                        ),
                ))
                .to_owned(),
        )
        .await?;

    create_message_dispatch_indexes(manager).await
}

async fn create_message_dispatch_indexes(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_index(
            Index::create()
                .name("uq_message_dispatches_correlation_id")
                .table(MessageDispatches::Table)
                .col(MessageDispatches::CorrelationId)
                .unique()
                .to_owned(),
        )
        .await?;

    manager
        .create_index(
            Index::create()
                .name("idx_message_dispatches_recovery")
                .table(MessageDispatches::Table)
                .col(MessageDispatches::State)
                .col(MessageDispatches::AvailableAtMs)
                .to_owned(),
        )
        .await
}

#[derive(DeriveIden)]
enum AttachedProjects {
    Table,
    ProjectId,
    RootPath,
    DisplayName,
    AttachedAtMs,
}

#[derive(DeriveIden)]
enum Threads {
    Table,
    ThreadId,
    ProjectId,
    Title,
    CreatedAtMs,
    UpdatedAtMs,
}

#[derive(DeriveIden)]
enum Messages {
    Table,
    MessageId,
    ThreadId,
    Ordinal,
    Body,
    AcceptedAtMs,
}

#[derive(DeriveIden)]
enum MessageDispatches {
    Table,
    MessageId,
    CorrelationId,
    State,
    AttemptCount,
    QueuedAtMs,
    AvailableAtMs,
    LeaseOwner,
    LeaseExpiresAtMs,
    LastError,
    UpdatedAtMs,
}
