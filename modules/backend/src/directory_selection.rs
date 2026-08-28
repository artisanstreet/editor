//! Process-owned authority for directories picked outside Forge.
//!
//! One [`SelectedDirectoryAuthority`] lives in the future Forge assembly and
//! bridges the gap between a desktop chooser's successful pick and the
//! `AttachProject` command that consumes it. The trusted backend caller mints
//! one validated [`DirectoryId`] through the existing
//! [`crate::command_admission::CommandOrigin`] boundary, hands the helper's
//! canonical [`RootPath`] to [`SelectedDirectoryAuthority::register`], and
//! later consumes the selection exactly once when its request reaches project
//! resolution.
//!
//! # Trusted canonical-input precondition
//!
//! [`SelectedDirectoryAuthority::register`] accepts whatever [`RootPath`] it
//! receives as already canonical: producing that canonical UTF-8 result is
//! the picker helper's job in the future composition. The authority performs
//! no canonicalization, no `stat`, no confinement check, no existence
//! revalidation, and no other filesystem I/O — it never touches the
//! filesystem at all. Network, mapped, and reparse-resolved locations are
//! therefore admitted exactly like local ones, and the authority grants no
//! filesystem capability and claims no exact-object identity beyond the
//! canonical path text itself.
//!
//! # Process budgets
//!
//! The owner deliberately bounds itself to [`MAX_LIVE_SELECTIONS`] live
//! entries, [`MAX_LIFETIME_ISSUED_IDENTITIES`] issued identities per
//! process, and a [`SELECTION_TIME_TO_LIVE`] of ten minutes. These are
//! initial process budgets, not claims of limitless service: lifetime
//! exhaustion is permanent for one authority instance, while live capacity
//! returns when selections are consumed or expire. An unexpired entry is
//! never evicted to admit another selection. Consumption transfers its path
//! and name out of the authority; expired payloads are removed at the next
//! successful registration or consumption. Only identity text is retained
//! permanently in the bounded issuance history.
//!
//! # Publication and consumption
//!
//! Publication is deliberately orphan-tolerant: the future flow registers
//! the pick before forming its `Selected` response, and the handler cannot
//! observe wire completion, so a send failure or lost response may leave a
//! live entry until it expires. No delivery callback, retirement signal, or
//! fabricated cancellation exists. Consumption is the single-use act of
//! handing the selection to request admission; the future handler performs
//! it only after its durable attach-replay lookup misses, and a failure
//! after consumption simply requires a fresh pick.
//!
//! # Time
//!
//! Every operation takes the caller's monotonic [`std::time::Instant`]
//! observation. The authority consults no wall clock, no origin, and no
//! asynchronous machinery, keeps expiry as a checked deadline computed once
//! at registration, and never extends a deadline when observed. An entry
//! whose deadline is reached (`now >= expiry`) expires exactly at equality.
//! Expired entries are pruned only by a successful registration and by
//! consumption; a failed admission performs no cleanup and changes nothing.
//! All operations are synchronous and return owned values, so no resource
//! borrow survives an await point.
//!
//! # Redaction
//!
//! Selected paths and display names are presentation payloads: the owner and
//! both returned values implement neither [`Debug`](std::fmt::Debug) nor
//! [`Clone`](std::clone::Clone), and the typed admission error carries no
//! payload text, so failures describe shape without ever exposing a raw path
//! or name. Consumption returns the same unknown result for never-issued,
//! consumed, and expired identities. Registration still rejects every
//! previously issued identity with a collision error.
//!
//! This foundation is not yet wired into
//! [`crate::request_handler::RequestHandler`]; `PickDirectory` stays
//! unsupported until the real helper, controller, and handler compose around
//! this owner.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, Prefix};
use std::time::{Duration, Instant};

use artisan_domain::{DirectoryId, DisplayName, DisplayNameError, RootPath};
use thiserror::Error;

/// Maximum number of simultaneously live selections one authority holds.
pub const MAX_LIVE_SELECTIONS: usize = 8;

/// Maximum number of identities one authority issues over its whole life.
///
/// Reaching this budget permanently retires the authority instance: a fresh
/// process (and therefore a fresh authority) is required to issue again.
pub const MAX_LIFETIME_ISSUED_IDENTITIES: usize = 256;

/// Time a registered selection stays live after its registration observation.
pub const SELECTION_TIME_TO_LIVE: Duration = Duration::from_secs(600);

/// Failure to admit one directory selection into the authority.
///
/// Every variant is bounded and payload-free: none of them carries or
/// renders the submitted path, the derived name, or any other selection
/// payload.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum DirectorySelectionAdmissionError {
    /// The identity is still live or already retired in this authority.
    ///
    /// Identity history is permanent, so a consumed or expired identity
    /// collides exactly like a live one and is never reminted.
    #[error("directory identity is already issued by this authority")]
    IdentityAlreadyIssued,
    /// Every live slot is occupied by an unexpired entry.
    ///
    /// Admission never evicts an unexpired selection; capacity returns only
    /// through consumption or expiry.
    #[error("every live selection slot is occupied by an unexpired entry")]
    LiveCapacityFull,
    /// The bounded lifetime issuance budget is spent.
    ///
    /// This outcome is permanent for the authority instance; only a fresh
    /// process with a fresh authority can issue further identities.
    #[error("authority exhausted its bounded lifetime issuance budget")]
    LifetimeExhausted,
    /// The checked deadline left the representable monotonic range.
    #[error("selection deadline exceeds the monotonic instant range")]
    DeadlineOverflow,
    /// The derived display name violated the existing name validation.
    #[error("derived display name is not admissible")]
    DisplayName(#[source] DisplayNameError),
    /// The root form exposes no displayable name component.
    ///
    /// Only the approved canonical shapes carry a derivable label (final
    /// component, drive-letter label, share segment, or filesystem root);
    /// anything else fails closed instead of inventing a placeholder.
    #[error("canonical root path form exposes no displayable name component")]
    UnnamedRootForm,
}

/// One live selection recorded by the authority, with its checked deadline.
struct LiveSelection {
    root_path: RootPath,
    display_name: DisplayName,
    expires_at: Instant,
}

/// The confirmed registration of one picked directory.
///
/// Returned exactly once by a successful
/// [`SelectedDirectoryAuthority::register`] so the future composition can
/// form its picker response from the issued values.
pub struct IssuedDirectory {
    /// Opaque routing identity minted by the trusted backend caller.
    pub directory_id: DirectoryId,
    /// Canonical root path exactly as supplied by the trusted helper result.
    pub root_path: RootPath,
    /// Display label derived from the canonical path without filesystem I/O.
    pub display_name: DisplayName,
}

/// The owned selection released by one successful single-use consumption.
pub struct SelectedDirectory {
    /// Opaque routing identity the request carried.
    pub directory_id: DirectoryId,
    /// Canonical root path exactly as the trusted helper reported it.
    pub root_path: RootPath,
    /// Display label derived at registration time.
    pub display_name: DisplayName,
}

/// Process-owned registry of freshly picked directories awaiting attachment.
///
/// Future Forge assembly must construct one instance shared by its request
/// admission. The owner maps each issued [`DirectoryId`] to its canonical
/// [`RootPath`], derived [`DisplayName`], and checked monotonic expiry, and
/// remembers every identity it has ever issued so no identity — live,
/// consumed, or expired — can ever be issued twice.
///
/// The type deliberately implements neither [`Clone`](std::clone::Clone) nor
/// [`Debug`](std::fmt::Debug): duplicating the owner would duplicate
/// single-use state, and automatic formatting could expose selection
/// payloads.
pub struct SelectedDirectoryAuthority {
    live: HashMap<DirectoryId, LiveSelection>,
    issued: HashSet<DirectoryId>,
}

impl Default for SelectedDirectoryAuthority {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectedDirectoryAuthority {
    /// Creates an empty authority holding no selections and no history.
    #[must_use]
    pub fn new() -> Self {
        Self {
            live: HashMap::new(),
            issued: HashSet::new(),
        }
    }

    /// Records one freshly picked directory under a caller-minted identity.
    ///
    /// `root_path` must already be the trusted helper's successful canonical
    /// UTF-8 result; this method validates no filesystem fact and derives the
    /// [`DisplayName`] purely from the path text. `now` is the caller's
    /// monotonic observation and fixes the selection's deadline one
    /// [`SELECTION_TIME_TO_LIVE`] after it.
    ///
    /// Every admission prerequisite — identity novelty, derivable name,
    /// lifetime issuance budget, live capacity counted over unexpired
    /// entries without removing them, and representable deadline arithmetic —
    /// is validated before any state changes. Only a fully admitted
    /// registration mutates the authority: pruning expired entries and
    /// inserting the new selection happen together on success. A rejected
    /// admission therefore burns neither the supplied identity nor any other
    /// budget and leaves the live map and issued history untouched.
    ///
    /// # Errors
    ///
    /// Returns [`DirectorySelectionAdmissionError::IdentityAlreadyIssued`]
    /// when the identity was ever issued before,
    /// [`DirectorySelectionAdmissionError::DisplayName`] when the derived
    /// name is blank after trimming or exceeds 256 UTF-8 bytes,
    /// [`DirectorySelectionAdmissionError::UnnamedRootForm`] when the
    /// canonical root shape carries no displayable label,
    /// [`DirectorySelectionAdmissionError::LiveCapacityFull`] or
    /// [`DirectorySelectionAdmissionError::LifetimeExhausted`] when the
    /// respective budget is spent, and
    /// [`DirectorySelectionAdmissionError::DeadlineOverflow`] when the
    /// deadline leaves the monotonic instant range.
    pub fn register(
        &mut self,
        directory_id: DirectoryId,
        root_path: RootPath,
        now: Instant,
    ) -> Result<IssuedDirectory, DirectorySelectionAdmissionError> {
        if self.issued.contains(&directory_id) {
            return Err(DirectorySelectionAdmissionError::IdentityAlreadyIssued);
        }

        let display_name = derive_display_name(&root_path)?;

        if self.issued.len() >= MAX_LIFETIME_ISSUED_IDENTITIES {
            return Err(DirectorySelectionAdmissionError::LifetimeExhausted);
        }
        // Counted read-only: expired entries still occupying slots do not
        // block admission, and nothing is removed before every prerequisite
        // has passed.
        let live_unexpired = self
            .live
            .values()
            .filter(|selection| now < selection.expires_at)
            .count();
        if live_unexpired >= MAX_LIVE_SELECTIONS {
            return Err(DirectorySelectionAdmissionError::LiveCapacityFull);
        }

        let expires_at = now
            .checked_add(SELECTION_TIME_TO_LIVE)
            .ok_or(DirectorySelectionAdmissionError::DeadlineOverflow)?;

        // The single successful-admission mutation: prune expired entries
        // and insert the new selection together.
        self.prune_expired(now);
        self.live.insert(
            directory_id.clone(),
            LiveSelection {
                root_path: root_path.clone(),
                display_name: display_name.clone(),
                expires_at,
            },
        );
        self.issued.insert(directory_id.clone());

        Ok(IssuedDirectory {
            directory_id,
            root_path,
            display_name,
        })
    }

    /// Consumes the live selection named by `directory_id` exactly once.
    ///
    /// `now` is the caller's monotonic observation; expired entries are
    /// pruned first, and observing never extends a deadline. A successful
    /// lookup removes the entry and transfers its path and name to the
    /// returned selection for the request's future validation and database
    /// work. The identity itself stays retired in the authority's history
    /// and can never be issued again.
    ///
    /// Unknown answers are uniform by design: a never-registered, already
    /// consumed, and expired identity are indistinguishable, matching the
    /// identity's role as opaque routing data rather than a credential.
    #[must_use]
    pub fn consume(
        &mut self,
        directory_id: &DirectoryId,
        now: Instant,
    ) -> Option<SelectedDirectory> {
        self.prune_expired(now);
        let removed = self.live.remove(directory_id)?;
        Some(SelectedDirectory {
            directory_id: directory_id.clone(),
            root_path: removed.root_path,
            display_name: removed.display_name,
        })
    }

    /// Drops every live entry whose deadline is reached at `now`.
    ///
    /// Equality expires: an entry observed exactly at its deadline is
    /// treated as expired. Only the live map is touched; the issued-identity
    /// history is never pruned. Called only on the two mutating paths —
    /// after every registration prerequisite has passed, and during
    /// consumption — so refused admissions never clean up or otherwise
    /// change state.
    fn prune_expired(&mut self, now: Instant) {
        self.live
            .retain(|_directory_id, selection| now < selection.expires_at);
    }
}

/// Derives the display label from canonical path text without filesystem I/O.
///
/// The label is the canonical path's last component. Root-shaped paths
/// answer from their structure instead: a Windows drive root (plain or
/// verbatim) uses the path's original drive-letter text such as `c:` or
/// `C:` — the exact spelling written in the UTF-8 path, lowercase included,
/// never rebuilt or case-folded — a UNC share root (plain or verbatim) uses
/// its share segment as originally spelled, and a Unix filesystem root uses
/// `/`. Text, case, spacing, and Unicode content are preserved verbatim;
/// nothing is trimmed, truncated, or converted lossily.
///
/// # Errors
///
/// Returns [`DirectorySelectionAdmissionError::DisplayName`] when the
/// derived label fails the existing display-name validation, and
/// [`DirectorySelectionAdmissionError::UnnamedRootForm`] when the path shape
/// offers no derivable label at all.
fn derive_display_name(
    root_path: &RootPath,
) -> Result<DisplayName, DirectorySelectionAdmissionError> {
    let path = Path::new(root_path.as_str());

    let label = match path.file_name() {
        Some(final_component) => component_text(final_component).to_owned(),
        None => match root_form_label(path) {
            Some(root_label) => root_label,
            None => return Err(DirectorySelectionAdmissionError::UnnamedRootForm),
        },
    };

    DisplayName::parse(label).map_err(DirectorySelectionAdmissionError::DisplayName)
}

/// Labels a root-shaped canonical path from its prefix or filesystem root.
///
/// Returns [`None`] for shapes outside the approved canonical forms — device
/// namespaces, generic verbatim prefixes, and relative remnants — because
/// inventing a placeholder label would misrepresent the selection.
fn root_form_label(path: &Path) -> Option<String> {
    let first_component = path.components().next()?;

    match first_component {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(_) | Prefix::VerbatimDisk(_) => {
                drive_label_from_original(prefix.as_os_str())
            }
            Prefix::UNC(_server, share) | Prefix::VerbatimUNC(_server, share) => {
                Some(component_text(share).to_owned())
            }
            Prefix::Verbatim(_) | Prefix::DeviceNS(_) => None,
        },
        Component::RootDir => Some(String::from("/")),
        Component::CurDir | Component::Normal(_) | Component::ParentDir => None,
    }
}

/// The canonical verbatim wrapper around extended-length path forms.
const VERBATIM_PREFIX: &str = r"\\?\";

/// Slices the drive-letter label from the ORIGINAL UTF-8 prefix text.
///
/// [`std::path::PrefixComponent::as_os_str`] yields the prefix exactly as
/// written in the path — `c:` for a classic drive root, `\\?\c:` for a verbatim one.
/// Stripping only a literal `\\?\` wrapper and requiring the remaining body
/// to be one original ASCII letter followed by `:` keeps lowercase, mixed,
/// and uppercase spellings byte-identical; rebuilding from the parsed
/// [`Prefix`] byte could not make that guarantee. Anything outside that
/// exact label shape returns [`None`], failing closed instead of guessing.
fn drive_label_from_original(prefix_text: &OsStr) -> Option<String> {
    let text = component_text(prefix_text);
    let body = text.strip_prefix(VERBATIM_PREFIX).unwrap_or(text);

    let mut characters = body.chars();
    match (characters.next(), characters.next(), characters.next()) {
        (Some(letter), Some(':'), None) if letter.is_ascii_alphabetic() => Some(body.to_owned()),
        _ => None,
    }
}

/// Returns one path component as text.
///
/// [`RootPath`] carries validated UTF-8 text and path components slice that
/// text only at ASCII separators, so every component of an admitted value
/// converts back to [`str`]; this conversion cannot fail.
fn component_text(component: &OsStr) -> &str {
    component
        .to_str()
        .expect("components of validated UTF-8 RootPath text stay valid UTF-8")
}
