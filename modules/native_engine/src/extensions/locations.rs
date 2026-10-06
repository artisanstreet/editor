//! Where each managed engine reads skills and its global prompt.
//!
//! Every location is inside the engine's Artisan-managed home under the
//! Forge state root, never the user's own home: the Forge launches engines
//! with `HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, or `XDG_CONFIG_HOME`
//! pointing there (see [`crate::build_environment`] and the `OpenCode2`
//! profile environment), so these are exactly the paths each engine reads.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ManagedEngine;

/// Why an engine cannot receive extensions on this Forge.
pub const CURSOR_UNAVAILABLE_REASON: &str = "Cursor runs are not available in Artisan yet.";

/// Whether an engine can receive extensions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnginePresence {
    /// Installed: Artisan keeps its homes in sync.
    Installed,
    /// Not installed yet.
    Absent,
    /// Cannot run on this Forge; the reason is presentation-ready.
    Unavailable(&'static str),
}

/// The extension locations of one engine home.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineHomeLocations {
    /// The home itself.
    pub home: PathBuf,
    /// The folder the engine discovers skills in.
    pub skills_dir: PathBuf,
    /// The engine's global prompt file.
    pub prompt_file: PathBuf,
    /// Files the engine reads instead of `prompt_file` when present (Codex's
    /// `AGENTS.override.md`), which Artisan retires into its backups.
    pub prompt_overrides: Vec<PathBuf>,
}

/// One engine's presence and homes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineSlot {
    /// The engine.
    pub engine: ManagedEngine,
    /// Whether it can receive extensions.
    pub presence: EnginePresence,
    /// Its homes (`OpenCode2` has one per profile); empty unless installed.
    pub homes: Vec<EngineHomeLocations>,
    /// The global prompt file relative to a home, for display.
    pub prompt_display: &'static str,
}

/// Returns every managed engine's slot in [`ManagedEngine::ALL`] order.
#[must_use]
pub fn engine_slots(state_root: &Path) -> Vec<EngineSlot> {
    ManagedEngine::ALL
        .into_iter()
        .map(|engine| engine_slot(state_root, engine))
        .collect()
}

fn engine_slot(state_root: &Path, engine: ManagedEngine) -> EngineSlot {
    let prompt_display = prompt_display(engine);
    if engine == ManagedEngine::Cursor {
        return EngineSlot {
            engine,
            presence: EnginePresence::Unavailable(CURSOR_UNAVAILABLE_REASON),
            homes: Vec::new(),
            prompt_display,
        };
    }
    let engine_root = state_root.join("toolchain").join(engine.id());
    if !installed(&engine_root) {
        return EngineSlot {
            engine,
            presence: EnginePresence::Absent,
            homes: Vec::new(),
            prompt_display,
        };
    }
    let homes = if engine == ManagedEngine::OpenCode2 {
        opencode_homes(&engine_root)
    } else {
        vec![engine_root.join("home")]
    };
    EngineSlot {
        engine,
        presence: EnginePresence::Installed,
        homes: homes
            .into_iter()
            .map(|home| home_locations(engine, home))
            .collect(),
        prompt_display,
    }
}

/// An engine is installed once the Forge activated a generation or created
/// its home.
fn installed(engine_root: &Path) -> bool {
    let has_entries =
        |path: PathBuf| fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some());
    has_entries(engine_root.join("versions")) || engine_root.join("home").is_dir()
}

/// The primary home and every named profile home of `OpenCode2`.
fn opencode_homes(engine_root: &Path) -> Vec<PathBuf> {
    let mut homes = vec![engine_root.join("home")];
    if let Ok(entries) = fs::read_dir(engine_root.join("homes")) {
        let mut named: Vec<PathBuf> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect();
        named.sort();
        homes.extend(named);
    }
    homes
}

fn home_locations(engine: ManagedEngine, home: PathBuf) -> EngineHomeLocations {
    let (skills_dir, prompt_file, prompt_overrides) = match engine {
        ManagedEngine::Claude => {
            let config = home.join(".claude");
            (config.join("skills"), config.join("CLAUDE.md"), Vec::new())
        }
        ManagedEngine::Codex => {
            let config = home.join(".codex");
            (
                config.join("skills"),
                config.join("AGENTS.md"),
                vec![config.join("AGENTS.override.md")],
            )
        }
        ManagedEngine::Grok => {
            let config = home.join(".grok");
            (config.join("skills"), config.join("AGENTS.md"), Vec::new())
        }
        ManagedEngine::OpenCode2 => {
            let config = home.join("config").join("opencode");
            (config.join("skills"), config.join("AGENTS.md"), Vec::new())
        }
        ManagedEngine::Cursor => {
            let config = home.join(".cursor");
            (config.join("skills"), config.join("AGENTS.md"), Vec::new())
        }
    };
    EngineHomeLocations {
        home,
        skills_dir,
        prompt_file,
        prompt_overrides,
    }
}

const fn prompt_display(engine: ManagedEngine) -> &'static str {
    match engine {
        ManagedEngine::Claude => ".claude/CLAUDE.md",
        ManagedEngine::Codex => ".codex/AGENTS.md",
        ManagedEngine::Grok => ".grok/AGENTS.md",
        ManagedEngine::OpenCode2 => "config/opencode/AGENTS.md",
        ManagedEngine::Cursor => "Cursor user rules",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_follow_installed_engines_and_their_homes() {
        let state = tempfile::tempdir().unwrap();
        let toolchain = state.path().join("toolchain");
        fs::create_dir_all(toolchain.join("claude").join("home")).unwrap();
        fs::create_dir_all(toolchain.join("codex").join("versions").join("0.1.0")).unwrap();
        fs::create_dir_all(toolchain.join("opencode2").join("homes").join("work")).unwrap();
        fs::create_dir_all(toolchain.join("opencode2").join("home")).unwrap();
        let slots = engine_slots(state.path());
        assert_eq!(slots.len(), ManagedEngine::ALL.len());
        let slot = |engine| slots.iter().find(|slot| slot.engine == engine).unwrap();

        let claude = slot(ManagedEngine::Claude);
        assert_eq!(claude.presence, EnginePresence::Installed);
        assert_eq!(
            claude.homes[0].skills_dir,
            toolchain.join("claude/home/.claude/skills")
        );
        assert_eq!(
            slot(ManagedEngine::Codex).homes[0].prompt_overrides,
            vec![toolchain.join("codex/home/.codex/AGENTS.override.md")]
        );
        assert_eq!(slot(ManagedEngine::Grok).presence, EnginePresence::Absent);
        assert_eq!(
            slot(ManagedEngine::Cursor).presence,
            EnginePresence::Unavailable(CURSOR_UNAVAILABLE_REASON)
        );
        let opencode = slot(ManagedEngine::OpenCode2);
        assert_eq!(
            opencode
                .homes
                .iter()
                .map(|home| home.prompt_file.clone())
                .collect::<Vec<_>>(),
            vec![
                toolchain.join("opencode2/home/config/opencode/AGENTS.md"),
                toolchain.join("opencode2/homes/work/config/opencode/AGENTS.md"),
            ]
        );
    }
}
