//! Best-effort live model discovery for every installed/built-in engine.
//!
//! Each adapter probes one engine with a bounded deadline and returns raw,
//! engine-neutral rows. Probes never block a turn: a failed or absent engine
//! simply contributes no rows, and the static catalogue remains the
//! fallback. The overlay in `crate::native_model_catalog` merges these rows
//! onto the static manifest, preserving the harness-level policy (context
//! windows, speed economics, permissions) that discovery cannot report.
//!
//! Privacy: no prompt, credential, or file content crosses this module. Only
//! model metadata returned by the engines themselves is retained.

mod claude;
mod codex;
mod cursor;
mod grok;
mod process;

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use claude::discover_claude;
use codex::discover_codex;
use cursor::discover_cursor;
use grok::discover_grok;

/// How long one discovery snapshot is served before re-probing.
pub(crate) const DISCOVERY_TTL: Duration = Duration::from_secs(300);

/// Per-engine probe deadline. The slowest adapter is the Codex app-server
/// handshake; the whole bundle is bounded by `DISCOVERY_DEADLINE`.
const ENGINE_DEADLINE: Duration = Duration::from_secs(6);

/// One live row reported by an engine.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DiscoveredModel {
    /// Owning harness/engine identifier (`codex`, `claude`, `cursor`, `grok`).
    pub(crate) engine_id: &'static str,
    /// Provider identifier used by the catalogue.
    pub(crate) provider: String,
    /// Exact identifier handed to the engine.
    pub(crate) native_model_id: String,
    /// Optional upstream provider model id when it differs from the native id.
    pub(crate) upstream_model_id: Option<String>,
    /// Display name reported by the engine.
    pub(crate) name: String,
    /// Optional description reported by the engine.
    pub(crate) description: Option<String>,
    /// Whether the engine hides this row from its own default picker.
    pub(crate) hidden: bool,
    /// Whether the engine marks this row as its default.
    pub(crate) default: bool,
    /// Reported reasoning capability.
    pub(crate) thinking: DiscoveredThinking,
    /// Whether the engine reports a distinct fast delivery tier.
    pub(crate) fast: bool,
    /// Provider-reported context limit in tokens.
    pub(crate) context_window_tokens: Option<u64>,
    /// Provider-reported maximum context limit in tokens.
    pub(crate) max_context_window_tokens: Option<u64>,
    /// Maximum output tokens when reported.
    pub(crate) output_tokens: Option<u64>,
    /// Whether image input is accepted.
    pub(crate) image_input: bool,
    /// Whether tool use is accepted.
    pub(crate) tools: bool,
    /// Whether web search is accepted.
    pub(crate) web_search: bool,
    /// Optional USD-per-million input/output pricing.
    pub(crate) cost: Option<(f64, f64)>,
    /// Lifecycle status reported by the engine.
    pub(crate) status: &'static str,
    /// Provenance marker for every field in this row.
    pub(crate) metadata_confidence: &'static str,
}

/// Reasoning capability reported by an engine.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DiscoveredThinking {
    /// The engine reports no selectable reasoning option.
    Unavailable,
    /// Reasoning is engine-managed with no separate control.
    Native {
        /// Exact native-control description.
        description: String,
    },
    /// The engine reports selectable options.
    Supported {
        /// Default option identifier.
        default: String,
        /// Options in engine order.
        options: Vec<DiscoveredThinkingOption>,
    },
}

/// One selectable reasoning option reported by an engine.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DiscoveredThinkingOption {
    /// Artisan level identifier (`light`, `medium`, `high`, `xhigh`, `max`,
    /// `ultra`).
    pub(crate) id: String,
    /// Exact native value sent to the engine.
    pub(crate) native_value: String,
    /// Optional engine-provided copy.
    pub(crate) description: Option<String>,
    /// Economics classification (`standard`, `diminishing-returns`,
    /// `harness-orchestration`).
    pub(crate) economics: &'static str,
    /// Presentation group (`base` or `special`).
    pub(crate) presentation_group: &'static str,
}

/// One bounded discovery bundle.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DiscoveryBundle {
    /// Rows from every engine that answered, in engine order.
    pub(crate) models: Vec<DiscoveredModel>,
    /// Engines whose probe completed successfully (empty probes included).
    pub(crate) probed_engines: Vec<&'static str>,
}

impl DiscoveryBundle {
    /// Returns rows for one engine in reported order.
    #[must_use]
    pub(crate) fn for_engine(&self, engine_id: &str) -> Vec<&DiscoveredModel> {
        self.models
            .iter()
            .filter(|model| model.engine_id == engine_id)
            .collect()
    }
}

type Cache = Mutex<Option<(Instant, Arc<DiscoveryBundle>)>>;

static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();

fn cache() -> &'static Cache {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Returns the current discovery bundle, probing all engines when the cache is
/// cold or stale. Probes run concurrently and each adapter is independently
/// bounded; an adapter that fails contributes nothing.
pub(crate) async fn discovery_bundle() -> Arc<DiscoveryBundle> {
    let mut guard = cache().lock().await;
    if let Some((observed, bundle)) = guard.as_ref()
        && observed.elapsed() < DISCOVERY_TTL
    {
        return Arc::clone(bundle);
    }

    let (codex, claude, cursor, grok) = tokio::join!(
        discover_codex(),
        discover_claude(),
        discover_cursor(),
        discover_grok(),
    );

    let mut models = Vec::new();
    let mut probed_engines = Vec::new();
    for (engine_id, rows) in [
        ("codex", codex),
        ("claude", claude),
        ("cursor", cursor),
        ("grok", grok),
    ] {
        if let Some(rows) = rows {
            probed_engines.push(engine_id);
            models.extend(rows);
        }
    }

    let bundle = Arc::new(DiscoveryBundle {
        models,
        probed_engines,
    });
    *guard = Some((Instant::now(), Arc::clone(&bundle)));
    bundle
}

/// Returns the executable for one engine: an explicit `ARTISAN_<ENGINE>_
/// EXECUTABLE` override, then `fallback`.
fn engine_executable(env_var: &str, fallback: &str) -> String {
    std::env::var(env_var)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

/// Maps an engine effort spelling to the Artisan level identifier.
fn artisan_level(effort: &str) -> Option<&'static str> {
    match effort {
        "none" => None,
        "minimal" | "low" => Some("light"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" => Some("xhigh"),
        "max" => Some("max"),
        "ultra" => Some("ultra"),
        _ => None,
    }
}

/// Classifies one effort for presentation.
fn effort_economics(level: &str) -> (&'static str, &'static str) {
    match level {
        "max" => ("diminishing-returns", "special"),
        "ultra" => ("harness-orchestration", "special"),
        _ => ("standard", "base"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opt-in live probe used during development on a machine with the CLIs
    /// installed. Never runs in CI; asserts only what this host exposes.
    #[tokio::test]
    #[ignore = "requires locally installed engine CLIs and network access"]
    async fn live_discovery_smoke() {
        let bundle = discovery_bundle().await;
        for engine in ["codex", "claude", "cursor", "grok"] {
            let rows = bundle.for_engine(engine);
            println!(
                "{engine}: {} rows probed={}",
                rows.len(),
                bundle.probed_engines.contains(&engine)
            );
        }
        assert!(
            !bundle.for_engine("codex").is_empty(),
            "the codex CLI is expected on this development host"
        );
        assert!(
            !bundle.for_engine("claude").is_empty(),
            "the published Claude catalogue is expected to be reachable"
        );
    }
}
