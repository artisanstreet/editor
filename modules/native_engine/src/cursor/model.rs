//! Typed Cursor model resolution, ACP argument shape, startup-failure
//! classification, and image-input record.
//!
//! All three mirror `modules/engines/src/cursor/engine.ts` exactly:
//!
//! - [`resolve_cursor_model`] ports `ResolveCursorModel`
//!   (`model[-effort][-fast]` from `cursor.reasoning_effort` + `cursor.speed`;
//!   `[...]` bracket passthrough);
//! - [`cursor_acp_args`] ports `CursorAcpArgs`
//!   (`--model`, `--mode ask` on read-only, `--force`, then `acp`);
//! - [`classify_cursor_startup_failure`] ports `ClassifyCursorStartupFailure`
//!   (`Cannot use this model: X` → `AE-PROVIDER-206` carrying `X`);
//! - [`CURSOR_IMAGE_INPUT`] records `image_input: "image"`
//!   (`cursor/engine.ts:136`): Cursor images use image blocks, never embedded
//!   resources and never dropped.
//!
//! These are data for the later ACP runtime packet, not executed here: no
//! prompt, no inference, no session or account changes.

/// ACP image-input mode recorded for the later Cursor runtime packet.
/// Cursor takes image blocks (`"image"`), never embedded resources.
pub const CURSOR_IMAGE_INPUT: &str = "image";

/// Stable Artisan code for a known pre-session model rejection.
pub const CURSOR_ARTISAN_CODE_UNAVAILABLE_MODEL: &str = "AE-PROVIDER-206";

/// Engine id carried by the startup-failure record.
pub const CURSOR_ENGINE_ID: &str = "cursor";

/// Maximum model-name characters captured from a startup failure, mirroring
/// the `{1,160}` bound in the TypeScript pattern.
pub const MAX_UNAVAILABLE_MODEL_CHARS: usize = 160;

/// Cursor speed selector (`cursor.speed`). `Fast` appends the `-fast` suffix
/// unless the resolved model already carries it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorSpeed {
    Normal,
    Fast,
}

impl CursorSpeed {
    /// Returns whether this selector requests the `-fast` model suffix.
    #[must_use]
    pub const fn is_fast(self) -> bool {
        matches!(self, Self::Fast)
    }
}

/// Cursor permission selector (`cursor.permission_mode`). Only `Force` maps
/// to a CLI flag (`--force`); read-only sessions map to `--mode ask` via
/// [`CursorAcpInputs::read_only`] regardless of this value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorPermissionMode {
    Force,
}

/// Model-resolution inputs recorded for the later ACP runtime packet,
/// mirroring the `EngineOpenInput` fields `ResolveCursorModel` reads
/// (`model`, `cursor.reasoning_effort`, `cursor.speed`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorModelInputs {
    /// Base model id, if the caller selected one.
    pub model: Option<String>,
    /// Requested reasoning effort, appended as `-effort` unless the base
    /// model already carries an effort suffix.
    pub reasoning_effort: Option<String>,
    /// Requested speed; `Fast` appends `-fast` unless already present.
    pub speed: Option<CursorSpeed>,
}

impl CursorModelInputs {
    /// Resolves these inputs to one exact model id, or `None` when no model
    /// was selected. See [`resolve_cursor_model`].
    #[must_use]
    pub fn resolve(&self) -> Option<String> {
        resolve_cursor_model(
            self.model.as_deref(),
            self.reasoning_effort.as_deref(),
            self.speed,
        )
    }
}

/// ACP spawn inputs recorded for the later runtime packet, mirroring the
/// `EngineOpenInput` fields `CursorAcpArgs` reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorAcpInputs {
    /// Model-resolution inputs (`model`, `reasoning_effort`, `speed`).
    pub model: CursorModelInputs,
    /// Permission-mode selector (`cursor.permission_mode`).
    pub permission_mode: Option<CursorPermissionMode>,
    /// `true` when `permission_policy.write_access === false`, mapping to
    /// `--mode ask` and taking precedence over `Force`.
    pub read_only: bool,
}

fn has_reasoning_suffix(model: &str) -> bool {
    const EFFORT_SUFFIXES: &[&str] = &["-low", "-medium", "-high", "-xhigh", "-max", "-ultra"];
    let without_fast = model.strip_suffix("-fast").unwrap_or(model);
    EFFORT_SUFFIXES
        .iter()
        .any(|suffix| without_fast.ends_with(suffix))
}

/// Resolves Artisan's Cursor controls to one exact model id, mirroring
/// `ResolveCursorModel` in `modules/engines/src/cursor/engine.ts`:
///
/// - `None` stays `None`;
/// - a model containing `[` passes through unchanged (effort and speed are
///   ignored, exactly like the TypeScript early return);
/// - otherwise the non-empty effort is appended as `-effort` unless the base
///   already carries an effort suffix (`-(low|medium|high|xhigh|max|ultra)`
///   with an optional `-fast`);
/// - `Fast` speed appends `-fast` unless already present.
#[must_use]
pub fn resolve_cursor_model(
    model: Option<&str>,
    effort: Option<&str>,
    speed: Option<CursorSpeed>,
) -> Option<String> {
    let model = model?;
    if model.contains('[') {
        return Some(model.to_owned());
    }
    let effort = effort.map(str::trim).filter(|value| !value.is_empty());
    let with_effort = match effort {
        None => model.to_owned(),
        Some(effort) if has_reasoning_suffix(model) => model.to_owned(),
        Some(effort) => format!("{model}-{effort}"),
    };
    if speed.is_some_and(CursorSpeed::is_fast) && !with_effort.ends_with("-fast") {
        Some(format!("{with_effort}-fast"))
    } else {
        Some(with_effort)
    }
}

/// Builds the Cursor ACP spawn arguments, mirroring `CursorAcpArgs` in
/// `modules/engines/src/cursor/engine.ts`: `--model <resolved>` when a model
/// resolves, `--mode ask` for read-only sessions (which wins over `Force`),
/// `--force` for the force permission mode, then the `acp` subcommand.
#[must_use]
pub fn cursor_acp_args(inputs: &CursorAcpInputs) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = inputs.model.resolve() {
        args.push("--model".to_owned());
        args.push(model);
    }
    if inputs.read_only {
        args.push("--mode".to_owned());
        args.push("ask".to_owned());
    } else if inputs.permission_mode == Some(CursorPermissionMode::Force) {
        args.push("--force".to_owned());
    }
    args.push("acp".to_owned());
    args
}

/// Typed `AE-PROVIDER-206` startup failure: Cursor's known pre-session model
/// rejection (`Cannot use this model: X`), carrying the captured model name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorStartupFailure {
    model: String,
}

impl CursorStartupFailure {
    /// Returns the rejected model name captured from the diagnostic.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Returns the stable Artisan code (`AE-PROVIDER-206`).
    #[must_use]
    pub const fn artisan_code(&self) -> &'static str {
        CURSOR_ARTISAN_CODE_UNAVAILABLE_MODEL
    }

    /// Returns the engine id (`cursor`).
    #[must_use]
    pub const fn engine_id(&self) -> &'static str {
        CURSOR_ENGINE_ID
    }

    /// Returns the human message, mirroring the TypeScript wording exactly.
    #[must_use]
    pub fn message(&self) -> String {
        format!(
            "Cursor does not make model {model} available to this account.",
            model = self.model
        )
    }
}

const UNAVAILABLE_MODEL_PREFIX: &[u8] = b"cannot use this model:";

fn ascii_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C)
}

/// Converts Cursor's known pre-session model rejection to Artisan's stable
/// model code, mirroring `ClassifyCursorStartupFailure` in
/// `modules/engines/src/cursor/engine.ts`
/// (`/Cannot use this model:\s*([^\r\n]{1,160}?)(?:\.\s+Valid models|[\r\n]|$)/i`):
/// a case-insensitive prefix, skipped whitespace, a lazy capture of up to
/// 160 non-line characters, and the first terminator (`. Valid models`
/// case-insensitively, a line break, or end of input). Returns `None` when
/// the diagnostic carries no such rejection.
#[must_use]
pub fn classify_cursor_startup_failure(stderr: &str) -> Option<CursorStartupFailure> {
    let bytes = stderr.as_bytes();
    let mut search = 0_usize;
    while search + UNAVAILABLE_MODEL_PREFIX.len() <= bytes.len() {
        // Byte-wise scan: only attempt the prefix match on UTF-8 boundaries
        // so every slice below stays safe on non-ASCII input.
        if !stderr.is_char_boundary(search) {
            search += 1;
            continue;
        }
        if bytes[search..search + UNAVAILABLE_MODEL_PREFIX.len()]
            .eq_ignore_ascii_case(UNAVAILABLE_MODEL_PREFIX)
        {
            let mut cursor = search + UNAVAILABLE_MODEL_PREFIX.len();
            while cursor < bytes.len() && ascii_whitespace(bytes[cursor]) {
                cursor += 1;
            }
            let capture_start = cursor;
            let mut char_count = 0_usize;
            while cursor < bytes.len()
                && bytes[cursor] != b'\r'
                && bytes[cursor] != b'\n'
                && char_count < MAX_UNAVAILABLE_MODEL_CHARS
            {
                // Advance by one UTF-8 character; the pattern counts
                // characters, not bytes.
                let step = utf8_char_len(bytes[cursor]);
                cursor += step;
                char_count += 1;
            }
            let line_ended =
                cursor >= bytes.len() || bytes[cursor] == b'\r' || bytes[cursor] == b'\n';
            let capture = &stderr[capture_start..cursor];
            // Lazy expansion: the shortest prefix of at least one character
            // satisfying a terminator wins, mirroring `{1,160}?`. End
            // positions step over whole characters so slicing stays on UTF-8
            // boundaries; terminators only match ASCII, so intermediate byte
            // offsets could never satisfy one.
            let ends = capture
                .char_indices()
                .map(|(index, ch)| index + ch.len_utf8());
            for end in ends {
                let rest = &capture[end..];
                if end == capture.len() {
                    if line_ended {
                        return Some(CursorStartupFailure {
                            model: capture[..end].trim().to_owned(),
                        });
                    }
                    break;
                }
                if rest.as_bytes()[0] == b'.' && valid_models_follow(&capture[end + 1..]) {
                    return Some(CursorStartupFailure {
                        model: capture[..end].trim().to_owned(),
                    });
                }
            }
            // This occurrence carries no terminator; keep scanning for a
            // later one, like the TypeScript regex engine does.
        }
        search += 1;
    }
    None
}

/// Returns the UTF-8 sequence length for a leading byte. Continuation bytes
/// (which never start a scan position here) count as one byte so slicing
/// stays safe.
fn utf8_char_len(lead: u8) -> usize {
    if lead < 0x80 {
        1
    } else if lead >= 0xF0 {
        4
    } else if lead >= 0xE0 {
        3
    } else if lead >= 0xC0 {
        2
    } else {
        1
    }
}

/// Returns whether `rest` (just past a `.`) continues with whitespace and
/// `valid models` case-insensitively, mirroring `\.\s+Valid models` under
/// the `/i` flag.
fn valid_models_follow(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    let mut cursor = 0_usize;
    while cursor < bytes.len() && ascii_whitespace(bytes[cursor]) {
        cursor += 1;
    }
    if cursor == 0 {
        return false;
    }
    bytes[cursor..]
        .get(..12)
        .is_some_and(|head| head.eq_ignore_ascii_case(b"valid models"))
}
