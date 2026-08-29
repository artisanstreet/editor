//! Dependency-light editor workspace identity and route targets.
//!
//! This is the native value boundary for
//! modules/frontend/src/lib/editor/workspace-identity.ts. It owns only the
//! small thread/project projections needed by that policy; protocol records,
//! navigation effects, and route reconciliation remain with their callers.
//!
//! URL components use the exact encodeURIComponent safe-byte set
//! (A-Z a-z 0-9 - _ . ! ~ * ' ( )) and uppercase hexadecimal escapes. Rust
//! strings are valid Unicode, so their UTF-8 bytes provide the same encoding
//! for every representable JavaScript string without URL parsing or other
//! normalization.

#![allow(clippy::module_name_repetitions)]

const LEGACY_THREAD_PREFIX: &str = "thread_";
const DETACHED_WORKSPACE_ROUTE_ID: &str = "_";
const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// The minimal project projection required to build an editor target.
///
/// `project_id` is caller-owned identity text. This type stores it without
/// trimming, case folding, path conversion, or any other interpretation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorProjectInput {
    /// The authoritative workspace/project identifier.
    pub project_id: String,
}

impl EditorProjectInput {
    /// Creates a project input while retaining the supplied identifier.
    #[must_use]
    pub fn new(project_id: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
        }
    }
}

/// The minimal thread projection required by the editor target policy.
///
/// None for `primary_project` represents a detached historical thread. It is
/// not interchangeable with an empty project identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorThreadInput {
    /// The authoritative primary project, when the thread has one.
    pub primary_project: Option<EditorProjectInput>,
    /// The caller's authoritative thread identity.
    pub thread_id: String,
}

impl EditorThreadInput {
    /// Creates a thread input without changing either supplied identity.
    #[must_use]
    pub fn new(thread_id: impl Into<String>, primary_project: Option<EditorProjectInput>) -> Self {
        Self {
            primary_project,
            thread_id: thread_id.into(),
        }
    }
}

/// The exact target shape selected by the legacy editor policy.
///
/// A detached thread has only path and the "thread" target type. A thread
/// with a primary project has path, the "editor" target type, and the
/// unmodified project identifier as `workspace_id`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorRouteTarget {
    /// Fall back to the canonical thread route when no primary project exists.
    Thread {
        /// Canonical thread route path.
        path: String,
    },
    /// Open the editor in the thread's authoritative workspace.
    Editor {
        /// Canonical editor route path.
        path: String,
        /// Exact project identifier supplied by the thread projection.
        workspace_id: String,
    },
}

impl EditorRouteTarget {
    /// Returns the exact legacy target discriminator.
    #[must_use]
    pub const fn target_type(&self) -> &'static str {
        match self {
            Self::Thread { .. } => "thread",
            Self::Editor { .. } => "editor",
        }
    }

    /// Borrows the canonical path in either target variant.
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Thread { path } | Self::Editor { path, .. } => path,
        }
    }

    /// Borrows the editor workspace identity, when this is an editor target.
    #[must_use]
    pub fn workspace_id(&self) -> Option<&str> {
        match self {
            Self::Thread { .. } => None,
            Self::Editor { workspace_id, .. } => Some(workspace_id),
        }
    }
}

/// Builds the canonical editor URL for one workspace/thread pair.
///
/// The thread route id removes one leading thread_ prefix when that leaves a
/// non-empty id, matching `ThreadRouteId`. A present file is always emitted as
/// ?file=..., including Some(""); None omits the query entirely. Components
/// are encoded independently and no caller text is otherwise normalized.
#[must_use]
pub fn editor_route_path(workspace_id: &str, thread_id: &str, file: Option<&str>) -> String {
    let file_query = file.map_or_else(String::new, |file| {
        format!("?file={}", encode_uri_component(file))
    });

    format!(
        "/e/{}/{}{}",
        encode_uri_component(workspace_id),
        encode_uri_component(thread_route_id(thread_id)),
        file_query,
    )
}

/// Selects the canonical thread or editor target for an authoritative thread.
///
/// A thread without a primary project falls back to the thread route and does
/// not carry a file query. A project-backed thread produces the editor target,
/// retaining the exact project identifier in `workspace_id` while using the
/// same encoded value in its path.
#[must_use]
pub fn editor_route_target_for_thread(
    thread: &EditorThreadInput,
    file: Option<&str>,
) -> EditorRouteTarget {
    match thread.primary_project.as_ref() {
        None => EditorRouteTarget::Thread {
            path: thread_route_path(None, &thread.thread_id),
        },
        Some(project) => EditorRouteTarget::Editor {
            path: editor_route_path(&project.project_id, &thread.thread_id, file),
            workspace_id: project.project_id.clone(),
        },
    }
}

/// Applies the legacy `ThreadRouteId` projection to one thread identity.
fn thread_route_id(thread_id: &str) -> &str {
    let route_id = thread_id
        .strip_prefix(LEGACY_THREAD_PREFIX)
        .unwrap_or(thread_id);
    if route_id.is_empty() {
        thread_id
    } else {
        route_id
    }
}

/// Builds the canonical thread URL used by the detached-target fallback.
fn thread_route_path(workspace_id: Option<&str>, thread_id: &str) -> String {
    format!(
        "/t/{}/{}",
        encode_uri_component(workspace_id.unwrap_or(DETACHED_WORKSPACE_ROUTE_ID)),
        encode_uri_component(thread_route_id(thread_id)),
    )
}

/// Encodes one URL component exactly like JavaScript encodeURIComponent.
fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if is_uri_component_safe(byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

/// Returns whether an ASCII byte is unescaped by encodeURIComponent.
const fn is_uri_component_safe(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')'
    )
}
