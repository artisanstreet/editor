//! State values projected from Forge for the first native workflow.
//!
//! These values are the durable and listed state the workflow exchanges:
//! directory browsing, the attached-project catalog, project-scoped thread
//! summaries, command receipts, and the durably queued first message. They are
//! plain owned data: no engine, run, or provider concepts appear because
//! engine dispatch is explicitly outside this milestone.

use std::collections::HashSet;

use thiserror::Error;

use crate::bounds::{
    DIRECTORY_LISTING_MAX_ENTRIES, DIRECTORY_LISTING_MAX_PLACES, PROJECT_LISTING_MAX_PROJECTS,
    THREAD_LISTING_MAX_THREADS,
};
use crate::identifiers::{DirectoryId, MessageId, ProjectId, RequestId, ThreadId};
use crate::text::{DisplayName, MessageBody, RootPath, ThreadTitle};
use crate::time::UnixMillis;

/// Position of a directory within Forge's browsing model.
///
/// Legacy literals: `root | directory` in
/// `modules/protocol/src/project-directory.ts`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DirectoryKind {
    /// A top-level allowed root offered by Forge.
    Root,
    /// A directory nested below an allowed root.
    Directory,
}

/// A well-known user folder offered as a one-click shortcut.
///
/// Mirrors the seven legacy shortcut places in presentation order
/// (`modules/backend/src/projects/project-directory-service.ts`).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PlaceKind {
    /// The user's home directory.
    Home,
    /// Standard desktop folder.
    Desktop,
    /// Standard documents folder.
    Documents,
    /// Standard downloads folder.
    Downloads,
    /// Standard music folder.
    Music,
    /// Standard pictures folder.
    Pictures,
    /// Standard videos folder.
    Videos,
}

/// One bounded directory entry safe to expose to clients.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DirectoryEntry {
    /// Opaque identity Forge minted for this directory.
    pub directory_id: DirectoryId,
    /// Display label served by Forge; never derived from host path data.
    pub display_name: DisplayName,
    /// Whether this entry is an allowed root or a nested directory.
    pub kind: DirectoryKind,
    /// Whether the directory is known to contain child directories.
    pub has_children: bool,
}

/// One well-known shortcut place with its display label.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DirectoryPlace {
    /// Opaque identity of the place's directory.
    pub directory_id: DirectoryId,
    /// Display label served by Forge.
    pub display_name: DisplayName,
    /// Which standard user folder this place represents.
    pub kind: PlaceKind,
}

/// Failure raised when a listing would exceed its documented bounds.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum DirectoryListingError {
    /// Too many shortcut places were supplied.
    #[error("directory listing holds {count} places; the maximum is {maximum}")]
    TooManyPlaces {
        /// Offending number of places.
        count: usize,
        /// Documented ceiling ([`DIRECTORY_LISTING_MAX_PLACES`]).
        maximum: usize,
    },
    /// Too many directory entries were supplied.
    #[error("directory listing holds {count} entries; the maximum is {maximum}")]
    TooManyEntries {
        /// Offending number of entries.
        count: usize,
        /// Documented ceiling ([`DIRECTORY_LISTING_MAX_ENTRIES`]).
        maximum: usize,
    },
}

/// One bounded directory listing: shortcut places plus browsable entries.
///
/// Plain file names from the legacy listing shape are deliberately omitted:
/// they existed as picker context for a selectable file-chooser surface, and
/// the attach-by-opaque-id workflow never consumes them. Restoring them later
/// would carry their 256-entry bound from the same legacy evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryListing {
    places: Vec<DirectoryPlace>,
    entries: Vec<DirectoryEntry>,
    parent: Option<DirectoryId>,
}

impl DirectoryListing {
    /// Builds a listing after enforcing the documented collection bounds.
    ///
    /// # Errors
    ///
    /// Returns [`DirectoryListingError::TooManyPlaces`] when more than
    /// [`DIRECTORY_LISTING_MAX_PLACES`] places are supplied and
    /// [`DirectoryListingError::TooManyEntries`] when more than
    /// [`DIRECTORY_LISTING_MAX_ENTRIES`] entries are supplied.
    pub fn new(
        places: Vec<DirectoryPlace>,
        entries: Vec<DirectoryEntry>,
        parent: Option<DirectoryId>,
    ) -> Result<Self, DirectoryListingError> {
        let places_count = places.len();
        if places_count > DIRECTORY_LISTING_MAX_PLACES {
            return Err(DirectoryListingError::TooManyPlaces {
                count: places_count,
                maximum: DIRECTORY_LISTING_MAX_PLACES,
            });
        }

        let entries_count = entries.len();
        if entries_count > DIRECTORY_LISTING_MAX_ENTRIES {
            return Err(DirectoryListingError::TooManyEntries {
                count: entries_count,
                maximum: DIRECTORY_LISTING_MAX_ENTRIES,
            });
        }

        Ok(Self {
            places,
            entries,
            parent,
        })
    }

    /// Returns the bounded shortcut places.
    #[must_use]
    pub fn places(&self) -> &[DirectoryPlace] {
        &self.places
    }

    /// Returns the bounded directory entries.
    #[must_use]
    pub fn entries(&self) -> &[DirectoryEntry] {
        &self.entries
    }

    /// Returns the directory whose contents are listed, if not a root view.
    #[must_use]
    pub fn parent(&self) -> Option<&DirectoryId> {
        self.parent.as_ref()
    }
}

/// One attached project as Forge projects it to clients.
///
/// Minimal subset of the legacy catalog row
/// (`modules/protocol/src/project.ts`): identity, labels, the root
/// description, and the attachment instant as signed Unix epoch milliseconds,
/// matching the native schema's `attachedAtMillis` field.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProjectSummary {
    /// Forge-minted project identity, stable across detach and re-attach.
    pub project_id: ProjectId,
    /// Display label served by Forge.
    pub display_name: DisplayName,
    /// Verbatim root path description; opaque in the domain.
    pub root_path: RootPath,
    /// Moment the project was attached, as signed Unix epoch millis.
    pub attached_at: UnixMillis,
}

/// Failure raised when a project listing would violate its documented bounds.
///
/// Variants carry Forge-minted identities, so this error is deliberately
/// not [`Copy`] and never renders the offending row's payload.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ProjectListingError {
    /// Too many project summaries were supplied.
    #[error("project listing holds {count} projects; the maximum is {maximum}")]
    TooManyProjects {
        /// Offending number of projects.
        count: usize,
        /// Documented ceiling ([`PROJECT_LISTING_MAX_PROJECTS`]).
        maximum: usize,
    },
    /// Two summaries reused the same Forge-minted project identity.
    #[error("project listing contains duplicate project id {project_id}")]
    DuplicateProject {
        /// Reused identity.
        project_id: ProjectId,
    },
}

/// One bounded listing of every currently attached project.
///
/// This is the rediscovery read of the milestone: a returning client asks
/// once and receives the complete attached-project catalog in Forge-supplied
/// order, never an unbounded array. Each row projects one durable
/// `attached_projects` primary key, so a listing names each project identity
/// at most once; duplicates can only arise from corrupt or hostile input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectListing(Vec<ProjectSummary>);

impl ProjectListing {
    /// Builds a listing after enforcing the documented collection bound and
    /// identity uniqueness.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectListingError::TooManyProjects`] when more than
    /// [`PROJECT_LISTING_MAX_PROJECTS`] summaries are supplied, and
    /// [`ProjectListingError::DuplicateProject`] naming the first repeated
    /// identity when two summaries reuse one project id.
    pub fn new(projects: Vec<ProjectSummary>) -> Result<Self, ProjectListingError> {
        let count = projects.len();
        if count > PROJECT_LISTING_MAX_PROJECTS {
            return Err(ProjectListingError::TooManyProjects {
                count,
                maximum: PROJECT_LISTING_MAX_PROJECTS,
            });
        }

        let mut project_ids = HashSet::with_capacity(count);
        for project in &projects {
            if !project_ids.insert(project.project_id.clone()) {
                return Err(ProjectListingError::DuplicateProject {
                    project_id: project.project_id.clone(),
                });
            }
        }

        Ok(Self(projects))
    }

    /// Returns the bounded attached-project summaries.
    #[must_use]
    pub fn projects(&self) -> &[ProjectSummary] {
        &self.0
    }
}

/// One project-scoped thread as the list surfaces need it.
///
/// Deliberate subset of the legacy projection (`ThreadListItem` in
/// `modules/protocol/src/thread.ts`): live statuses, attention and reader
/// cursors, affinity scores, rename/rehome suggestions, pins, and archive
/// state all belong to later slices and are cut explicitly here rather than
/// silently. Creation and update instants travel as signed Unix epoch
/// milliseconds per the native schema's `createdAtMillis` and
/// `updatedAtMillis` fields; ordering between them is not enforced here.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ThreadSummary {
    /// Forge-minted thread identity.
    pub thread_id: ThreadId,
    /// Owning project identity.
    pub project_id: ProjectId,
    /// Current validated title.
    pub title: ThreadTitle,
    /// Moment the thread was created, as signed Unix epoch millis.
    pub created_at: UnixMillis,
    /// Moment the projection last changed, as signed Unix epoch millis.
    pub updated_at: UnixMillis,
}

/// Failure raised when a thread listing would violate its documented bounds.
///
/// Variants carry Forge-minted identities, so this error is deliberately
/// not [`Copy`] and never renders the offending row's payload.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ThreadListingError {
    /// Too many thread summaries were supplied.
    #[error("thread listing holds {count} threads; the maximum is {maximum}")]
    TooManyThreads {
        /// Offending number of threads.
        count: usize,
        /// Documented ceiling ([`THREAD_LISTING_MAX_THREADS`]).
        maximum: usize,
    },
    /// Two summaries reused the same Forge-minted thread identity.
    #[error("thread listing contains duplicate thread id {thread_id}")]
    DuplicateThread {
        /// Reused identity.
        thread_id: ThreadId,
    },
}

/// One bounded, project-scoped thread listing.
///
/// Each row projects one durable `threads` primary key, so a listing names
/// each thread identity at most once; duplicates can only arise from corrupt
/// or hostile input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadListing(Vec<ThreadSummary>);

impl ThreadListing {
    /// Builds a listing after enforcing the documented collection bound and
    /// identity uniqueness.
    ///
    /// # Errors
    ///
    /// Returns [`ThreadListingError::TooManyThreads`] when more than
    /// [`THREAD_LISTING_MAX_THREADS`] summaries are supplied, and
    /// [`ThreadListingError::DuplicateThread`] naming the first repeated
    /// identity when two summaries reuse one thread id.
    pub fn new(threads: Vec<ThreadSummary>) -> Result<Self, ThreadListingError> {
        let count = threads.len();
        if count > THREAD_LISTING_MAX_THREADS {
            return Err(ThreadListingError::TooManyThreads {
                count,
                maximum: THREAD_LISTING_MAX_THREADS,
            });
        }

        let mut thread_ids = HashSet::with_capacity(count);
        for thread in &threads {
            if !thread_ids.insert(thread.thread_id.clone()) {
                return Err(ThreadListingError::DuplicateThread {
                    thread_id: thread.thread_id.clone(),
                });
            }
        }

        Ok(Self(threads))
    }

    /// Returns the bounded thread summaries.
    #[must_use]
    pub fn threads(&self) -> &[ThreadSummary] {
        &self.0
    }
}

/// How Forge dispositioned one mutation identified by its request id.
///
/// Exactly the two legacy outcomes survive into the native domain: byte-exact
/// replays answer `Duplicate` with no second effect, everything else that
/// commits answers `Accepted` (`modules/backend/src/persistence/
/// journal-store.ts`). Rejections are typed errors instead of receipts.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReceiptDisposition {
    /// The mutation committed durably for the first time.
    Accepted,
    /// A previous request with the identical id already committed; no second
    /// effect was applied.
    Duplicate,
}

/// Durable receipt returned for every accepted mutation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CommandReceipt {
    /// Client-minted request identity the receipt answers.
    pub request_id: RequestId,
    /// Whether the request newly committed or replayed an earlier commit.
    pub disposition: ReceiptDisposition,
}

/// A first message durably queued on a thread.
///
/// This is the explicit queued state of the milestone: the message exists
/// durably behind its receipt, and dispatch is intentionally absent. No run,
/// engine, or provider field appears because none belongs to this contract.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QueuedMessage {
    /// Forge-minted message identity assigned at acceptance.
    pub message_id: MessageId,
    /// Thread the message is queued on.
    pub thread_id: ThreadId,
    /// Client request identity whose acceptance produced this message.
    pub request_id: RequestId,
    /// Validated body exactly as accepted.
    pub body: MessageBody,
}
