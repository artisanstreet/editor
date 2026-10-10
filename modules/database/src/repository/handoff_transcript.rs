//! Bounded, read-only transcript for a portable engine handoff.
//!
//! When a thread moves to an engine that cannot resume the previous provider
//! session, the new session is seeded from Artisan's own conversation
//! projection instead. This module reads the settled user and assistant
//! messages of one thread, newest first, until a byte budget is met, so a
//! long thread never loads its whole history to build one handoff.

use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect};

use artisan_domain::{EngineSelection, RunId, ThreadId, TurnId};

use crate::entities::{self, ConversationItemKind, EntityLifecycle};

use super::session_continuation::load_run_config;
use super::{Repository, RepositoryError, database_error};

/// Rows read per page while walking back from the newest message.
const PAGE_ROWS: u64 = 128;

/// Who wrote one transcript entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoffRole {
    /// A message the user sent.
    User,
    /// A message an engine replied with.
    Assistant,
}

/// One settled message, in conversation order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandoffTranscriptEntry {
    /// Who wrote the message.
    pub role: HandoffRole,
    /// The message text exactly as projected.
    pub text: String,
}

/// The newest settled messages of a thread within a byte budget.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HandoffTranscript {
    /// Oldest first. Holds the newest messages whose combined text meets
    /// the requested budget; older messages are only counted.
    pub entries: Vec<HandoffTranscriptEntry>,
    /// Every settled message in the thread, including those not read.
    pub total_entries: u64,
    /// The thread's first user message, kept even when it falls outside
    /// `entries`, so a degraded handoff still names the original objective.
    pub first_user_message: Option<String>,
}

impl Repository {
    /// Reads the settled transcript a portable handoff is built from.
    ///
    /// Only completed, non-empty user and assistant messages count. The
    /// messages of `exclude_turn_id` (the turn being launched) are left out,
    /// since the handoff precedes that turn's own request. Pages walk back
    /// from the newest message until the entries read hold at least
    /// `budget_bytes` of text; each message counts at most
    /// `entry_cap_bytes` toward that budget, matching how the caller bounds
    /// a rendered entry.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError`] when a database read fails.
    pub async fn read_handoff_transcript(
        &self,
        thread_id: &ThreadId,
        exclude_turn_id: &TurnId,
        budget_bytes: usize,
        entry_cap_bytes: usize,
    ) -> Result<HandoffTranscript, RepositoryError> {
        let settled = || {
            entities::conversation_item::Entity::find()
                .filter(entities::conversation_item::Column::ThreadId.eq(thread_id.as_str()))
                .filter(entities::conversation_item::Column::TurnId.ne(exclude_turn_id.as_str()))
                .filter(
                    entities::conversation_item::Column::Lifecycle.eq(EntityLifecycle::Completed),
                )
                .filter(entities::conversation_item::Column::Body.ne(""))
        };
        let total_entries = settled()
            .count(&self.database)
            .await
            .map_err(|source| database_error("count handoff transcript", source))?;
        let first_user_message = settled()
            .filter(
                entities::conversation_item::Column::ItemKind.eq(ConversationItemKind::UserMessage),
            )
            .order_by_asc(entities::conversation_item::Column::Ordinal)
            .one(&self.database)
            .await
            .map_err(|source| database_error("read handoff objective", source))?
            .map(|row| row.body);

        let mut newest_first = Vec::new();
        let mut counted_bytes = 0_usize;
        let mut before: Option<i64> = None;
        'pages: loop {
            let mut page = settled();
            if let Some(ordinal) = before {
                page = page.filter(entities::conversation_item::Column::Ordinal.lt(ordinal));
            }
            let rows = page
                .order_by_desc(entities::conversation_item::Column::Ordinal)
                .limit(PAGE_ROWS)
                .all(&self.database)
                .await
                .map_err(|source| database_error("read handoff transcript page", source))?;
            let exhausted = (rows.len() as u64) < PAGE_ROWS;
            for row in rows {
                before = Some(row.ordinal);
                counted_bytes = counted_bytes.saturating_add(row.body.len().min(entry_cap_bytes));
                newest_first.push(HandoffTranscriptEntry {
                    role: match row.item_kind {
                        ConversationItemKind::UserMessage => HandoffRole::User,
                        ConversationItemKind::AssistantMessage => HandoffRole::Assistant,
                    },
                    text: row.body,
                });
                if counted_bytes >= budget_bytes {
                    break 'pages;
                }
            }
            if exhausted {
                break;
            }
        }
        newest_first.reverse();
        Ok(HandoffTranscript {
            entries: newest_first,
            total_entries,
            first_user_message,
        })
    }

    /// Reads the immutable engine selection one run launched with.
    ///
    /// Returns `None` when the run is absent or belongs to another thread.
    ///
    /// # Errors
    ///
    /// Returns [`RepositoryError`] when the read fails or the run's stored
    /// configuration is corrupt.
    pub async fn read_run_engine_selection(
        &self,
        thread_id: &ThreadId,
        run_id: &RunId,
    ) -> Result<Option<EngineSelection>, RepositoryError> {
        let Some(row) = entities::assistant_run::Entity::find_by_id(run_id.as_str())
            .one(&self.database)
            .await
            .map_err(|source| database_error("read run engine selection", source))?
        else {
            return Ok(None);
        };
        if row.thread_id != thread_id.as_str() {
            return Ok(None);
        }
        Ok(Some(load_run_config(&row)?.selection().clone()))
    }
}
