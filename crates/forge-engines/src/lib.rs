//! Engine adapters.
//!
//! Each adapter maps one engine's wiring onto the item model in
//! `artisan-api` (see `ARCHITECTURE.md`, "Engines and the item model").
//! Decoding is a pure step (`feed(line) -> Result<Vec<EngineEvent>>`) separate from
//! process I/O, so recorded transcripts test it directly.

mod account;
mod acp;
mod catalog;
mod claude;
mod codex;
mod commands;
mod customise;
mod engines;
mod harness;
mod install;
mod manager;
mod mentions;
mod opencode;
pub mod process;
mod question;
mod reasoning;
mod retained;
mod rpc;
mod trace;
#[cfg(test)]
mod transcript_tests;
mod usage;

pub use account::{SignIn, check as check_sign_in};
pub use commands::{PromptCommand, normalize_command};
#[cfg(feature = "legacy-import")]
pub use customise::import_legacy_extensions;
pub use customise::{
    Customisation, McpServer, PromptApplied, PromptFile, adopt_skill, apply_prompt, enabled_skills,
    inspect_prompt, materialize, merge_opencode_config, normalize_prompt, prompt_digest,
    read_prompt_file, reconcile_skills, skill_catalog,
};
pub use harness::*;
pub use manager::{BinaryLease, EngineChange, Engines};
pub use mentions::with_skill_mentions;
pub use usage::{UsageCache, has_usage_reader, read_usage};

use artisan_api::EngineId;
use std::sync::Arc;

/// The adapter for an engine.
#[must_use]
pub fn harness(engine: EngineId) -> Arc<dyn Harness> {
    match engine {
        EngineId::Claude => Arc::new(claude::ClaudeHarness),
        EngineId::Grok => Arc::new(acp::AcpHarness {
            profile: acp::Profile::Grok,
        }),
        EngineId::Cursor => Arc::new(acp::AcpHarness {
            profile: acp::Profile::Cursor,
        }),
        EngineId::OpenCode => Arc::new(opencode::OpenCodeHarness),
        EngineId::Codex => Arc::new(codex::CodexHarness),
    }
}
