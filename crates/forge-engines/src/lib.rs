#![expect(
    clippy::allow_attributes,
    clippy::inconsistent_struct_constructor,
    clippy::iter_over_hash_type,
    clippy::let_underscore_untyped,
    clippy::partial_pub_fields,
    clippy::renamed_function_params,
    clippy::string_slice,
    clippy::struct_field_names,
    clippy::unused_result_ok,
    elided_lifetimes_in_paths,
    let_underscore_drop,
    missing_debug_implementations,
    unreachable_pub,
    unused_qualifications,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![allow(
    clippy::assertions_on_result_states,
    clippy::exit,
    clippy::print_stderr,
    clippy::wildcard_imports,
    dead_code,
    unused_imports,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]
mod account;
mod catalog;
pub mod claude;
pub mod claudeai;
pub mod codex;
mod commands;
mod compaction;
mod customise;
mod engines;
mod expansion;
#[cfg(test)]
#[path = "../tests/fakes/support.rs"]
mod fake_engine;
mod harness;
mod install;
mod manager;
mod marketplace;
pub mod mcp_oauth;
mod mentions;
mod native;
pub mod opencode;
pub mod process;
mod question;
mod reasoning;
mod retained;
mod rpc;
pub mod trace;
#[cfg(test)]
mod transcript_tests;
mod usage;

pub use account::{SignIn, SubmitCodeError, check as check_sign_in};
pub use claude::{CLAUDE_AI_TRANSPORT, CLAUDE_AI_UNAVAILABLE};
pub use commands::{MissingPrompt, normalize_command};
pub use compaction::{COMPACTION_NUDGE, compaction_prompt, is_compaction_summary};
pub use customise::{
    ARTISAN_MCP, COMPUTER_MCP, Customisation, McpServer, PromptApplied, PromptFile, adopt_skill,
    apply_prompt, discard_skill, enabled_skills, inspect_prompt, install_skill, materialize,
    merge_opencode_config, normalize_prompt, prompt_digest, read_prompt_file, reconcile_skills,
    skill_catalog, with_gateway_mcp,
};
pub use customise::{
    AdoptSkillError, ApplyPromptError, DiscardSkillError, FindSkillError, ImportError,
    InstallSkillError, LaunchError, MaterializeError, ReadSkillsError, ReconcileSkillsError,
    RecoverSkillError,
};
pub use engines::host_entry;
pub use harness::*;
pub use manager::{BinaryLease, EngineChange, Engines};
pub use manager::{
    CheckUpdateError, ClaudeAiSignInError, InstallError, ListVersionsError, NoBuild, NotInstalled,
    RefreshModelsError, ValidateVersionError,
};
pub use marketplace::{
    FetchSkillError, SKILLS_SH_URL, SearchSkillsError, fetch_skill, search_skills,
};
pub use mentions::{with_skill_mentions, with_thread_mentions};
pub use usage::{UsageCache, has_usage_reader, read_usage};

use artisan_api::EngineId;
use std::sync::Arc;

#[must_use]
pub fn harness(engine: EngineId) -> Arc<dyn Harness> {
    match engine {
        EngineId::Claude => Arc::new(claude::ClaudeHarness),
        EngineId::OpenCode => Arc::new(opencode::OpenCodeHarness),
        EngineId::Codex => Arc::new(codex::CodexHarness),
    }
}
