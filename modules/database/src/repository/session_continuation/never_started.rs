//! Detection of runs that never reached their provider.

use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};

use artisan_domain::RunId;

use crate::entities::{self, AssistantRunLifecycle};
use crate::repository::{RepositoryError, database_error};

/// Whether a settled run never reached its provider: no provider binding was
/// ever recorded and no batch or assistant item was ever committed for it.
/// Such a run cannot hold provider-side state worth continuing, whichever
/// engine it selected, so it is not continuation history.
pub(super) async fn never_started<C: ConnectionTrait>(
    database: &C,
    run: &entities::assistant_run::Model,
    run_id: &RunId,
) -> Result<bool, RepositoryError> {
    let settled = matches!(
        run.lifecycle,
        AssistantRunLifecycle::Interrupted
            | AssistantRunLifecycle::Completed
            | AssistantRunLifecycle::Failed
            | AssistantRunLifecycle::Cancelled
    );
    if !settled
        || run.provider_binding.is_some()
        || run.provider_binding_version.is_some()
        || run.provider_bound_at_ms.is_some()
    {
        return Ok(false);
    }
    let receipt = entities::run_batch_receipt::Entity::find()
        .filter(entities::run_batch_receipt::Column::RunId.eq(run_id.as_str()))
        .one(database)
        .await
        .map_err(|source| database_error("read never-started run receipts", source))?;
    if receipt.is_some() {
        return Ok(false);
    }
    let output = entities::conversation_item::Entity::find()
        .filter(entities::conversation_item::Column::RunId.eq(run_id.as_str()))
        .filter(
            entities::conversation_item::Column::ItemKind
                .eq(entities::ConversationItemKind::AssistantMessage),
        )
        .one(database)
        .await
        .map_err(|source| database_error("read never-started run output", source))?;
    Ok(output.is_none())
}
