//! Bounded model discovery for installed engines.
//! Failed or absent engines contribute no models; no bundled model fallback exists.
//! Only model metadata is retained, never prompts or credentials.

mod claude;
mod codex;
mod cursor;
mod grok;
mod opencode2;
mod process;

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use claude::discover_claude;
use codex::discover_codex;
use cursor::discover_cursor;
use grok::discover_grok;
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

fn cache() -> &'static Cache {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Returns the current discovery bundle, probing all engines when the cache is
/// cold or stale. Probes run concurrently and each adapter is independently
/// bounded; an adapter that fails contributes nothing.
pub(crate) async fn discovery_bundle() -> Arc<DiscoveryBundle> {
    let mut guard = cache().lock().await;
    if let Some((observed, ttl, bundle)) = guard.as_ref()
        && observed.elapsed() < *ttl
    {
        return Arc::clone(bundle);
    }

    let cursor_program = engine_executable("ARTISAN_CURSOR_EXECUTABLE", &["cursor-agent", "agent"]);
    let grok_program = engine_executable("ARTISAN_GROK_EXECUTABLE", &["grok"]);
    let opencode2_program = engine_executable("ARTISAN_OPENCODE2_EXECUTABLE", &["opencode2"]);
    let (codex, claude, opencode2, cursor, grok) = tokio::join!(
        discover_codex(),
        discover_claude(),
        discover_opencode2(opencode2_program.as_deref()),
        discover_cursor(cursor_program.as_deref()),
        discover_grok(grok_program.as_deref()),
    );

    let mut retry_needed = false;
    let mut models = Vec::new();
    let mut probed_engines = Vec::new();
    let mut missing_engines = Vec::new();
    for (engine_id, program, rows) in [
        ("codex", None, codex),
        ("claude", None, claude),
        ("opencode2", opencode2_program, opencode2),
        ("cursor", cursor_program, cursor),
        ("grok", grok_program, grok),
    ] {
        match rows {
            Some(rows) => {
                probed_engines.push(engine_id);
                models.extend(rows);
            }
            // A CLI-backed engine whose executable does not resolve is
            // definitively not installed; a failed probe is retried promptly.
            None if program.is_none() && matches!(engine_id, "opencode2" | "cursor" | "grok") => {
                missing_engines.push(engine_id);
            }
            None => {
                retry_needed = true;
                // Retain only previously discovered runtime rows during a
                // transient failure. A successful probe replaces them.
                if let Some((_, _, previous)) = guard.as_ref() {
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
    });
    let ttl = if retry_needed {
        Duration::from_secs(10)
    } else {
        DISCOVERY_TTL
    };
    *guard = Some((Instant::now(), ttl, Arc::clone(&bundle)));
    bundle
}

/// Resolves one engine executable: an explicit `ARTISAN_<ENGINE>_EXECUTABLE`
/// override, then the first candidate found on `PATH`. Windows resolves
/// `.exe` images before `.cmd`/`.bat` shims; `run_bounded` routes the latter
/// through `cmd.exe`.
fn engine_executable(env_var: &str, candidates: &[&str]) -> Option<String> {
    if let Ok(value) = std::env::var(env_var) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }
    candidates.iter().find_map(|name| resolve_program(name))
}

/// Finds one program on `PATH`, returning the full path so batch shims keep
/// their `.cmd` routing. Extensionless shell scripts are skipped on Windows
/// because they are not executable images.
fn resolve_program(name: &str) -> Option<String> {
    if name.contains(['/', '\\']) {
        return std::path::Path::new(name)
            .is_file()
            .then(|| name.to_owned());
    }
    let path = std::env::var_os("PATH")?;
    let extensions: &[&str] = if cfg!(windows) {
        &["exe", "cmd", "bat"]
    } else {
        &[""]
    };
    for directory in std::env::split_paths(&path) {
        for extension in extensions {
            let candidate = if extension.is_empty() {
                directory.join(name)
            } else {
                directory.join(format!("{name}.{extension}"))
            };
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
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
            "hermes is not part of the bundled manifest"
        );
    }
}
