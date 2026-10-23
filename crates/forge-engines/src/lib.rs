//! Engine adapters.
//!
//! Each adapter maps one engine's wiring onto the item model in
//! `artisan-api` (see `ARCHITECTURE.md`, "Engines and the item model").
//! Decoding is a pure step (`feed(line) -> Result<Vec<EngineEvent>>`) separate from
//! process I/O, so recorded transcripts test it directly.

mod account;
mod catalog;
pub(crate) mod claude;
pub mod claudeai;
mod codex;
mod commands;
mod compaction;
mod customise;
mod engines;
mod harness;
mod install;
mod manager;
mod marketplace;
pub mod mcp_oauth;
mod mentions;
mod native;
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
pub use claude::{CLAUDE_AI_TRANSPORT, CLAUDE_AI_UNAVAILABLE};
pub use commands::normalize_command;
pub use compaction::{COMPACTION_NUDGE, compaction_prompt, is_compaction_summary};
#[cfg(feature = "legacy-import")]
pub use customise::import_legacy_extensions;
pub use customise::{
    ARTISAN_MCP, COMPUTER_MCP, Customisation, McpServer, PromptApplied, PromptFile, adopt_skill,
    apply_prompt, discard_skill, enabled_skills, inspect_prompt, install_skill, materialize,
    merge_opencode_config, normalize_prompt, prompt_digest, read_prompt_file, reconcile_skills,
    skill_catalog, with_gateway_mcp,
};
pub use harness::*;
pub use manager::{BinaryLease, EngineChange, Engines};
pub use marketplace::{SKILLS_SH_URL, fetch_skill, search_skills};
pub use mentions::{with_skill_mentions, with_thread_mentions};
pub use usage::{UsageCache, has_usage_reader, read_usage};

use artisan_api::EngineId;
use std::sync::Arc;

/// The adapter for an engine.
#[must_use]
pub fn harness(engine: EngineId) -> Arc<dyn Harness> {
    match engine {
        EngineId::Claude => Arc::new(claude::ClaudeHarness),
        EngineId::OpenCode => Arc::new(opencode::OpenCodeHarness),
        EngineId::Codex => Arc::new(codex::CodexHarness),
    }
}
