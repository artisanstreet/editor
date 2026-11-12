use crate::{ItemId, ProjectId, Seq, ThreadId, Timestamp};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSearchQuery {
    pub query: String,
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextHit {
    pub thread_id: ThreadId,
    pub thread_title: Option<String>,
    pub project_id: ProjectId,
    pub item_id: ItemId,
    pub position: Seq,
    pub created_at: Timestamp,
    pub snippet: String,
    pub matched: (u32, u32),
    pub offset: (u32, u32),
    pub length: u32,
}
