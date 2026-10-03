//! Bounded model discovery for installed engines.
//! Failed or absent engines contribute no models; no bundled model fallback exists.
//! Only model metadata is retained, never prompts or credentials.

mod claude;
mod codex;
mod cursor;
mod grok;
mod opencode2;
mod process;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use artisan_native_engine::ManagedEngine;

use tokio::sync::Mutex;

use claude::discover_claude;
use codex::discover_codex;
use cursor::discover_cursor;
use grok::{GrokListing, discover_grok};
use opencode2::discover_opencode2;

/// How long one discovery snapshot is served before re-probing.
pub(crate) const DISCOVERY_TTL: Duration = Duration::from_secs(300);

/// Per-engine probe deadline. The slowest adapter is the Codex app-server
/// handshake; the whole bundle is bounded by `DISCOVERY_DEADLINE`.
const ENGINE_DEADLINE: Duration = Duration::from_secs(6);

/// One live row reported by an engine.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the row carries independent provider capability bits; grouping them into a nested struct would not reduce ambiguity"
)]
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
    /// Optional engine variant id (`OpenCode2` reasoning level).
    pub(crate) variant_id: Option<String>,
    /// Display name reported by the engine.
    pub(crate) name: String,
    /// Optional description reported by the engine.
    pub(crate) description: Option<String>,
    /// Whether the engine hides this row from its own default picker.
    /// Hidden rows are engine internals and are never surfaced in the
    /// catalogue, but they still count as reported by the probe.
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
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DiscoveryBundle {
    /// Rows from every engine that answered, in engine order.
    pub(crate) models: Vec<DiscoveredModel>,
    /// Engines whose probe completed successfully (empty probes included).
    pub(crate) probed_engines: Vec<&'static str>,
    /// Engines whose CLI is not installed on this machine; their harnesses
    /// are hidden from the picker until the engine appears.
    pub(crate) missing_engines: Vec<&'static str>,
    /// Installed engines whose CLI reported it is signed out. They report no
    /// models and are not runnable, so the picker greys their tab out.
    pub(crate) unauthenticated_engines: Vec<&'static str>,
}

impl DiscoveryBundle {
    /// Returns rows for one engine in reported order.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn for_engine(&self, engine_id: &str) -> Vec<&DiscoveredModel> {
        self.models
            .iter()
            .filter(|model| model.engine_id == engine_id)
            .collect()
    }
}

type Cache = Mutex<Option<(Instant, Duration, Arc<DiscoveryBundle>)>>;

static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();

/// Admits one probe at a time: a cold read waits for it, a stale read only
/// starts one when none is running.
static PROBE: Mutex<()> = Mutex::const_new(());

fn cache() -> &'static Cache {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// The context window the catalog lists for one engine's model, from the
/// bundle already cached; never probes, so a usage read never waits on an
/// engine process.
pub(crate) async fn cached_context_window(engine_id: &str, model_id: &str) -> Option<u64> {
    let (_, _, bundle) = cache().lock().await.clone()?;
    bundle
        .models
        .iter()
        .find(|model| model.engine_id == engine_id && model.native_model_id == model_id)
        .and_then(|model| model.context_window_tokens)
}

/// Returns the current discovery bundle.
///
/// Once any probe has completed this never waits on engine processes: a
/// stale bundle is served as is while one background probe refreshes it,
/// so catalog reads and sends (which resolve their model against the
/// catalog) answer immediately. Only a cold Forge waits, once, for its
/// first probe. Probes run concurrently, each adapter independently
/// bounded; an adapter that fails contributes nothing.
pub(crate) async fn discovery_bundle() -> Arc<DiscoveryBundle> {
    if let Some((observed, ttl, bundle)) = cache().lock().await.clone() {
        if observed.elapsed() >= ttl {
            refresh_in_background();
        }
        return bundle;
    }
    let _probe = PROBE.lock().await;
    // A concurrent cold read may have probed while this one waited.
    if let Some((_, _, bundle)) = cache().lock().await.clone() {
        return bundle;
    }
    probe_and_store().await
}

/// Warms discovery at Forge startup so the first catalog read rarely waits:
/// reads that arrive meanwhile join this probe instead of starting their own.
pub(crate) fn warm_in_background() {
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async {
            let _ = discovery_bundle().await;
        });
    }
}

/// Starts a background probe unless one is already running. Outside a Tokio
/// runtime the stale bundle simply keeps serving until the next read.
fn refresh_in_background() {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let Ok(probe) = PROBE.try_lock() else {
        return;
    };
    drop(probe);
    runtime.spawn(async {
        let Ok(_probe) = PROBE.try_lock() else {
            return;
        };
        let stale = cache()
            .lock()
            .await
            .as_ref()
            .is_none_or(|(observed, ttl, _)| observed.elapsed() >= *ttl);
        if stale {
            probe_and_store().await;
        }
    });
}

/// Probes every engine and stores the bundle. The cache lock is never held
/// across the probes, so readers keep answering from the previous bundle.
async fn probe_and_store() -> Arc<DiscoveryBundle> {
    let previous = cache()
        .lock()
        .await
        .as_ref()
        .map(|(_, _, bundle)| Arc::clone(bundle));
    let codex_program = managed_program(ManagedEngine::Codex);
    let cursor_program = managed_program(ManagedEngine::Cursor);
    let grok_program = managed_program(ManagedEngine::Grok);
    let opencode2_program = managed_program(ManagedEngine::OpenCode2);
    let (codex, claude, opencode2, cursor, grok) = tokio::join!(
        discover_codex(codex_program.as_ref()),
        discover_claude(managed_home(ManagedEngine::Claude)),
        discover_opencode2(opencode2_program.as_ref()),
        discover_cursor(cursor_program.as_ref()),
        discover_grok(grok_program.as_ref()),
    );

    let mut retry_needed = false;
    let mut models = Vec::new();
    let mut probed_engines = Vec::new();
    let mut missing_engines = Vec::new();
    let mut unauthenticated_engines = Vec::new();
    let grok = grok.map(|listing| match listing {
        GrokListing::Models(rows) => rows,
        GrokListing::SignedOut => {
            unauthenticated_engines.push("grok");
            Vec::new()
        }
    });
    for (engine_id, installed, rows) in [
        ("codex", codex_program.is_some(), codex),
        ("claude", true, claude),
        ("opencode2", opencode2_program.is_some(), opencode2),
        ("cursor", cursor_program.is_some(), cursor),
        ("grok", grok_program.is_some(), grok),
    ] {
        match rows {
            Some(rows) => {
                probed_engines.push(engine_id);
                models.extend(rows);
            }
            // An engine the Forge has not installed is definitively missing;
            // a failed probe of an installed engine is retried promptly.
            None if !installed => {
                missing_engines.push(engine_id);
            }
            None => {
                retry_needed = true;
                // Retain only previously discovered runtime rows during a
                // transient failure. A successful probe replaces them.
                if let Some(previous) = previous.as_ref() {
                    models.extend(
                        previous
                            .models
                            .iter()
                            .filter(|model| model.engine_id == engine_id)
                            .cloned(),
                    );
                }
            }
        }
    }

    let bundle = Arc::new(DiscoveryBundle {
        models,
        probed_engines,
        missing_engines,
        unauthenticated_engines,
    });
    let ttl = if retry_needed {
        Duration::from_secs(10)
    } else {
        DISCOVERY_TTL
    };
    *cache().lock().await = Some((Instant::now(), ttl, Arc::clone(&bundle)));
    bundle
}

/// A Forge-managed engine program: the verified executable, its complete
/// environment, and its private home. Discovery never looks on `PATH`.
pub(super) struct EngineProgram {
    pub(super) executable: PathBuf,
    pub(super) environment: Vec<(OsString, OsString)>,
    pub(super) home: PathBuf,
}

/// Resolves one managed engine for discovery; `None` when it is not
/// installed on this Forge (or not supported on this platform).
fn managed_program(engine: ManagedEngine) -> Option<EngineProgram> {
    let target = artisan_native_engine::resolve_launch_target(engine).ok()?;
    Some(EngineProgram {
        executable: target.executable().to_path_buf(),
        environment: target.environment().ok()?,
        home: target.home(),
    })
}

/// Returns the managed engine home even when the engine is not installed,
/// so cached catalogues written by an earlier install stay readable.
fn managed_home(engine: ManagedEngine) -> Option<PathBuf> {
    let database = artisan_native_engine::managed_database()?;
    Some(artisan_native_engine::engine_home(
        database.parent()?,
        engine,
    ))
}

/// Maps an engine effort spelling to the Artisan level identifier.
fn artisan_level(effort: &str) -> Option<&'static str> {
    match effort {
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
        for engine in ["codex", "claude", "opencode2", "cursor", "grok"] {
            let rows = bundle.for_engine(engine);
            println!(
                "{engine}: {} rows probed={} missing={}",
                rows.len(),
                bundle.probed_engines.contains(&engine),
                bundle.missing_engines.contains(&engine)
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
        let catalog = crate::native_model_catalog::from_discovery(&bundle)
            .expect("the live discovery bundle must build a wire-valid catalog");
        let opencode2_rows = catalog
            .manifest
            .models
            .iter()
            .filter(|model| model.harness == "opencode2")
            .count();
        assert!(
            opencode2_rows > 0,
            "the opencode2 CLI is expected on this development host"
        );
        assert!(
            catalog.manifest.harness("hermes").is_none(),
            "hermes is not a shipped harness"
        );
    }
}
