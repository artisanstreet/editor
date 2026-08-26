//! Product concepts that do not depend on transport, storage, or presentation.
//!
//! This crate owns the application-domain vocabulary of the first native
//! end-to-end workflow: attach one Forge-visible directory by opaque identity,
//! create a project-scoped thread, and durably queue the first bounded text
//! message behind an accepted-or-duplicate receipt. Engine dispatch and
//! execution stay outside the crate: the conversation vocabulary stores
//! assistant output under an opaque Forge-minted run routing id, but models
//! no engine, provider, or run-lifecycle state.
//!
//! Structure:
//!
//! - [`bounds`] documents every ceiling and its unit (UTF-8 bytes throughout);
//! - [`identifiers`] validates the wire-facing identities, split by who mints
//!   them: clients mint only [`RequestId`], Forge mints everything else;
//! - [`text`] holds bounded display and message values;
//! - [`model`] holds listed and durable state values;
//! - [`conversation`] holds renderer snapshots, replay batches, and both
//!   durable item kinds;
//! - [`commands`] and [`events`] hold the workflow's mutations with their
//!   request correlation and its durable facts;
//! - [`time`] carries the schema's signed Unix epoch milliseconds.
//!
//! The domain is independent of Cap'n Proto, Quinn, `SeaORM`, GPUI, Tokio,
//! filesystem APIs, and wall-clock acquisition; it depends only on
//! `thiserror`. External values return typed errors instead of panicking.
//! Filesystem paths are carried as opaque descriptions without
//! canonicalization.

pub mod bounds;
pub mod commands;
pub mod conversation;
pub mod events;
pub mod identifiers;
mod legacy_workspace_id;
pub mod model;
pub mod text;
pub mod time;

pub use bounds::{
    CONVERSATION_PATCH_BATCH_MAX_PATCHES, CONVERSATION_QUERY_MAX_TURNS,
    CONVERSATION_TEXT_FRAGMENT_MAX_BYTES, DIRECTORY_LISTING_MAX_ENTRIES,
    DIRECTORY_LISTING_MAX_PLACES, DISPLAY_NAME_MAX_BYTES, IDENTIFIER_MAX_BYTES,
    MESSAGE_BODY_MAX_BYTES, PROJECT_LISTING_MAX_PROJECTS, ROOT_PATH_MAX_BYTES,
    THREAD_LISTING_MAX_THREADS, THREAD_TITLE_MAX_BYTES,
};
pub use commands::{
    AttachProject, Command, CreateThread, ListAttachedProjects, ListDirectories,
    ListProjectThreads, Query, QueueFirstMessage,
};
pub use conversation::{
    AssistantBody, AssistantBodyError, AssistantMessageItem, AssistantMessagePhase,
    ConversationCursor, ConversationItem, ConversationLifecycle, ConversationPatch,
    ConversationQuery, ConversationQueryBounds, ConversationRequest, ConversationSnapshot,
    ConversationSnapshotError, ConversationSubscribe, ConversationSubscriptionStart,
    ConversationTurn, ConversationUnsubscribe, CounterError, IncrementalText, IncrementalTextError,
    ItemOrdinal, LifecycleTransitionError, PatchBatch, PatchBatchError, PatchSequence,
    QueryTurnCount, QueryTurnCountError, Revision, TurnOrdinal, UserMessageItem,
};
pub use events::{Event, FirstMessageQueued, ProjectAttached, ThreadCreated};
pub use identifiers::{
    DirectoryId, IdentifierError, ItemId, MessageId, PatchId, ProjectId, RequestId, RunId,
    ThreadId, TurnId,
};
pub use model::{
    CommandReceipt, DirectoryEntry, DirectoryKind, DirectoryListing, DirectoryListingError,
    DirectoryPlace, PlaceKind, ProjectListing, ProjectListingError, ProjectSummary, QueuedMessage,
    ReceiptDisposition, ThreadListing, ThreadListingError, ThreadSummary,
};
pub use text::{
    DisplayName, DisplayNameError, MessageBody, MessageBodyError, RootPath, RootPathError,
    ThreadTitle, ThreadTitleError,
};
pub use time::UnixMillis;

pub use legacy_workspace_id::{WorkspaceId, WorkspaceIdError};
