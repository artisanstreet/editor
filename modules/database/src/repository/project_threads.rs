//! Attached-project and thread-catalog persistence.

use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseTransaction, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, TransactionTrait,
};

use artisan_domain::{
    CommandReceipt, DirectoryId, DisplayName, ProjectId, ProjectSummary, ReceiptDisposition,
    RequestId, RootPath, THREAD_LISTING_MAX_THREADS, ThreadId, ThreadListing, ThreadSummary,
    ThreadTitle, UnixMillis,
};

use crate::entities::{self, CommandKind};

use super::{Repository, RepositoryError, corrupt_data, database_error, millis};

/// Storage input for an attach command after Forge resolves and mints values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachProjectInput {
    pub request_id: RequestId,
    pub directory_id: DirectoryId,
    pub project_id: ProjectId,
    pub root_path: RootPath,
    pub display_name: DisplayName,
    pub attached_at: UnixMillis,
}

/// Durable attach receipt paired with the stable project result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachProjectResult {
    pub receipt: CommandReceipt,
    pub project: ProjectSummary,
}

/// Storage input for a create command after Forge mints the thread identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateThreadInput {
    pub request_id: RequestId,
    pub thread_id: ThreadId,
    pub project_id: ProjectId,
    pub title: ThreadTitle,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
}

/// Durable create receipt paired with the original thread result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateThreadResult {
    pub receipt: CommandReceipt,
    pub thread: ThreadSummary,
}

impl Repository {
    /// Checks an attach receipt before Forge resolves the directory again.
    ///
    /// # Errors
    ///
    /// Returns an idempotency conflict when the request id names another
    /// command or directory, corrupt persisted data, or a database failure.
    pub async fn lookup_attach_project(
        &self,
        request_id: &RequestId,
        directory_id: &DirectoryId,
    ) -> Result<Option<AttachProjectResult>, RepositoryError> {
        lookup_attach_receipt(
            &self.database,
            request_id,
            directory_id,
            ReceiptDisposition::Duplicate,
        )
        .await
    }

    /// Persists a project effect and its globally unique command receipt.
    ///
    /// `root_path` is the canonical path already resolved by Forge. Call
    /// [`Self::lookup_attach_project`] before filesystem resolution; this
    /// method repeats the lookup transactionally to close concurrent races.
    ///
    /// # Errors
    ///
    /// Returns a project or request identity conflict, corrupt persisted data,
    /// or a preserved database failure. Rejected effects are rolled back.
    pub async fn attach_project(
        &self,
        input: AttachProjectInput,
    ) -> Result<AttachProjectResult, RepositoryError> {
        if let Some(duplicate) = self
            .lookup_attach_project(&input.request_id, &input.directory_id)
            .await?
        {
            return Ok(duplicate);
        }

        let transaction = self
            .database
            .begin()
            .await
            .map_err(|source| database_error("begin attach-project transaction", source))?;
        let inserted = insert_project(&transaction, &input).await?;
        let project = if inserted == 1 {
            input.project_summary()
        } else {
            match project_row_by_id(&transaction, &input.project_id).await? {
                Some(row) => {
                    let existing = project_summary(row)?;
                    if existing.root_path != input.root_path {
                        return rollback_with_error(
                            transaction,
                            RepositoryError::ProjectConflict {
                                project_id: input.project_id,
                                existing_root_path: existing.root_path,
                                requested_root_path: input.root_path,
                            },
                        )
                        .await;
                    }
                    existing
                }
                None => project_row_by_root(&transaction, &input.root_path)
                    .await?
                    .map(project_summary)
                    .transpose()?
                    .ok_or(RepositoryError::Invariant {
                        reason: "project insert was ignored without an identifiable conflict",
                    })?,
            }
        };

        let receipt_inserted = insert_attach_receipt(&transaction, &input, &project).await?;
        if receipt_inserted == 0 {
            let result = lookup_attach_receipt(
                &transaction,
                &input.request_id,
                &input.directory_id,
                ReceiptDisposition::Duplicate,
            )
            .await;
            return rollback_with_result(transaction, result).await;
        }

        transaction
            .commit()
            .await
            .map_err(|source| database_error("commit attach-project transaction", source))?;
        Ok(AttachProjectResult {
            receipt: command_receipt(input.request_id, ReceiptDisposition::Accepted),
            project,
        })
    }

    /// Checks a create-thread receipt before Forge mints another thread id.
    ///
    /// # Errors
    ///
    /// Returns an idempotency conflict when the request id names another
    /// command payload, corrupt persisted data, or a database failure.
    pub async fn lookup_create_thread(
        &self,
        request_id: &RequestId,
        project_id: &ProjectId,
        title: &ThreadTitle,
    ) -> Result<Option<CreateThreadResult>, RepositoryError> {
        lookup_create_receipt(
            &self.database,
            request_id,
            project_id,
            title,
            ReceiptDisposition::Duplicate,
        )
        .await
    }

    /// Persists a thread effect and its globally unique command receipt.
    ///
    /// Call [`Self::lookup_create_thread`] before minting a thread id; this
    /// method repeats the lookup transactionally to close concurrent races.
    ///
    /// # Errors
    ///
    /// Returns a missing-project, chronology, thread, or request conflict;
    /// corrupt persisted data; or a preserved database failure.
    pub async fn create_thread(
        &self,
        input: CreateThreadInput,
    ) -> Result<CreateThreadResult, RepositoryError> {
        if let Some(duplicate) = self
            .lookup_create_thread(&input.request_id, &input.project_id, &input.title)
            .await?
        {
            return Ok(duplicate);
        }
        if input.updated_at < input.created_at {
            return Err(RepositoryError::InvalidChronology {
                earlier_field: "created_at",
                later_field: "updated_at",
            });
        }
        if project_row_by_id(&self.database, &input.project_id)
            .await?
            .is_none()
        {
            return Err(RepositoryError::ProjectNotFound {
                project_id: input.project_id,
            });
        }

        let transaction = self
            .database
            .begin()
            .await
            .map_err(|source| database_error("begin create-thread transaction", source))?;
        let summary = input.thread_summary();
        let inserted = insert_thread(&transaction, &summary).await?;
        if inserted == 0 {
            let duplicate = lookup_create_receipt(
                &transaction,
                &input.request_id,
                &input.project_id,
                &input.title,
                ReceiptDisposition::Duplicate,
            )
            .await?;
            if let Some(duplicate) = duplicate {
                transaction.rollback().await.map_err(|source| {
                    database_error("finish duplicate create-thread request", source)
                })?;
                return Ok(duplicate);
            }
            return rollback_with_error(
                transaction,
                RepositoryError::ThreadConflict {
                    thread_id: input.thread_id,
                },
            )
            .await;
        }

        let receipt_inserted = insert_create_receipt(&transaction, &input).await?;
        if receipt_inserted == 0 {
            let result = lookup_create_receipt(
                &transaction,
                &input.request_id,
                &input.project_id,
                &input.title,
                ReceiptDisposition::Duplicate,
            )
            .await;
            return rollback_with_result(transaction, result).await;
        }

        transaction
            .commit()
            .await
            .map_err(|source| database_error("commit create-thread transaction", source))?;
        Ok(CreateThreadResult {
            receipt: command_receipt(input.request_id, ReceiptDisposition::Accepted),
            thread: summary,
        })
    }

    /// Lists project threads by deterministic recency.
    ///
    /// # Errors
    ///
    /// Returns a missing-project error, a typed bounded-listing overflow,
    /// corrupt persisted data, or a preserved database failure.
    pub async fn list_threads(
        &self,
        project_id: &ProjectId,
    ) -> Result<ThreadListing, RepositoryError> {
        if project_row_by_id(&self.database, project_id)
            .await?
            .is_none()
        {
            return Err(RepositoryError::ProjectNotFound {
                project_id: project_id.clone(),
            });
        }

        let rows = entities::thread::Entity::find()
            .filter(entities::thread::Column::ProjectId.eq(project_id.as_str()))
            .order_by_desc(entities::thread::Column::UpdatedAtMs)
            .order_by_asc(entities::thread::Column::ThreadId)
            .limit((THREAD_LISTING_MAX_THREADS + 1) as u64)
            .all(&self.database)
            .await
            .map_err(|source| database_error("list project threads", source))?;
        let summaries = rows
            .into_iter()
            .map(thread_summary)
            .collect::<Result<Vec<_>, _>>()?;
        ThreadListing::new(summaries).map_err(|source| RepositoryError::ThreadListing { source })
    }
}

impl AttachProjectInput {
    fn project_summary(&self) -> ProjectSummary {
        ProjectSummary {
            project_id: self.project_id.clone(),
            display_name: self.display_name.clone(),
            root_path: self.root_path.clone(),
            attached_at: self.attached_at,
        }
    }
}

impl CreateThreadInput {
    fn thread_summary(&self) -> ThreadSummary {
        ThreadSummary {
            thread_id: self.thread_id.clone(),
            project_id: self.project_id.clone(),
            title: self.title.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

async fn insert_project(
    database: &impl ConnectionTrait,
    input: &AttachProjectInput,
) -> Result<u64, RepositoryError> {
    entities::attached_project::Entity::insert(entities::attached_project::ActiveModel {
        project_id: Set(input.project_id.as_str().to_owned()),
        root_path: Set(input.root_path.as_str().to_owned()),
        display_name: Set(input.display_name.as_str().to_owned()),
        attached_at_ms: Set(millis(input.attached_at)),
    })
    .on_conflict(do_nothing_on_conflict())
    .exec_without_returning(database)
    .await
    .map_err(|source| database_error("attach project", source))
}

async fn insert_thread(
    database: &impl ConnectionTrait,
    summary: &ThreadSummary,
) -> Result<u64, RepositoryError> {
    entities::thread::Entity::insert(entities::thread::ActiveModel {
        thread_id: Set(summary.thread_id.as_str().to_owned()),
        project_id: Set(summary.project_id.as_str().to_owned()),
        title: Set(summary.title.as_str().to_owned()),
        created_at_ms: Set(millis(summary.created_at)),
        updated_at_ms: Set(millis(summary.updated_at)),
    })
    .on_conflict(do_nothing_on_conflict())
    .exec_without_returning(database)
    .await
    .map_err(|source| database_error("create thread", source))
}

async fn insert_attach_receipt(
    database: &impl ConnectionTrait,
    input: &AttachProjectInput,
    project: &ProjectSummary,
) -> Result<u64, RepositoryError> {
    entities::command_receipt::Entity::insert(entities::command_receipt::ActiveModel {
        request_id: Set(input.request_id.as_str().to_owned()),
        command_kind: Set(CommandKind::AttachProject),
        directory_id: Set(Some(input.directory_id.as_str().to_owned())),
        project_id: Set(Some(project.project_id.as_str().to_owned())),
        thread_id: Set(None),
        title: Set(None),
        message_id: Set(None),
        body: Set(None),
        accepted_at_ms: Set(millis(input.attached_at)),
    })
    .on_conflict(do_nothing_on_conflict())
    .exec_without_returning(database)
    .await
    .map_err(|source| database_error("record attach-project receipt", source))
}

async fn insert_create_receipt(
    database: &impl ConnectionTrait,
    input: &CreateThreadInput,
) -> Result<u64, RepositoryError> {
    entities::command_receipt::Entity::insert(entities::command_receipt::ActiveModel {
        request_id: Set(input.request_id.as_str().to_owned()),
        command_kind: Set(CommandKind::CreateThread),
        directory_id: Set(None),
        project_id: Set(Some(input.project_id.as_str().to_owned())),
        thread_id: Set(Some(input.thread_id.as_str().to_owned())),
        title: Set(Some(input.title.as_str().to_owned())),
        message_id: Set(None),
        body: Set(None),
        accepted_at_ms: Set(millis(input.created_at)),
    })
    .on_conflict(do_nothing_on_conflict())
    .exec_without_returning(database)
    .await
    .map_err(|source| database_error("record create-thread receipt", source))
}

async fn lookup_attach_receipt(
    database: &impl ConnectionTrait,
    request_id: &RequestId,
    directory_id: &DirectoryId,
    disposition: ReceiptDisposition,
) -> Result<Option<AttachProjectResult>, RepositoryError> {
    let Some(row) = receipt_row_by_id(database, request_id).await? else {
        return Ok(None);
    };
    if row.command_kind != CommandKind::AttachProject
        || row.directory_id.as_deref() != Some(directory_id.as_str())
    {
        return Err(RepositoryError::IdempotencyConflict {
            request_id: request_id.clone(),
        });
    }
    let project_id = ProjectId::parse(required(row.project_id, "command_receipts", "project_id")?)
        .map_err(|error| corrupt_data("command_receipts", "project_id", &error))?;
    let project = project_row_by_id(database, &project_id)
        .await?
        .map(project_summary)
        .transpose()?
        .ok_or(RepositoryError::Invariant {
            reason: "attach receipt references a missing project",
        })?;
    Ok(Some(AttachProjectResult {
        receipt: command_receipt(request_id.clone(), disposition),
        project,
    }))
}

async fn lookup_create_receipt(
    database: &impl ConnectionTrait,
    request_id: &RequestId,
    project_id: &ProjectId,
    title: &ThreadTitle,
    disposition: ReceiptDisposition,
) -> Result<Option<CreateThreadResult>, RepositoryError> {
    let Some(row) = receipt_row_by_id(database, request_id).await? else {
        return Ok(None);
    };
    if row.command_kind != CommandKind::CreateThread
        || row.project_id.as_deref() != Some(project_id.as_str())
        || row.title.as_deref() != Some(title.as_str())
    {
        return Err(RepositoryError::IdempotencyConflict {
            request_id: request_id.clone(),
        });
    }
    let thread_id = ThreadId::parse(required(row.thread_id, "command_receipts", "thread_id")?)
        .map_err(|error| corrupt_data("command_receipts", "thread_id", &error))?;
    let thread = thread_row_by_id(database, &thread_id)
        .await?
        .map(thread_summary)
        .transpose()?
        .ok_or(RepositoryError::Invariant {
            reason: "create receipt references a missing thread",
        })?;
    Ok(Some(CreateThreadResult {
        receipt: command_receipt(request_id.clone(), disposition),
        thread,
    }))
}

async fn receipt_row_by_id(
    database: &impl ConnectionTrait,
    request_id: &RequestId,
) -> Result<Option<entities::CommandReceipt>, RepositoryError> {
    entities::command_receipt::Entity::find_by_id(request_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find command receipt", source))
}

async fn project_row_by_id(
    database: &impl ConnectionTrait,
    project_id: &ProjectId,
) -> Result<Option<entities::AttachedProject>, RepositoryError> {
    entities::attached_project::Entity::find_by_id(project_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find attached project by id", source))
}

async fn project_row_by_root(
    database: &impl ConnectionTrait,
    root_path: &RootPath,
) -> Result<Option<entities::AttachedProject>, RepositoryError> {
    entities::attached_project::Entity::find()
        .filter(entities::attached_project::Column::RootPath.eq(root_path.as_str()))
        .one(database)
        .await
        .map_err(|source| database_error("find attached project by root", source))
}

async fn thread_row_by_id(
    database: &impl ConnectionTrait,
    thread_id: &ThreadId,
) -> Result<Option<entities::Thread>, RepositoryError> {
    entities::thread::Entity::find_by_id(thread_id.as_str())
        .one(database)
        .await
        .map_err(|source| database_error("find thread by id", source))
}

fn project_summary(row: entities::AttachedProject) -> Result<ProjectSummary, RepositoryError> {
    Ok(ProjectSummary {
        project_id: ProjectId::parse(row.project_id)
            .map_err(|error| corrupt_data("attached_projects", "project_id", &error))?,
        display_name: DisplayName::parse(row.display_name)
            .map_err(|error| corrupt_data("attached_projects", "display_name", &error))?,
        root_path: RootPath::parse(row.root_path)
            .map_err(|error| corrupt_data("attached_projects", "root_path", &error))?,
        attached_at: UnixMillis::from_millis(row.attached_at_ms),
    })
}

fn thread_summary(row: entities::Thread) -> Result<ThreadSummary, RepositoryError> {
    Ok(ThreadSummary {
        thread_id: ThreadId::parse(row.thread_id)
            .map_err(|error| corrupt_data("threads", "thread_id", &error))?,
        project_id: ProjectId::parse(row.project_id)
            .map_err(|error| corrupt_data("threads", "project_id", &error))?,
        title: ThreadTitle::parse(row.title)
            .map_err(|error| corrupt_data("threads", "title", &error))?,
        created_at: UnixMillis::from_millis(row.created_at_ms),
        updated_at: UnixMillis::from_millis(row.updated_at_ms),
    })
}

fn command_receipt(request_id: RequestId, disposition: ReceiptDisposition) -> CommandReceipt {
    CommandReceipt {
        request_id,
        disposition,
    }
}

fn required(
    value: Option<String>,
    table: &'static str,
    field: &'static str,
) -> Result<String, RepositoryError> {
    value.ok_or_else(|| corrupt_data(table, field, "required value is null"))
}

async fn rollback_with_error<T>(
    transaction: DatabaseTransaction,
    error: RepositoryError,
) -> Result<T, RepositoryError> {
    transaction
        .rollback()
        .await
        .map_err(|source| database_error("roll back rejected transaction", source))?;
    Err(error)
}

async fn rollback_with_result<T>(
    transaction: DatabaseTransaction,
    result: Result<Option<T>, RepositoryError>,
) -> Result<T, RepositoryError> {
    transaction
        .rollback()
        .await
        .map_err(|source| database_error("roll back duplicate transaction", source))?;
    result?.ok_or(RepositoryError::Invariant {
        reason: "receipt insert was ignored without an identifiable request",
    })
}

fn do_nothing_on_conflict() -> OnConflict {
    let mut conflict = OnConflict::new();
    conflict.do_nothing();
    conflict.clone()
}
