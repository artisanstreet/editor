//! Forge's internal directory helper mode and its blocking path work.
//!
//! Invoked as exactly `--internal-directory-helper-v1`, the ordinary Forge
//! executable becomes one short-lived private helper: it reads exactly one
//! bounded request frame from standard input (see
//! [`crate::directory_helper_codec`]), performs the requested chooser or
//! validation work entirely inside this process, writes exactly one bounded
//! response frame to standard output, and exits. Dispatch happens in
//! [`run_if_requested`] before any normal backend startup, storage, network,
//! or UI code can run, and normal Forge behavior is untouched when the flag
//! is absent.
//!
//! This module owns all blocking operating-system work: the native chooser,
//! real `std` canonicalization, and directory metadata. It deliberately does
//! not own the parent side of the conversation—the exact child handle, the
//! sole stdin writer, generation assignment, deadlines, cancellation, kill,
//! and reap belong to a separate controller. The helper's own defense is the
//! stdin lifeline: once the single request has been read, a watcher thread
//! treats any completed stdin read—end of stream, read error, or even one
//! additional byte—as the parent losing interest and ends this process with
//! [`EXIT_CODE_LIFELINE_LOST`]. A read completes whenever the operating
//! system delivers it; how long the watcher waits is unbounded here, because
//! deadline enforcement belongs to the future parent controller. The parent
//! must keep its writer end open for the whole operation.
//!
//! Discipline enforced here: nothing is ever written to standard error, no
//! log line exists, and no filesystem text travels anywhere except inside
//! the private pipe payload. Diagnostics would leak user paths and imply a
//! reporting contract that belongs to the parent, so every abnormal ending
//! is a silent fixed nonzero exit code instead.

use std::convert::Infallible;
use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, ExitCode};
use std::thread;

use artisan_domain::RootPath;
use thiserror::Error;

use crate::directory_helper_codec::{
    HelperRequest, RequestReadFault, Response, encode_response, read_request,
};

// Contract-named test sources live under tests/backend and compile as
// private modules of this crate through the supplied backend-crate rust_test
// wiring (`rust_test` with `crate` and no srcs). The unit module carries the
// private chooser/operation white-box coverage; the framing module carries
// the v1 codec coverage; the fixture module carries the headless real-child
// lifeline coverage. Paths here resolve relative to this source file.
#[cfg(test)]
#[path = "../../../tests/backend/directory_helper_unit.rs"]
mod directory_helper_unit;

#[cfg(test)]
#[path = "../../../tests/backend/directory_helper.rs"]
mod directory_helper_framing;

#[cfg(test)]
#[path = "../../../tests/backend/directory_helper_fixture.rs"]
mod directory_helper_fixture;

/// The one command-line flag that selects helper mode.
///
/// The invocation must consist of exactly this argument; any additional or
/// duplicated argument refuses the helper role rather than guessing.
pub(crate) const INTERNAL_DIRECTORY_HELPER_FLAG: &str = "--internal-directory-helper-v1";

/// Fixed exit code: the request frame was malformed, so nothing was answered.
///
/// No diagnostic accompanies this exit because framing failures occur before
/// any content can be trusted, and diagnostics here could be coerced into
/// carrying filesystem text. Rejected helper-flag invocations share this
/// code.
pub(crate) const EXIT_CODE_MALFORMED_REQUEST: u8 = 2;

/// Fixed exit code: the stdin lifeline observed the parent leaving.
///
/// End of stream, a read error, or any byte beyond the single request ends
/// the helper with this code once the watcher's read completes. When that
/// completion happens is governed by operating-system scheduling and the
/// parent's behavior; this code claims no timing bound of its own.
pub(crate) const EXIT_CODE_LIFELINE_LOST: u8 = 3;

/// Fixed exit code: the lifeline watcher could not be created, so the
/// helper refused to start any chooser or validation work.
///
/// Failing closed keeps "no watcher" from becoming "no cancellation".
pub(crate) const EXIT_CODE_LIFELINE_UNAVAILABLE: u8 = 4;

/// Fixed exit code: the bounded response frame could not be produced or
/// delivered on standard output.
///
/// Both refusal causes land here silently—an encoder fault means an
/// out-of-contract response was refused, an I/O failure means the parent
/// could not receive either way—and neither may leak filesystem text.
pub(crate) const EXIT_CODE_RESPONSE_UNDELIVERABLE: u8 = 5;

/// What should happen after inspecting this process's command-line arguments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArgumentDispatch {
    /// The helper flag is absent; normal Forge startup proceeds unchanged.
    NotRequested,
    /// The sole argument is the helper flag; run the helper protocol.
    RunHelper,
    /// The helper flag appeared alongside other arguments; refuse silently
    /// rather than interpret an ambiguous invocation.
    RejectInvocation,
}

/// Classifies a command-line argument tail for helper dispatch.
///
/// Byte-exact matching against [`INTERNAL_DIRECTORY_HELPER_FLAG`]; arguments
/// that are not valid Unicode simply do not match, they never abort the
/// process.
#[must_use]
pub(crate) fn classify_arguments<I>(arguments: I) -> ArgumentDispatch
where
    I: IntoIterator,
    I::Item: AsRef<OsStr>,
{
    let flag_bytes = INTERNAL_DIRECTORY_HELPER_FLAG.as_bytes();
    let mut total = 0_usize;
    let mut occurrences = 0_usize;
    for argument in arguments {
        total += 1;
        if argument.as_ref().as_encoded_bytes() == flag_bytes {
            occurrences += 1;
        }
    }

    if occurrences == 0 {
        ArgumentDispatch::NotRequested
    } else if total == 1 && occurrences == 1 {
        ArgumentDispatch::RunHelper
    } else {
        ArgumentDispatch::RejectInvocation
    }
}

/// Entry point consulted by `main` before any normal startup work.
///
/// Returns the helper's exit code when this process was asked to act as the
/// internal directory helper, including rejected invocations of the helper
/// flag. Returns `None` when the flag is absent so `main` can proceed with
/// ordinary Forge startup unchanged.
#[must_use]
pub fn run_if_requested() -> Option<ExitCode> {
    match classify_arguments(std::env::args_os().skip(1)) {
        ArgumentDispatch::NotRequested => None,
        ArgumentDispatch::RunHelper => Some(run_helper_process()),
        ArgumentDispatch::RejectInvocation => Some(ExitCode::from(EXIT_CODE_MALFORMED_REQUEST)),
    }
}

/// Runs one complete helper conversation on the inherited streams.
///
/// Order is contractual for every valid complete request—including the typed
/// non-UTF-8 payload failure: exactly one request frame first, then the
/// stdin lifeline watcher, then—only then—the blocking chooser or validation
/// work (or the typed encoding outcome), then one flushed response frame and
/// a normal exit. Malformed frames exit silently before any watcher exists
/// and never echo a generation. The watcher's lifetime ends with this
/// process, which is precisely when the helper's work ends.
fn run_helper_process() -> ExitCode {
    let read_outcome = {
        let mut stdin = io::stdin().lock();
        read_request(&mut stdin)
    };
    // Classify before any work starts. A well-framed Validate whose payload
    // was not lossless UTF-8 carries a trusted correlation header, so it is
    // a typed operation outcome that still earns the watcher-first ordering
    // and a real reply; malformed framing earns neither.
    let (generation, request) = match read_outcome {
        Ok((generation, request)) => (generation, Some(request)),
        Err(RequestReadFault::PayloadNotUtf8 { generation }) => (generation, None),
        Err(RequestReadFault::Malformed) => {
            return ExitCode::from(EXIT_CODE_MALFORMED_REQUEST);
        }
    };

    // The watcher starts before any dialog or filesystem work: from here on,
    // the parent losing its writer end ends this process at the watcher's
    // next read observation.
    if spawn_lifeline_watcher().is_err() {
        return ExitCode::from(EXIT_CODE_LIFELINE_UNAVAILABLE);
    }

    let response = match request.as_ref() {
        Some(request) => perform_operation(request, &NativeDirectoryChooser),
        None => Response::UnsupportedEncoding,
    };
    respond_and_exit(generation, &response)
}

/// Writes and flushes exactly one bounded response frame.
fn respond_and_exit(generation: u64, response: &Response) -> ExitCode {
    let Ok(frame) = encode_response(generation, response) else {
        return ExitCode::from(EXIT_CODE_RESPONSE_UNDELIVERABLE);
    };
    let mut stdout = io::stdout().lock();
    let delivered = stdout.write_all(&frame).and_then(|()| stdout.flush());
    match delivered {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(EXIT_CODE_RESPONSE_UNDELIVERABLE),
    }
}

/// The lifeline watcher's entire body: block on one read from `input`, then
/// end this process with [`EXIT_CODE_LIFELINE_LOST`].
///
/// Deliberately private and generic so the production closure below supplies
/// real standard input while the in-crate headless fixture can drive this
/// exact body with its own reader—no public or crate-public testing API, no
/// runtime flag, no environment hook. Every way the single read can complete
/// is a breach: zero bytes means the parent closed its sole writer, an error
/// means the pipe failed, and one-or-more bytes means input arrived beyond
/// the single framed request. The read may block for an unbounded time and
/// carries no timing promise; termination follows whenever the operating
/// system completes it. The result is discarded because every completion
/// path is a deliberate breach whose detail is reported nowhere.
fn watch_input_lifeline<R: Read>(mut input: R) -> Infallible {
    let mut breach = [0_u8; 1];
    let _breach_outcome = input.read(&mut breach);
    process::exit(i32::from(EXIT_CODE_LIFELINE_LOST));
}

/// Spawns the helper-local stdin lifeline watcher around real stdin.
///
/// The spawned closure delegates to [`watch_input_lifeline`], the extracted
/// production body, with no wrapper and no marker. The watcher makes no
/// timing promise and enforces no deadline, both of which belong to the
/// future parent controller; a still-blocking read is the healthy case and
/// the watcher stays parked until the process exits normally first.
///
/// # Errors
///
/// Returns the join-handle error when the watcher thread cannot be created;
/// callers must fail closed before starting chooser or validation work.
fn spawn_lifeline_watcher() -> io::Result<thread::JoinHandle<Infallible>> {
    thread::Builder::new()
        .name("directory-helper-lifeline".to_owned())
        .spawn(|| watch_input_lifeline(io::stdin()))
}

/// Why a candidate directory could not serve as a project root.
///
/// Variants carry no filesystem text by construction; the parent learns only
/// the classification, never the offending string.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum ResolutionFailure {
    /// Not absolute, an unsupported Windows prefix on input or on the
    /// canonical result, nonexistent, not a directory, blank after domain
    /// validation, or beyond the shared root-path byte ceiling.
    #[error("candidate path cannot serve as a project root")]
    InvalidPath,
    /// The canonical path could not cross the boundary losslessly as UTF-8.
    #[error("canonical path is not representable as lossless utf-8")]
    UnsupportedEncoding,
}

/// Validates one candidate path with real canonicalization and metadata.
///
/// The candidate must already be absolute filesystem text. On Windows the
/// leading prefix must be `Disk`, `VerbatimDisk`, `UNC`, or `VerbatimUNC`;
/// device-namespace and verbatim-relative prefixes are refused before any
/// filesystem access. Because resolution can change the leading form, the
/// same prefix policy is applied again to the canonical result. The path is
/// resolved with `std::fs::canonicalize` (which resolves symlinks and
/// reparse points, so explicitly selected network or mapped targets are
/// permitted), confirmed to name a directory through real metadata, and
/// converted without lossy substitution. The unchanged domain
/// [`RootPath`] validation is the final text boundary: it refuses blank or
/// over-length text while preserving exact bytes. Text is never trimmed,
/// case-folded, enumerated past, promoted to a repository root, or otherwise
/// normalized.
///
/// Operating-system errors are intentionally discarded rather than wrapped:
/// their display forms can embed the candidate path, and typed outcomes are
/// this boundary's entire vocabulary.
///
/// # Errors
///
/// Returns [`ResolutionFailure`] as classified above. The byte ceiling is
/// the existing [`artisan_domain::ROOT_PATH_MAX_BYTES`], enforced through
/// the unchanged domain type.
pub(crate) fn resolve_selected_directory(candidate: &str) -> Result<String, ResolutionFailure> {
    let path = Path::new(candidate);
    if !path.is_absolute() {
        return Err(ResolutionFailure::InvalidPath);
    }
    #[cfg(target_os = "windows")]
    if !has_allowed_windows_prefix(path) {
        return Err(ResolutionFailure::InvalidPath);
    }

    // Real resolution; symlink and reparse targets are legitimate results.
    let canonical = std::fs::canonicalize(path).map_err(|_| ResolutionFailure::InvalidPath)?;
    #[cfg(target_os = "windows")]
    if !has_allowed_windows_prefix(&canonical) {
        return Err(ResolutionFailure::InvalidPath);
    }
    let metadata = std::fs::metadata(&canonical).map_err(|_| ResolutionFailure::InvalidPath)?;
    if !metadata.is_dir() {
        return Err(ResolutionFailure::InvalidPath);
    }

    let canonical_text = canonical
        .to_str()
        .ok_or(ResolutionFailure::UnsupportedEncoding)?;
    let root_path = RootPath::parse(canonical_text).map_err(|_| ResolutionFailure::InvalidPath)?;
    Ok(root_path.as_str().to_owned())
}

/// Whether a Windows path leads with a prefix this helper accepts.
#[cfg(target_os = "windows")]
fn has_allowed_windows_prefix(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(
                prefix.kind(),
                Prefix::Disk(_)
                    | Prefix::VerbatimDisk(_)
                    | Prefix::UNC(..)
                    | Prefix::VerbatimUNC(..)
            )
    )
}

/// Why acquiring a chooser decision did not yield one.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum PickFailure {
    /// This platform has no supported native chooser implementation.
    #[error("this platform offers no supported directory chooser")]
    UnsupportedPlatform,
    /// The native dialog failed before producing a decision.
    #[error("the native directory dialog failed")]
    Failed,
}

/// Blocking chooser boundary behind the `Pick` operation.
///
/// One narrow private nondeterminism seam in the repo's acquisition-boundary
/// style: production always answers through [`NativeDirectoryChooser`],
/// while in-crate `cfg(test)` doubles inject deterministic decisions—including
/// cancellations and failures—to prove response mapping without ever opening
/// a visible dialog. This seam is deliberately not exported from the crate.
pub(crate) trait DirectoryChooser: fmt::Debug {
    /// Blocks until one directory is chosen, the user dismisses, or the
    /// dialog fails.
    ///
    /// Runs on the caller's thread; the helper invokes this on its main
    /// thread, where the Windows implementation reaches the common dialog
    /// stack that initializes COM STA internally.
    ///
    /// # Errors
    ///
    /// Returns [`PickFailure::UnsupportedPlatform`] where no native chooser
    /// exists (there is deliberately no shell fallback) and
    /// [`PickFailure::Failed`] when the dialog itself fails. Library error
    /// detail is dropped at this boundary so no implementation text can leak.
    fn choose_directory(&self) -> Result<Option<PathBuf>, PickFailure>;
}

/// Production chooser answering through the prepared native-dialog crate.
///
/// The builder defaults give a separate top-level dialog: no owner window,
/// no default location, no filename seed, and no filters. Cancellation
/// surfaces as `Ok(None)` from the library and passes through unchanged.
#[derive(Debug)]
pub(crate) struct NativeDirectoryChooser;

#[cfg(target_os = "windows")]
impl DirectoryChooser for NativeDirectoryChooser {
    fn choose_directory(&self) -> Result<Option<PathBuf>, PickFailure> {
        let selection = native_dialog::FileDialogBuilder::default()
            .open_single_dir()
            .show()
            .map_err(|_| PickFailure::Failed)?;
        Ok(selection)
    }
}

#[cfg(not(target_os = "windows"))]
impl DirectoryChooser for NativeDirectoryChooser {
    fn choose_directory(&self) -> Result<Option<PathBuf>, PickFailure> {
        Err(PickFailure::UnsupportedPlatform)
    }
}

/// Performs the one decoded helper operation and produces its typed outcome.
///
/// `Validate` resolves through [`resolve_selected_directory`]. `Pick` consults
/// the supplied chooser and applies the same resolution to whatever was
/// chosen; a non-UTF-8-representable raw selection becomes
/// [`Response::UnsupportedEncoding`], and every resolution failure maps onto
/// its matching response tag.
#[must_use]
pub(crate) fn perform_operation(
    request: &HelperRequest,
    chooser: &dyn DirectoryChooser,
) -> Response {
    match request {
        HelperRequest::Pick => pick_response(chooser),
        HelperRequest::Validate { path_text } => match resolve_selected_directory(path_text) {
            Ok(canonical_path) => Response::Selected { canonical_path },
            Err(failure) => response_for_resolution_failure(failure),
        },
    }
}

/// Maps chooser output onto responses, resolving any concrete selection.
fn pick_response(chooser: &dyn DirectoryChooser) -> Response {
    match chooser.choose_directory() {
        Ok(Some(selection)) => match selection.to_str() {
            Some(selection_text) => match resolve_selected_directory(selection_text) {
                Ok(canonical_path) => Response::Selected { canonical_path },
                Err(failure) => response_for_resolution_failure(failure),
            },
            None => Response::UnsupportedEncoding,
        },
        Ok(None) => Response::Cancelled,
        Err(PickFailure::UnsupportedPlatform) => Response::UnsupportedPlatform,
        Err(PickFailure::Failed) => Response::DialogFailed,
    }
}

/// Maps a resolution failure onto its empty-payload response tag.
const fn response_for_resolution_failure(failure: ResolutionFailure) -> Response {
    match failure {
        ResolutionFailure::InvalidPath => Response::InvalidPath,
        ResolutionFailure::UnsupportedEncoding => Response::UnsupportedEncoding,
    }
}
