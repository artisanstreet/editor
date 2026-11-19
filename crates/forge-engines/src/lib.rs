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
mod provider;
mod question;
mod reasoning;
mod retained;
mod rpc;
mod sandbox;
pub mod trace;
#[cfg(test)]
#[path = "../tests/transcript/unit.rs"]
mod transcript_tests;
mod usage;

pub use account::{SignIn, SubmitCodeError};
pub use claude::{CLAUDE_AI_TRANSPORT, CLAUDE_AI_UNAVAILABLE};
pub use commands::{MissingPrompt, normalize_command};
pub use compaction::{COMPACTION_NUDGE, compaction_prompt, is_compaction_summary};
pub(crate) use customise::materialize;
pub use customise::{
    ARTISAN_MCP, COMPUTER_MCP, Customisation, McpServer, PromptApplied, PromptFile, adopt_skill,
    apply_prompt, discard_skill, enabled_skills, inspect_prompt, install_skill, normalize_prompt,
    prompt_digest, read_prompt_file, reconcile_skills, skill_catalog, with_gateway_mcp,
};
pub use customise::{
    AdoptSkillError, ApplyPromptError, DiscardSkillError, FindSkillError, ImportError,
    InstallSkillError, LaunchError, MaterializeError, ReadSkillsError, ReconcileSkillsError,
    RecoverSkillError,
};
pub use engines::host_entry;
pub use harness::{
    ClearGoalError, EngineEvent, Failure, Harness, Input, Launch, LiveKind, Outcome, Retry, RunIo,
    SetTitleError, SideQueryError, SideReply, SideUsage, Turn, UserInput,
};
pub use manager::{BinaryLease, EngineChange, Engines};
pub use manager::{
    ClaudeAiSignInError, InstallError, ListVersionsError, NoBuild, NotInstalled,
    ValidateVersionError,
};
pub use marketplace::{
    FetchSkillError, SKILLS_SH_URL, SearchSkillsError, fetch_skill, search_skills,
};
pub use mentions::{with_skill_mentions, with_thread_mentions};
pub use usage::UsageCache;
pub(crate) use usage::has_usage_reader;

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
