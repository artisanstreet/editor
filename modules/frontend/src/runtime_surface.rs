//! Pure host-surface and renderer-bootstrap telemetry projections.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/browser/runtime-surface.ts` and the value
//! derivation in `modules/frontend/src/lib/telemetry/product-bootstrap.ts`.
//! Host inspection is deliberately limited to caller-supplied user-agent
//! text, and lifecycle timing is deliberately limited to caller-supplied
//! millisecond values. No clock, browser API, process, telemetry effect, or
//! layer is accessed here.

#![allow(clippy::module_name_repetitions)]

/// The two host surfaces that can render the frontend.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RuntimeSurface {
    /// The bundled Electron renderer.
    Desktop,
    /// A browser renderer connected to a remote Forge.
    Browser,
}

impl RuntimeSurface {
    /// Returns the exact surface label used by the host projection.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Browser => "browser",
        }
    }

    /// Returns the exact Forge-connection value for renderer bootstrap.
    #[must_use]
    pub const fn forge_connection(self) -> &'static str {
        match self {
            Self::Desktop => "local",
            Self::Browser => "remote",
        }
    }

    /// Returns the exact telemetry surface value for renderer bootstrap.
    #[must_use]
    pub const fn telemetry_surface(self) -> &'static str {
        match self {
            Self::Desktop => "desktop_renderer",
            Self::Browser => "browser_renderer",
        }
    }
}

/// Identifies the renderer host using the same case-sensitive substring test
/// as `RuntimeSurfaceFor` in the TypeScript source.
#[must_use]
pub fn runtime_surface_for(user_agent: &str) -> RuntimeSurface {
    if user_agent.contains("Electron/") {
        RuntimeSurface::Desktop
    } else {
        RuntimeSurface::Browser
    }
}

/// Whether the host is the macOS desktop renderer.
///
/// Both predicates are required: a browser user agent may contain
/// `Macintosh`, but it is still a browser surface unless it also contains the
/// exact case-sensitive `Electron/` marker.
#[must_use]
pub fn is_mac_desktop(user_agent: &str) -> bool {
    runtime_surface_for(user_agent) == RuntimeSurface::Desktop && user_agent.contains("Macintosh")
}

/// Maximum renderer-bootstrap ready time accepted by the telemetry contract.
pub const MAX_TIME_TO_READY_MS: u64 = 600_000;

/// Exact event name emitted by the renderer bootstrap projection.
const EDITOR_SESSION_STARTED_EVENT: &str = "editor_session_started";

/// The typed value projection for the `editor_session_started` intent.
///
/// The string fields are fixed literals from the telemetry contract rather
/// than caller-provided data. The caller supplies only the host user-agent and
/// the two observed millisecond values.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EditorSessionStartedTelemetryIntent {
    /// Exact event discriminator.
    pub event: &'static str,
    /// Exact local/remote Forge-connection value.
    pub forge_connection: &'static str,
    /// Exact renderer telemetry surface value.
    pub surface: &'static str,
    /// Ready time clamped to `0..=MAX_TIME_TO_READY_MS`.
    pub time_to_ready_ms: u64,
}

/// Clamps the difference between ready and started millisecond values to the
/// renderer telemetry contract's inclusive range.
///
/// The widened subtraction mirrors `ready - started` while avoiding signed
/// overflow for synthetic or malformed extreme inputs. No clock is read.
#[allow(clippy::cast_lossless, clippy::cast_possible_truncation)]
#[must_use]
pub const fn time_to_ready_ms(started_at_ms: i64, ready_at_ms: i64) -> u64 {
    let elapsed_ms = ready_at_ms as i128 - started_at_ms as i128;
    if elapsed_ms <= 0 {
        0
    } else if elapsed_ms >= MAX_TIME_TO_READY_MS as i128 {
        MAX_TIME_TO_READY_MS
    } else {
        elapsed_ms as u64
    }
}

/// Projects the pure value portion of renderer telemetry bootstrap.
///
/// This corresponds to the object passed to `TelemetryController.Capture` in
/// `product-bootstrap.ts`; effect execution and the two `Date.now()` reads
/// remain outside this pure function.
#[must_use]
pub fn editor_session_started_telemetry(
    user_agent: &str,
    started_at_ms: i64,
    ready_at_ms: i64,
) -> EditorSessionStartedTelemetryIntent {
    let surface = runtime_surface_for(user_agent);
    EditorSessionStartedTelemetryIntent {
        event: EDITOR_SESSION_STARTED_EVENT,
        forge_connection: surface.forge_connection(),
        surface: surface.telemetry_surface(),
        time_to_ready_ms: time_to_ready_ms(started_at_ms, ready_at_ms),
    }
}
