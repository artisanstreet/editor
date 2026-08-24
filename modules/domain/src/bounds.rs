//! Documented bounds for every external value carried by the domain.
//!
//! Every string bound is measured in UTF-8 bytes. The legacy TypeScript
//! product validated most lengths in UTF-16 code units; the native protocol
//! deliberately re-specifies each ceiling in bytes so a client and Forge can
//! never disagree about whether a given payload fits. Each constant records
//! its provenance and why the chosen size is appropriate.

/// Shared UTF-8 byte ceiling for every wire-facing identifier.
///
/// This single bound applies to `RequestId`, `DirectoryId`, `ProjectId`,
/// `ThreadId`, and `MessageId`. Legacy identifiers were non-empty and
/// whitespace-free but unbounded (`Identifier` in
/// `modules/protocol/src/common.ts`); 128 bytes matches the strictest existing
/// identifier-scale bound (single directory segment names are capped at 128
/// bytes by `modules/backend/src/projects/project-directory-service.ts`) while
/// leaving generous room for opaque Forge-minted forms such as prefixed hex,
/// snowflakes, and UUIDs.
pub const IDENTIFIER_MAX_BYTES: usize = 128;

/// Maximum UTF-8 byte length of a thread title.
///
/// Legacy titles were non-empty after trimming but unbounded
/// (`modules/frontend/src/lib/root/draft-thread.ts` seeds `"New thread"`).
/// An unbounded label is hostile to a bounded wire frame, so the native
/// protocol adopts this deliberate ceiling: long enough for a descriptive
/// opening line, short enough that titles remain sidebar labels.
pub const THREAD_TITLE_MAX_BYTES: usize = 256;

/// Maximum UTF-8 byte length of a Forge-supplied display name for projects
/// and directories.
///
/// Legacy display names were merely non-empty strings served by Forge
/// (`modules/protocol/src/thread.ts`,
/// `modules/protocol/src/project-directory.ts`). They are short UI labels by
/// construction, so the native bound simply makes that property explicit.
pub const DISPLAY_NAME_MAX_BYTES: usize = 256;

/// Maximum UTF-8 byte length of a project root path description.
///
/// The root path travels as an opaque, verbatim host-path description for
/// display and reference only; containment and canonicalization stay in
/// backend services. 32,768 bytes is the explicit, generous finite
/// application-protocol byte ceiling chosen to accommodate extended-length
/// host paths. It is not derived from an operating-system limit: Windows
/// measures its relevant extended-length limit in UTF-16 code units, not
/// UTF-8 bytes, so this bound claims neither unit equivalence with that limit
/// nor that every host-representable path fits.
pub const ROOT_PATH_MAX_BYTES: usize = 32_768;

/// Maximum UTF-8 byte length of a first-message body.
///
/// Preserves the legacy conversation body ceiling of 65,536 established by
/// `conversation_body_text_limit` in `modules/protocol/src/conversation.ts`:
/// a reply truncated mid-sentence reads as answered, so the stored body bound
/// stays generous while remaining finite.
pub const MESSAGE_BODY_MAX_BYTES: usize = 65_536;

/// Maximum number of directory entries in one listing.
///
/// Matches `maximum_directories` in
/// `modules/backend/src/projects/project-directory-service.ts`.
pub const DIRECTORY_LISTING_MAX_ENTRIES: usize = 256;

/// Maximum number of well-known shortcut places in one listing.
///
/// Matches the legacy place cap of 16 in
/// `modules/protocol/src/project-directory.ts`; only seven standard user
/// folders exist today, so 16 leaves headroom without allowing unbounded
/// growth.
pub const DIRECTORY_LISTING_MAX_PLACES: usize = 16;

/// Maximum number of summaries in one project-scoped thread listing.
///
/// Deliberate improvement over legacy, whose thread list arrays were
/// unbounded. 256 aligns with the collection caps the legacy service applies
/// elsewhere and keeps list responses bounded end to end.
pub const THREAD_LISTING_MAX_THREADS: usize = 256;

/// Maximum UTF-8 byte length of one incremental renderer text fragment.
///
/// Preserves `ConversationText`'s 4,096-unit legacy ceiling while making the
/// native unit unambiguous. Full stored messages use
/// [`MESSAGE_BODY_MAX_BYTES`]; this smaller bound applies only to one streamed
/// append patch and permits the empty fragment used to open a stream.
pub const CONVERSATION_TEXT_FRAGMENT_MAX_BYTES: usize = 4_096;

/// Maximum patches returned by one replay read.
///
/// Matches `conversation_patch_replay_batch_size` in the legacy conversation
/// projection. A caller advances its cursor and asks again when more patches
/// remain.
pub const CONVERSATION_PATCH_BATCH_MAX_PATCHES: usize = 64;

/// Maximum turns returned by one bounded conversation query.
///
/// Matches `conversation_query_maximum_turn_count` in the legacy protocol.
pub const CONVERSATION_QUERY_MAX_TURNS: u16 = 512;

/// Maximum number of summaries in one attached-project listing.
///
/// Deliberate improvement over legacy, whose project catalog array was
/// unbounded (`ProjectCatalogSnapshot.projects` in
/// `modules/protocol/src/project.ts`). 256 aligns with
/// [`DIRECTORY_LISTING_MAX_ENTRIES`] and [`THREAD_LISTING_MAX_THREADS`] and
/// keeps the rediscovery response bounded end to end.
pub const PROJECT_LISTING_MAX_PROJECTS: usize = 256;
