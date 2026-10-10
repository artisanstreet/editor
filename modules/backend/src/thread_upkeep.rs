//! The Forge's upkeep of where threads live: filing threads nobody has used
//! or viewed for a day into History, and erasing threads archived for longer
//! than the retention period. Each pass that changes anything wakes the
//! Editors, whose recent-threads listings then push the change.

use std::sync::Arc;
use std::time::Duration;

use artisan_database::{Repository, RepositoryError};
use artisan_transport::CancelHandle;

use crate::conversation_commit_notifier::ConversationCommitNotifier;
use crate::{CommandOrigin as _, SystemCommandOrigin};

/// How often the Forge tidies threads. A quiet thread reaches History at
/// most this long after its day is up.
const TIDY_INTERVAL: Duration = Duration::from_mins(5);

/// The most archived threads erased in one pass; the rest wait for the next.
const ERASE_BATCH: usize = 64;

/// Tidies threads until `cancel` fires: once now, then periodically.
pub(crate) async fn keep_threads_tidy(
    repository: Repository,
    notifier: ConversationCommitNotifier,
    cancel: Arc<CancelHandle>,
) {
    loop {
        match tidy(&repository).await {
            Ok(true) => notifier.wake_any(),
            Ok(false) => {}
            Err(error) => eprintln!("thread upkeep failed: {error}"),
        }
        tokio::select! {
            () = cancel.wait() => return,
            () = tokio::time::sleep(TIDY_INTERVAL) => {}
        }
    }
}

/// One pass; whether any thread moved or was erased.
async fn tidy(repository: &Repository) -> Result<bool, RepositoryError> {
    let Ok(at) = SystemCommandOrigin.acceptance_instant() else {
        return Ok(false);
    };
    let filed = repository.file_quiet_threads(at).await?;
    let erased = repository
        .erase_expired_archived_threads(at, ERASE_BATCH)
        .await?;
    Ok(filed > 0 || !erased.is_empty())
}
