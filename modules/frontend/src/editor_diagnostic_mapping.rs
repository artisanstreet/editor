//! Pure editor-diagnostic position and severity mapping.
//!
//! This is the dependency-free native counterpart of the mapping seam in
//! `modules/frontend/src/lib/editor/codemirror-adapter.ts`. It keeps the
//! provider-facing diagnostic shape separate from the marker shape a future
//! editor adapter can consume. It does not own an editor document, marker
//! owner, renderer, linter, language service, transport, or asynchronous
//! work.
//!
//! Positions are one-based Unicode-scalar line and column pairs. The mapped
//! positions are UTF-8 byte offsets, and every offset is a valid `str` scalar
//! boundary. This scalar-column rule avoids the UTF-16 and UTF-8 unit mismatch
//! that a native adapter cannot safely represent with direct byte arithmetic.

#![forbid(unsafe_code)]

/// The four severities accepted by the editor diagnostic seam.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditorDiagnosticSeverity {
    /// A diagnostic that represents an error.
    Error,
    /// A diagnostic that represents a warning.
    Warning,
    /// A diagnostic that provides information without indicating a failure.
    Info,
    /// A low-priority diagnostic hint.
    Hint,
}

impl EditorDiagnosticSeverity {
    /// Every severity in the same canonical order as the editor adapter map.
    pub const ALL: [Self; 4] = [Self::Error, Self::Warning, Self::Info, Self::Hint];

    /// Returns the exact lower-case severity name used by the marker adapter.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Hint => "hint",
        }
    }
}

/// A provider diagnostic expressed with one-based line and scalar-column pairs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorDiagnostic {
    /// Optional provider code copied to the mapped marker's `source` field.
    pub code: Option<String>,
    /// One-based end column, interpreted as an exclusive range endpoint.
    pub end_column: u32,
    /// One-based end line, interpreted as an exclusive range endpoint.
    pub end_line: u32,
    /// Human-readable diagnostic text, copied without normalization.
    pub message: String,
    /// Diagnostic severity.
    pub severity: EditorDiagnosticSeverity,
    /// One-based start column.
    pub start_column: u32,
    /// One-based start line.
    pub start_line: u32,
}

impl EditorDiagnostic {
    /// Creates a diagnostic without an optional provider code.
    #[must_use]
    pub fn new(
        message: impl Into<String>,
        severity: EditorDiagnosticSeverity,
        start_line: u32,
        start_column: u32,
        end_line: u32,
        end_column: u32,
    ) -> Self {
        Self {
            code: None,
            end_column,
            end_line,
            message: message.into(),
            severity,
            start_column,
            start_line,
        }
    }

    /// Returns this diagnostic with a provider code retained for `source`.
    #[must_use]
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }
}

/// A marker ready for an editor adapter to apply to a UTF-8 document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MappedMarker {
    /// UTF-8 byte offset at which the marker begins.
    pub from: usize,
    /// UTF-8 byte offset at which the marker ends, never below [`Self::from`].
    pub to: usize,
    /// Human-readable diagnostic text.
    pub message: String,
    /// Exact diagnostic severity.
    pub severity: EditorDiagnosticSeverity,
    /// Optional diagnostic code represented as the adapter's source value.
    pub source: Option<String>,
}

/// Maps editor diagnostics to byte-safe markers for `document`.
///
/// Lines are split at LF bytes. A CR immediately before an LF is part of the
/// line terminator and is therefore excluded from that line's content end;
/// standalone CR bytes remain content. The document always has at least one
/// logical line, so an empty document maps every position to byte offset zero.
/// Line zero and columns zero are treated like line one and column one.
/// Oversized lines clamp to the final logical line, and oversized columns clamp
/// to the selected line's content end. Endpoints are mapped independently,
/// then a reversed range is collapsed by setting `to` equal to `from`.
///
/// The input slice and its strings are only borrowed. Messages and optional
/// codes are cloned into the returned markers, so this function does not
/// mutate the diagnostics supplied by the caller. Marker-owner identity is
/// intentionally absent: the caller that owns that identity applies the
/// returned markers.
#[must_use]
pub fn map_editor_diagnostics(
    document: &str,
    diagnostics: &[EditorDiagnostic],
) -> Vec<MappedMarker> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let from =
                byte_offset_for_position(document, diagnostic.start_line, diagnostic.start_column);
            let mapped_to =
                byte_offset_for_position(document, diagnostic.end_line, diagnostic.end_column);

            MappedMarker {
                from,
                to: mapped_to.max(from),
                message: diagnostic.message.clone(),
                severity: diagnostic.severity,
                source: diagnostic.code.clone(),
            }
        })
        .collect()
}

/// Returns the UTF-8 byte offset for one clamped one-based document position.
fn byte_offset_for_position(document: &str, line_number: u32, column: u32) -> usize {
    let (line_start, content_end) = line_bounds(document, line_number);
    let scalar_column = column.saturating_sub(1);
    if scalar_column == 0 {
        return line_start;
    }

    let line = &document[line_start..content_end];
    let scalar_index = usize::try_from(scalar_column).unwrap_or(usize::MAX);
    line.char_indices()
        .nth(scalar_index)
        .map_or(content_end, |(offset, _)| line_start + offset)
}

/// Returns the byte start and content end of a clamped logical line.
fn line_bounds(document: &str, line_number: u32) -> (usize, usize) {
    let bytes = document.as_bytes();
    let target_line = line_number.max(1);
    let mut current_line = 1_u32;
    let mut line_start = 0;

    for (index, &byte) in bytes.iter().enumerate() {
        if byte != b'\n' {
            continue;
        }

        if current_line == target_line {
            let content_end = if index > line_start && bytes[index - 1] == b'\r' {
                index - 1
            } else {
                index
            };
            return (line_start, content_end);
        }

        current_line = current_line.saturating_add(1);
        line_start = index.saturating_add(1);
    }

    (line_start, document.len())
}
