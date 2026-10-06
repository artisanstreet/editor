//! Artisan-managed skills and the global prompt inside engine homes.
//!
//! The mechanics only: reading skill folders, the content-addressed store,
//! where each engine reads skills and its global prompt, the links that
//! place skills, and the prompt files. The Forge's backend decides what
//! belongs in each engine and records the outcome.

pub mod links;
pub mod locations;
pub mod prompt_files;
pub mod skill;
pub mod store;

pub use links::{
    LinkError, SkillEntry, inspect_entry, links_supported, move_to_backup, native_skill_dirs,
    owned_links, place_link, remove_link,
};
pub use locations::{
    CURSOR_UNAVAILABLE_REASON, EngineHomeLocations, EnginePresence, EngineSlot, engine_slots,
};
pub use prompt_files::{
    PromptFileError, back_up_prompt, normalize_prompt, prompt_digest, read_prompt, remove_prompt,
    write_prompt,
};
pub use skill::{
    SKILL_FILE, SKILL_MAX_BYTES, SKILL_MAX_FILES, SkillError, SkillFile, SkillPackage,
    parse_frontmatter, read_skill_dir,
};
pub use store::{ExtensionStore, StoreError};
