use std::path::{Path, PathBuf};

use artisan_domain::{
    ApprovalMode, ClaudePermissionMode, ClaudeSelection, FilesystemAccess, NetworkAccess,
    OBSERVATION_TITLE_MAX_BYTES, RootPath,
};
use artisan_native_engine::{CLAUDE_NATIVE_CONTINUATION_VERSION, ClaudeThinkingDisplaySupport};
use serde_json::Value;

use super::protocol::{CLAUDE_MAX_ID_BYTES, ClaudeTurnError, user_message_line};

/// How the spawned CLI session is identified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeSession {
    /// A fresh native session opened with `--session-id`.
    Start(String),
    /// A resumed native session opened with `--resume` (later packet).
    Resume(String),
}

impl ClaudeSession {
    /// Returns the native session identity carried on the wire.
    pub(crate) fn session_id(&self) -> &str {
        match self {
            Self::Start(id) | Self::Resume(id) => id,
        }
    }
}

/// Mints a fresh native session identity (32 lowercase hex characters).
///
/// Returns `None` when operating-system entropy is unavailable; the caller
/// maps that to its entropy failure without touching the child.
pub(crate) fn new_session_id() -> Option<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    let mut id = String::with_capacity(32);
    for byte in bytes {
        id.push(HEX[(byte >> 4) as usize] as char);
        id.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Some(id)
}

/// Thinking display Artisan requests for one managed launch.
///
/// Backend-local launch policy, never a persisted selection field: supported
/// CLIs request public `summarized` prose, every other CLI keeps its
/// existing arguments. The same value tells the pump whether thinking text is
/// public summary prose; unrequested display semantics are unknown, so their
/// thinking text is never projected.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ClaudeThinkingDisplay {
    /// Flag omitted: the CLI keeps its own default.
    #[default]
    Unrequested,
    /// `--thinking-display summarized`.
    Summarized,
}

impl ClaudeThinkingDisplay {
    /// Resolves the requested display from the verified launch capability.
    pub(crate) const fn for_support(support: ClaudeThinkingDisplaySupport) -> Self {
        match support {
            ClaudeThinkingDisplaySupport::Summarized => Self::Summarized,
            ClaudeThinkingDisplaySupport::Unsupported => Self::Unrequested,
        }
    }

    const fn flag_value(self) -> Option<&'static str> {
        match self {
            Self::Unrequested => None,
            Self::Summarized => Some("summarized"),
        }
    }
}

/// Typed Claude settings derived from the durable selection.
///
/// Mirrors `ResolveRunOptions` in `modules/engines/src/claude/cli-engine.ts`
/// through the native single-policy shape: the durable selection *is* the
/// policy, so `never` approval and `bypassPermissions` mode both take the
/// dangerous-bypass flag, `default`/`bypassPermissions` modes omit the
/// `--permission-mode` flag, and every other native mode passes through
/// verbatim. The append-system-prompt file option is a launch-time concern
/// and stays out of the durable selection by domain design.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeSettings {
    profile_id: String,
    model: Option<String>,
    dangerous_bypass: bool,
    permission_mode: Option<String>,
    disable_tools: bool,
    safe_mode: bool,
    effort: Option<String>,
    thinking_display: ClaudeThinkingDisplay,
}

impl ClaudeSettings {
    /// Derives typed spawn settings from the durable selection.
    ///
    /// # Errors
    ///
    /// Returns [`ClaudeTurnError::Configuration`] when the permission
    /// relations violate the Claude adapter contract (read-only or offline
    /// policies fail closed instead of seating a wrong permission mode).
    pub(crate) fn from_selection(selection: &ClaudeSelection) -> Result<Self, ClaudeTurnError> {
        if selection.permission().filesystem() == FilesystemAccess::None {
            return Err(ClaudeTurnError::Configuration);
        }
        if selection.permission().network() != NetworkAccess::Enabled {
            return Err(ClaudeTurnError::Configuration);
        }
        let dangerous_bypass = selection.permission().approval() == ApprovalMode::Never
            || selection.permission_mode() == Some(ClaudePermissionMode::BypassPermissions);
        let permission_mode = match selection.permission_mode() {
            None
            | Some(ClaudePermissionMode::Default | ClaudePermissionMode::BypassPermissions) => None,
            Some(mode) => Some(mode.as_str().to_owned()),
        };
        Ok(Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model: selection.model_id().map(|model| model.as_str().to_owned()),
            dangerous_bypass,
            permission_mode,
            disable_tools: selection.disable_tools(),
            safe_mode: selection.safe_mode(),
            effort: selection.effort().map(|effort| effort.as_str().to_owned()),
            thinking_display: ClaudeThinkingDisplay::Unrequested,
        })
    }

    /// Applies the launch's resolved thinking display policy.
    #[must_use]
    pub(crate) const fn with_thinking_display(mut self, display: ClaudeThinkingDisplay) -> Self {
        self.thinking_display = display;
        self
    }

    /// Returns the thinking display this launch requests.
    pub(crate) const fn thinking_display(&self) -> ClaudeThinkingDisplay {
        self.thinking_display
    }

    /// Returns the managed profile identity.
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Builds the exact `claude` argv for one session.
    ///
    /// Base flags mirror the TypeScript spawn (`-p`, stream-JSON stdio,
    /// `--permission-prompt-tool stdio`). `--thinking-display` follows the
    /// resolved display policy identically for fresh starts and native
    /// resumes, so a supported CLI requests public summaries on every turn.
    pub(crate) fn spawn_args(&self, session: &ClaudeSession) -> Vec<String> {
        let mut args = vec![
            "-p".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            "--input-format".to_owned(),
            "stream-json".to_owned(),
            "--verbose".to_owned(),
            "--include-partial-messages".to_owned(),
            "--forward-subagent-text".to_owned(),
            "--permission-prompt-tool".to_owned(),
            "stdio".to_owned(),
        ];
        if self.dangerous_bypass {
            args.push("--dangerously-skip-permissions".to_owned());
        } else if let Some(mode) = self.permission_mode.as_deref() {
            args.push("--permission-mode".to_owned());
            args.push(mode.to_owned());
        }
        if self.disable_tools {
            args.push("--tools".to_owned());
            args.push(String::new());
        }
        if self.safe_mode {
            args.push("--safe-mode".to_owned());
        }
        if let Some(effort) = self.effort.as_deref() {
            args.push("--effort".to_owned());
            args.push(effort.to_owned());
        }
        if let Some(display) = self.thinking_display.flag_value() {
            args.push("--thinking-display".to_owned());
            args.push(display.to_owned());
        }
        match session {
            ClaudeSession::Start(id) => {
                args.push("--session-id".to_owned());
                args.push(id.clone());
            }
            ClaudeSession::Resume(id) => {
                args.push("--resume".to_owned());
                args.push(id.clone());
            }
        }
        if let Some(model) = self.model.as_deref() {
            args.push("--model".to_owned());
            args.push(model.to_owned());
        }
        args
    }

    /// Builds the first stdio user-message line for one session.
    ///
    /// Includes text and native image content in the same user message.
    pub(crate) fn user_message_payload(
        session: &ClaudeSession,
        prompt: &artisan_domain::QueueMessagePayload,
    ) -> String {
        super::protocol::user_message_with_images(
            session.session_id(),
            prompt.text().map(artisan_domain::AuthoredText::as_str),
            prompt.attachments(),
        )
    }

    pub(crate) fn user_message_line(session: &ClaudeSession, text: &str) -> String {
        user_message_line(session.session_id(), text)
    }
}

// ---------------------------------------------------------------------------
// L3: native continuation gate, resume, usage, quota diagnostics, and title
// ---------------------------------------------------------------------------

/// Minimum Claude CLI for native continuation.
///
/// The transport floor and the continuation floor are the same verified
/// release (`2.1.220`): the launch authority already refuses older CLIs at
/// probe time, and this gate re-checks the recorded constant
/// ([`CLAUDE_NATIVE_CONTINUATION_VERSION`], mirroring
/// `claude_native_continuation_version` in
/// `modules/engines/src/claude/probe.ts`) so a stale capability can never
/// authorize a resume the installed CLI no longer honors.
pub(crate) const CLAUDE_CONTINUATION_MINIMUM_CLI_VERSION: &str = CLAUDE_NATIVE_CONTINUATION_VERSION;

/// Returns whether Claude teardown must terminate the whole process group.
///
/// Always true: the owner spawns Claude with whole-group custody (Job Object
/// on Windows), so teardown kills claude grandchildren that still hold pipes
/// instead of orphaning them. Unobserved reaps quarantine through the shared
/// `cleanup_after_abort` / `finish_turn_result` path.
#[cfg(test)]
pub(crate) const fn claude_requires_group_termination() -> bool {
    true
}

/// Compares two `X.Y.Z` CLI spellings by their numeric core.
///
/// A leading name and any trailing pre-release/build suffix are ignored, so
/// `2.1.220 (Claude Code)` compares equal to `2.1.220`. Returns `None` when
/// either side has no parseable triple; callers fail closed on `None`.
pub(crate) fn compare_claude_cli_versions(left: &str, right: &str) -> Option<std::cmp::Ordering> {
    Some(parse_cli_triple(left)?.cmp(&parse_cli_triple(right)?))
}

/// Returns whether a probed CLI version meets a minimum floor.
pub(crate) fn claude_cli_meets_minimum(version: &str, minimum: &str) -> bool {
    matches!(
        compare_claude_cli_versions(version, minimum),
        Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Greater)
    )
}

fn parse_cli_triple(text: &str) -> Option<[u64; 3]> {
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

/// Native-continuation decision for one Claude turn.
///
/// `Compatible` authorizes `--resume` against the stored native session;
/// `Incompatible` carries the stable reason the dispatcher surfaces instead
/// of silently starting fresh or resuming across engines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeContinuationDecision {
    Compatible,
    Incompatible { reason: &'static str },
}

/// Bounded input for [`check_claude_native_continuation`].
pub(crate) struct ClaudeContinuationGateInput<'a> {
    /// Probed CLI version (`VerifiedClaudeLaunch::version`).
    pub cli_version: &'a str,
    /// Explicit target model from the current selection; `None` fails closed.
    pub target_model: Option<&'a str>,
    /// Advertised models when a `model/list` inventory was read; `None`
    /// skips advertisement validation (live inventory is deferred) but never
    /// skips the explicit-model or CLI gates.
    pub advertised_models: Option<&'a [&'a str]>,
    /// Whether the stored binding names the same `claude` engine. The
    /// dispatcher scopes continuation reads to `EngineId::Claude`, so a false
    /// value fails closed instead of resuming across engines.
    pub same_engine: bool,
}

/// Gates one Claude native continuation without touching provider state.
///
/// Order is contractual: same-engine first, then the explicit target model
/// (pre-validated before resume), then the `2.1.220` CLI floor, then model
/// advertisement when an inventory is supplied. Any failure is a typed
/// incompatible — never a silent fresh start and never a cross-engine resume.
pub(crate) fn check_claude_native_continuation(
    input: &ClaudeContinuationGateInput<'_>,
) -> ClaudeContinuationDecision {
    if !input.same_engine {
        return ClaudeContinuationDecision::Incompatible {
            reason: "Claude native continuation cannot resume across engines",
        };
    }
    let Some(target) = input.target_model.filter(|model| !model.is_empty()) else {
        return ClaudeContinuationDecision::Incompatible {
            reason: "Claude native continuation requires an explicit target model",
        };
    };
    if !claude_cli_meets_minimum(input.cli_version, CLAUDE_CONTINUATION_MINIMUM_CLI_VERSION) {
        return ClaudeContinuationDecision::Incompatible {
            reason: "Claude native continuation requires CLI 2.1.220 or newer",
        };
    }
    if let Some(advertised) = input.advertised_models
        && !advertised.contains(&target)
    {
        return ClaudeContinuationDecision::Incompatible {
            reason: "Claude does not currently advertise the target model",
        };
    }
    ClaudeContinuationDecision::Compatible
}

/// Reopens the stored native session for one authorized continuation.
///
/// Mirrors the TypeScript open path (`--resume` with the stored session id
/// over the same flags a fresh start would use): the resume reopens
/// provider-owned state only and never invents checkpoints. Returns `None`
/// when the stored session id is outside its bounded route-segment grammar
/// so the caller fails closed instead of resuming a corrupt session. The
/// init gate then requires the CLI to announce exactly this session.
pub(crate) fn claude_resume_session(stored_session_id: &str) -> Option<ClaudeSession> {
    if stored_session_id.is_empty() || stored_session_id.len() > CLAUDE_MAX_ID_BYTES {
        return None;
    }
    Some(ClaudeSession::Resume(stored_session_id.to_owned()))
}

/// The directory Claude Code files a working directory's transcripts under.
///
/// Mirrors `claude_project_directory_name` in
/// `modules/engines/src/claude/session-title.ts`: every character outside
/// `[A-Za-z0-9]` becomes a dash, drive colon and path separators included.
pub(crate) fn claude_project_directory_name(working_directory: &str) -> String {
    working_directory
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// The session transcript's path inside one Claude config home.
///
/// Mirrors `claude_session_transcript_path` in
/// `modules/engines/src/claude/session-title.ts`.
pub(crate) fn claude_session_transcript_path(
    home: &str,
    working_directory: &str,
    session_id: &str,
) -> PathBuf {
    Path::new(home)
        .join("projects")
        .join(claude_project_directory_name(working_directory))
        .join(format!("{session_id}.jsonl"))
}

/// Maximum transcript bytes read for a generated title.
///
/// Mirrors `maximum_transcript_bytes` in
/// `modules/engines/src/claude/session-title.ts`: settles are seconds apart
/// at their fastest, so the read is rare, but an unbounded read of a runaway
/// transcript would trade a nicety for memory pressure.
pub(crate) const CLAUDE_MAX_TRANSCRIPT_BYTES: u64 = 64 * 1024 * 1024;

/// Environment variable naming the Claude config home whose transcripts carry
/// generated titles. Mirrors the TypeScript spawn-override resolution
/// (`CLAUDE_CONFIG_DIR` over the ambient CLI default).
pub(crate) const CLAUDE_CONFIG_DIR_ENV_VAR: &str = "CLAUDE_CONFIG_DIR";

/// Returns the newest generated title across transcript lines, if any.
///
/// Mirrors `claude_session_title_from_lines` in
/// `modules/engines/src/claude/session-title.ts`: the CLI appends its
/// model-written title as `ai-title` records within the first turn and again
/// as the conversation evolves, and resolves the current name by taking the
/// newest — so this reader does the same. Malformed records are skipped, and
/// titles outside the domain title bound never become observations.
pub(crate) fn claude_session_title_from_lines(lines: &[&str]) -> Option<String> {
    for line in lines.iter().rev() {
        if !line.contains("ai-title") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("ai-title") {
            continue;
        }
        let Some(title) = record.get("aiTitle").and_then(Value::as_str) else {
            continue;
        };
        let title = title.trim();
        if title.is_empty() || title.len() > OBSERVATION_TITLE_MAX_BYTES {
            continue;
        }
        return Some(title.to_owned());
    }
    None
}

/// Reads the newest generated title from one session transcript.
///
/// Deliberately total: a missing transcript, an unreadable file, an oversize
/// file, or malformed records all mean "no title yet" — a run must never
/// fail, or even complain, because a nicety could not be read.
pub(crate) fn read_claude_session_title(transcript_path: &Path) -> Option<String> {
    let size = std::fs::metadata(transcript_path).ok()?.len();
    if size > CLAUDE_MAX_TRANSCRIPT_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(transcript_path).ok()?;
    if u64::try_from(text.len()).ok()? > CLAUDE_MAX_TRANSCRIPT_BYTES {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    claude_session_title_from_lines(&lines)
}

/// Captures the generated title for one native session, if the managed config
/// home names a readable transcript for it.
///
/// Best-effort beside terminal settlement: any failure means "no title yet".
/// Only the managed `CLAUDE_CONFIG_DIR` override is consulted — ambient home
/// resolution stays with the CLI until a home-directory source exists.
pub(crate) fn claude_transcript_title_for_session(
    project_root: &RootPath,
    session_id: &str,
) -> Option<String> {
    let home = std::env::var(CLAUDE_CONFIG_DIR_ENV_VAR)
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let path = claude_session_transcript_path(home.trim(), project_root.as_str(), session_id);
    read_claude_session_title(&path)
}
