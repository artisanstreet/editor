//! Hermes service adapter: settings, verified launch, private-service
//! readiness, and the native-continuation gate.

use std::path::{Path, PathBuf};

use artisan_domain::HermesSelection;
use base64::Engine as _;
use serde_json::Value;
use thiserror::Error;
#[cfg(test)]
use tokio::io::AsyncRead;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::time::Instant;

use super::super::process::ChildParts;
use super::super::readiness::ReadinessError;
use artisan_transport::CancelHandle;

/// Maximum bytes consumed while waiting for the readiness record (mirrors the
/// TS 256 KiB readiness bound).
pub(crate) const HERMES_MAX_READY_BYTES: usize = 256 * 1024;

/// Payload-free failure of the Hermes wire boundary.
///
/// Transport mistakes (spawn, readiness, handshake, request, stream, stall,
/// cancel, shutdown, deadline, interruption, exit) surface as the owner
/// [`EngineOperationError`](super::operation::EngineOperationError) at the
/// dispatch arm; only typed-boundary mistakes originate here.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub(crate) enum HermesTurnError {
    #[error("hermes turn misconfigured")]
    Configuration,
    #[error("hermes does not accept image attachments")]
    ImagesUnsupported,
    #[error("hermes gateway stream failed")]
    StreamFailed,
}

/// Typed Hermes settings derived from the durable selection.
///
/// Mirrors the TypeScript `Open` selection checks (`profile_id`,
/// `provider_route_id`, `model_id`) plus the `hermes.*` provider options. The
/// domain selection already validates every field, so construction is total.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HermesSettings {
    profile_id: String,
    model_id: String,
    route_id: String,
    pub(super) permission_yolo: bool,
    reasoning_effort: Option<String>,
    fast: bool,
}

impl HermesSettings {
    /// Derives typed gateway settings from the durable selection.
    #[must_use]
    pub(crate) fn from_selection(selection: &HermesSelection) -> Self {
        Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model_id: selection.model_id().as_str().to_owned(),
            route_id: selection.route_id().as_str().to_owned(),
            permission_yolo: selection.permission_mode().as_str() == "yolo",
            reasoning_effort: selection
                .reasoning_effort()
                .map(|effort| effort.as_str().to_owned()),
            fast: selection.fast(),
        }
    }

    /// Returns the managed profile identity.
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns the selected model identity.
    pub(crate) fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Returns the selected provider route identity.
    pub(crate) fn route_id(&self) -> &str {
        &self.route_id
    }

    /// Builds the `session.create` params object for one turn.
    ///
    /// The installed profile is omitted for the `default` profile exactly
    /// like the TypeScript engine; reasoning effort defaults to `medium` at
    /// argument-building time per the domain contract.
    pub(crate) fn create_params(&self, project_root: &str, guidance: &[Value]) -> Value {
        let mut params = serde_json::Map::new();
        params.insert("close_on_disconnect".to_owned(), Value::Bool(false));
        params.insert("cwd".to_owned(), Value::String(project_root.to_owned()));
        params.insert("fast".to_owned(), Value::Bool(self.fast));
        params.insert("messages".to_owned(), Value::Array(guidance.to_owned()));
        params.insert("model".to_owned(), Value::String(self.model_id.clone()));
        if self.profile_id != "default" {
            params.insert("profile".to_owned(), Value::String(self.profile_id.clone()));
        }
        params.insert("provider".to_owned(), Value::String(self.route_id.clone()));
        params.insert(
            "reasoning_effort".to_owned(),
            Value::String(
                self.reasoning_effort
                    .clone()
                    .unwrap_or_else(|| "medium".to_owned()),
            ),
        );
        params.insert("source".to_owned(), Value::String("artisan".to_owned()));
        Value::Object(params)
    }
}

/// Verified Hermes service launch for one turn.
///
/// Carries the resolved executable, the selecting profile identity, and the
/// probed version. Never `Clone`: the dispatch arm moves it into the single
/// internal input. Revalidation rechecks that the executable is still a file;
/// install-fence depth beyond that stays with the native-engine discovery
/// packet.
pub(crate) struct VerifiedHermesLaunch {
    executable: PathBuf,
    profile_id: String,
    version: String,
}

impl VerifiedHermesLaunch {
    /// Creates a launch after validating its identities.
    ///
    /// Returns `None` when any field is empty.
    pub(crate) fn new(executable: PathBuf, profile_id: String, version: String) -> Option<Self> {
        if profile_id.is_empty() || version.is_empty() {
            return None;
        }
        Some(Self {
            executable,
            profile_id,
            version,
        })
    }

    /// Returns the managed profile identity.
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns the resolved executable path.
    pub(crate) fn executable_path(&self) -> &Path {
        &self.executable
    }

    /// Returns the probed version string.
    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    /// Revalidates that the executable is still present.
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] when the executable is no longer a file.
    pub(crate) fn revalidate(&self) -> std::io::Result<()> {
        if self.executable.is_file() {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "hermes launch rejected",
            ))
        }
    }
}

impl std::fmt::Debug for VerifiedHermesLaunch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VerifiedHermesLaunch { <redacted> }")
    }
}

/// Resolves the Hermes executable through discovery precedence
/// (`HERMES_EXECUTABLE`, installed local-app-data, `PATH`).
#[must_use]
pub(crate) fn resolve_service_executable() -> Option<PathBuf> {
    artisan_native_engine::hermes::resolve_hermes_executable()
        .map(|resolved| resolved.path().to_owned())
}

/// Mints a fresh 32-byte dashboard session token (base64url, no padding).
///
/// Returns `None` when operating-system entropy is unavailable; the caller
/// maps that to its entropy failure without touching the child.
#[must_use]
pub(crate) fn new_session_token() -> Option<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Parses one `HERMES_BACKEND_READY port=N` readiness line.
///
/// Returns the loopback port, or `None` for any other line, blank line, or
/// out-of-range port (0 and values above 65535 never bind a service).
#[must_use]
pub(crate) fn parse_ready_port_line(line: &str) -> Option<u16> {
    let trimmed = line.trim_end_matches(['\r', '\n']).trim();
    let rest = trimmed.strip_prefix("HERMES_BACKEND_READY port=")?;
    if rest.is_empty() || rest.len() > 5 || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let port: u16 = rest.parse().ok()?;
    if port == 0 { None } else { Some(port) }
}

/// Reads readiness lines from a generic stream until the ready record.
///
/// Test seam for the readiness line protocol; the owner driver
/// ([`drive_service_readiness`]) adds stderr pumping around the same grammar.
#[cfg(test)]
pub(crate) async fn read_ready_port<R: AsyncRead + Unpin>(
    reader: &mut R,
    maximum_line: usize,
    maximum_bytes: usize,
    deadline: Instant,
    cancel: &CancelHandle,
    shutdown: &CancelHandle,
) -> Result<u16, ReadinessError> {
    let mut buffered = BufReader::new(reader);
    let mut line = String::new();
    let mut consumed: usize = 0;
    loop {
        if shutdown.is_cancelled() {
            return Err(ReadinessError::Shutdown);
        }
        if cancel.is_cancelled() {
            return Err(ReadinessError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(ReadinessError::Deadline);
        }
        line.clear();
        let outcome = tokio::select! {
            biased;
            () = shutdown.wait() => Err(ReadinessError::Shutdown),
            () = cancel.wait() => Err(ReadinessError::Cancelled),
            () = tokio::time::sleep_until(deadline) => Err(ReadinessError::Deadline),
            read = buffered.read_line(&mut line) => match read {
                Ok(0) => Err(ReadinessError::EofBeforeNewline),
                Ok(count) => Ok(count),
                Err(_) => Err(ReadinessError::Io),
            },
        };
        let count = outcome?;
        consumed = consumed.saturating_add(count);
        if consumed > maximum_bytes || line.len() > maximum_line {
            return Err(ReadinessError::Io);
        }
        if let Some(port) = parse_ready_port_line(&line) {
            return Ok(port);
        }
    }
}

/// Drives bounded service readiness on the spawned child.
///
/// Mirrors the owner `drive_readiness` discipline: shutdown, cancellation,
/// and the phase deadline win over output, and stderr counting is pumped
/// while waiting so a chatty child cannot wedge the pipe.
pub(crate) async fn drive_service_readiness(
    stdout: &mut tokio::process::ChildStdout,
    parts: &mut ChildParts,
    deadline: Instant,
    shutdown: &CancelHandle,
    control: &CancelHandle,
    maximum_line: usize,
) -> Result<u16, ReadinessError> {
    let mut line = String::new();
    let mut consumed: usize = 0;
    let mut reader = BufReader::new(stdout);
    loop {
        if shutdown.is_cancelled() {
            return Err(ReadinessError::Shutdown);
        }
        if control.is_cancelled() {
            return Err(ReadinessError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(ReadinessError::Deadline);
        }
        line.clear();
        tokio::select! {
            biased;
            () = shutdown.wait() => return Err(ReadinessError::Shutdown),
            () = control.wait() => return Err(ReadinessError::Cancelled),
            () = tokio::time::sleep_until(deadline) => return Err(ReadinessError::Deadline),
            event = parts.stderr_counter.pump(), if parts.stderr_counter.state() == super::super::process::StderrState::Open => {
                let _ = event;
            }
            waited = parts.child.wait() => {
                match waited {
                    Ok(_) => return Err(ReadinessError::EofBeforeNewline),
                    Err(_) => return Err(ReadinessError::Io),
                }
            }
            read = reader.read_line(&mut line) => {
                match read {
                    Ok(0) => return Err(ReadinessError::EofBeforeNewline),
                    Ok(count) => {
                        consumed = consumed.saturating_add(count);
                        if consumed > HERMES_MAX_READY_BYTES || line.len() > maximum_line {
                            return Err(ReadinessError::Io);
                        }
                        if let Some(port) = parse_ready_port_line(&line) {
                            return Ok(port);
                        }
                    }
                    Err(_) => return Err(ReadinessError::Io),
                }
            }
        }
    }
}

/// Rejects image attachments with a typed error.
///
/// The Hermes catalog reports `image_input: false`, so any attachment fails
/// the turn closed here instead of sending a degraded text-only prompt.
///
/// # Errors
///
/// Returns [`HermesTurnError::ImagesUnsupported`] when the prompt carries any
/// image attachment.
pub(crate) fn reject_image_attachments(
    prompt: &artisan_domain::QueueMessagePayload,
) -> Result<(), HermesTurnError> {
    if prompt.attachments().is_empty() {
        Ok(())
    } else {
        Err(HermesTurnError::ImagesUnsupported)
    }
}

// H3: native continuation gate, recorded service version, and group teardown
// ---------------------------------------------------------------------------

/// Minimum Hermes service for native continuation.
///
/// The transport floor and the continuation floor are the same verified
/// release (`0.20.0`): the launch authority already refuses older services
/// at probe time, and this gate re-checks the recorded version (mirroring
/// `minimum_hermes_version` in `modules/engines/src/hermes/service.ts`) so a
/// stale capability can never authorize a resume the installed service no
/// longer honors.
pub(crate) const HERMES_CONTINUATION_MINIMUM_SERVICE_VERSION: &str = "0.20.0";

/// Returns whether Hermes teardown must terminate the whole process group.
///
/// Always true: the owner spawns Hermes with whole-group custody (Job Object
/// on Windows), so teardown kills hermes grandchildren that still hold pipes
/// instead of orphaning them. Unobserved reaps quarantine through the shared
/// `cleanup_after_abort` / `finish_turn_result` path.
#[cfg(test)]
pub(crate) const fn hermes_requires_group_termination() -> bool {
    true
}

/// Compares two service version spellings by their numeric core.
///
/// A leading name (`Hermes Agent v`) and any trailing pre-release suffix are
/// ignored, so `Hermes Agent v0.20.0` compares equal to `0.20.0`. The
/// recorded launch version is the bare `Display` form (`0.20.0`), which the
/// unanchored TypeScript probe pattern also accepts. Returns `None` when
/// either side has no parseable triple; callers fail closed on `None`.
pub(crate) fn compare_hermes_service_versions(
    left: &str,
    right: &str,
) -> Option<std::cmp::Ordering> {
    Some(parse_service_triple(left)?.cmp(&parse_service_triple(right)?))
}

/// Returns whether a recorded service version meets a minimum floor.
pub(crate) fn hermes_service_meets_minimum(version: &str, minimum: &str) -> bool {
    matches!(
        compare_hermes_service_versions(version, minimum),
        Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater)
    )
}

fn parse_service_triple(text: &str) -> Option<[u64; 3]> {
    let start = text.find(|character: char| character.is_ascii_digit())?;
    let run: String = text[start..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let mut parts = run.split('.');
    Some([
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ])
}

/// Native-continuation decision for one Hermes turn.
///
/// `Compatible` authorizes `session.resume` against the stored durable
/// session; `Incompatible` carries the stable reason the dispatcher surfaces
/// instead of silently starting fresh or resuming across engines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HermesContinuationDecision {
    Compatible,
    Incompatible { reason: &'static str },
}

/// Bounded input for [`check_hermes_native_continuation`].
pub(crate) struct HermesContinuationGateInput<'a> {
    /// Recorded service version (`VerifiedHermesLaunch::version`).
    pub service_version: &'a str,
    /// Explicit target model from the current selection; `None` fails closed.
    pub target_model: Option<&'a str>,
    /// Advertised models when a `model.options` inventory was read; `None`
    /// skips advertisement validation (the live inventory check pre-validates
    /// the exact target model before resume) but never skips the
    /// explicit-model or service gates.
    pub advertised_models: Option<&'a [&'a str]>,
    /// Whether the stored binding names the same `hermes` engine. The
    /// dispatcher scopes continuation reads to `EngineId::Hermes`, so a false
    /// value fails closed instead of resuming across engines.
    pub same_engine: bool,
}

/// Gates one Hermes native continuation without touching provider state.
///
/// Order is contractual: same-engine first, then the explicit target model
/// (pre-validated before resume), then the `0.20.0` service floor, then model
/// advertisement when an inventory is supplied. Any failure is a typed
/// incompatible — never a silent fresh start and never a cross-engine resume.
pub(crate) fn check_hermes_native_continuation(
    input: &HermesContinuationGateInput<'_>,
) -> HermesContinuationDecision {
    if !input.same_engine {
        return HermesContinuationDecision::Incompatible {
            reason: "Hermes native continuation cannot resume across engines",
        };
    }
    let Some(target) = input.target_model.filter(|model| !model.is_empty()) else {
        return HermesContinuationDecision::Incompatible {
            reason: "Hermes native continuation requires an explicit target model",
        };
    };
    if !hermes_service_meets_minimum(
        input.service_version,
        HERMES_CONTINUATION_MINIMUM_SERVICE_VERSION,
    ) {
        return HermesContinuationDecision::Incompatible {
            reason: "Hermes native continuation requires service 0.20.0 or newer",
        };
    }
    if let Some(advertised) = input.advertised_models
        && !advertised.contains(&target)
    {
        return HermesContinuationDecision::Incompatible {
            reason: "Hermes does not currently advertise the target model",
        };
    }
    HermesContinuationDecision::Compatible
}
