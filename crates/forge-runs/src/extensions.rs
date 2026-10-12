//! Skills and the global prompt across the engines (master's
//! `extension_sync`): what the Editor's Marketplace and Settings show and
//! change. The filesystem work is `forge_engines::customise`; this keeps
//! the user's choices in the store and serialises work on engine homes.

use crate::Hub;
use anyhow::{Context, Result};
use artisan_api::{
    CatalogSkill, EngineId, GlobalPrompt, PromptCandidate, PromptEngineState, PromptEngineStatus,
    ResolvePrompt, SavePrompt, SkillOrigin,
};
use forge_engines::{Customisation, PromptApplied};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Forge's setting holding the ids of the skills the user turned off.
pub const DISABLED_SKILLS_KEY: &str = "disabled_skills";
/// Forge's setting holding the global prompt (its latest version).
pub const GLOBAL_PROMPT_KEY: &str = "global_prompt";
/// The longest global prompt Forge keeps.
const PROMPT_MAX_BYTES: usize = 64 * 1024;

/// A request Forge turns down; the message is for the user.
#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

fn refused(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Refused(message.into()))
}

impl Hub {
    /// Every engine with its home when installed.
    fn engine_homes(&self) -> Vec<(EngineId, Option<PathBuf>)> {
        let engines = self.engines();
        EngineId::ALL
            .into_iter()
            .map(|engine| (engine, engines.binary(engine).map(|_| engines.home(engine))))
            .collect()
    }

    async fn disabled_skills(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .store()
            .setting(DISABLED_SKILLS_KEY)
            .await?
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default())
    }

    /// The skills part of what an engine gets: Artisan's own and the user's,
    /// less the ones turned off.
    async fn skill_customisation(&self) -> Result<Customisation> {
        Ok(Customisation {
            skills_dir: self.skills_dir(),
            first_party: true,
            disabled_skills: self.disabled_skills().await?,
            ..Customisation::default()
        })
    }

    /// What an engine gets at launch: the prompt (once the user has one),
    /// MCP servers, and the enabled skills.
    pub(crate) async fn customisation(&self) -> Result<Customisation> {
        Ok(Customisation {
            prompt: self.launch_prompt().await?,
            mcp_servers: self
                .store()
                .setting("mcp_servers")
                .await?
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default(),
            ..self.skill_customisation().await?
        })
    }

    /// Runs filesystem work on engine homes, one job at a time.
    async fn on_homes<R: Send + 'static>(
        &self,
        job: impl FnOnce() -> R + Send + 'static,
    ) -> Result<R> {
        let hub = self.clone();
        tokio::task::spawn_blocking(move || {
            let _homes = hub.homes_lock();
            job()
        })
        .await
        .context("engine home task")
    }

    /// Every skill, after linking the enabled ones into each installed
    /// engine.
    pub async fn skill_catalog(&self) -> Result<Vec<CatalogSkill>> {
        let customisation = self.skill_customisation().await?;
        let homes = self.engine_homes();
        self.on_homes(move || forge_engines::skill_catalog(&homes, &customisation))
            .await?
    }

    /// The skills a `$name` mention can name: the ones Forge puts into the
    /// engines.
    pub async fn enabled_skills(&self) -> Result<Vec<artisan_api::Skill>> {
        let customisation = self.skill_customisation().await?;
        self.on_homes(move || forge_engines::enabled_skills(&customisation))
            .await?
    }

    /// Turns a skill Forge manages on or off in every engine.
    pub async fn set_skill_enabled(&self, id: &str, enabled: bool) -> Result<Vec<CatalogSkill>> {
        let known = self
            .skill_catalog()
            .await?
            .into_iter()
            .any(|skill| skill.id == id && skill.origin != SkillOrigin::Discovered);
        if !known {
            return Err(refused("Artisan doesn't manage that skill."));
        }
        let mut disabled = self.disabled_skills().await?;
        if enabled {
            disabled.remove(id);
        } else {
            disabled.insert(id.to_owned());
        }
        self.store()
            .set_setting(DISABLED_SKILLS_KEY.to_owned(), serde_json::json!(disabled))
            .await?;
        self.skill_catalog().await
    }

    /// Copies a skill found in the engines into Forge's skills folder, so
    /// Forge keeps it in every engine.
    pub async fn adopt_skill(&self, id: &str) -> Result<Vec<CatalogSkill>> {
        let customisation = self.skill_customisation().await?;
        let homes = self.engine_homes();
        let id = id.to_owned();
        let adopted = self
            .on_homes(move || forge_engines::adopt_skill(&homes, &customisation, &id))
            .await?;
        if let Err(error) = adopted {
            // The customise layer phrases its refusals for the user.
            return Err(refused(format!("{error:#}")));
        }
        self.skill_catalog().await
    }

    /// The prompt engines get at launch; `None` while the user must still
    /// choose between the engines' differing prompts.
    async fn launch_prompt(&self) -> Result<Option<String>> {
        if let Some(text) = self.stored_prompt().await? {
            return Ok(Some(text));
        }
        let prompt = self.global_prompt().await?;
        Ok(prompt.candidates.is_empty().then_some(prompt.text))
    }

    async fn stored_prompt(&self) -> Result<Option<String>> {
        Ok(self
            .store()
            .setting(GLOBAL_PROMPT_KEY)
            .await?
            .and_then(|value| value.as_str().map(str::to_owned)))
    }

    /// The global prompt and where it stands in each engine, after bringing
    /// every engine not edited outside Artisan in line.
    pub async fn global_prompt(&self) -> Result<GlobalPrompt> {
        self.reconcile_prompt(None).await
    }

    async fn reconcile_prompt(&self, force: Option<EngineId>) -> Result<GlobalPrompt> {
        let homes = self.engine_homes();
        if self.stored_prompt().await?.is_none() {
            // Before Forge first writes, it takes what the engines have: one
            // shared text silently, differing ones as a choice for the user.
            let found = homes.clone();
            let mut candidates = self.on_homes(move || candidates(&found)).await?;
            if candidates.len() > 1 {
                return Ok(GlobalPrompt {
                    version: 0,
                    text: String::new(),
                    candidates,
                    engines: awaiting(&homes),
                });
            }
            let text = candidates
                .pop()
                .map(|candidate| candidate.text)
                .unwrap_or_default();
            // Taking it over isn't a save: no version. Another request may
            // have set it up meanwhile; that one stands.
            self.store()
                .set_setting_if_absent(GLOBAL_PROMPT_KEY.to_owned(), serde_json::json!(text))
                .await?;
        }
        let text = self.stored_prompt().await?.unwrap_or_default();
        let version = self.store().prompt_version().await?;
        let canonical = text.clone();
        let engines = self
            .on_homes(move || {
                homes
                    .iter()
                    .map(|(engine, home)| prompt_state(*engine, home.as_ref(), &canonical, force))
                    .collect()
            })
            .await?;
        Ok(GlobalPrompt {
            version,
            text,
            candidates: Vec::new(),
            engines,
        })
    }

    /// Saves a new version and writes it to every engine not edited outside
    /// Artisan.
    pub async fn save_global_prompt(&self, save: SavePrompt) -> Result<GlobalPrompt> {
        if save.text.len() > PROMPT_MAX_BYTES {
            return Err(refused("The global prompt is longer than 64 KB."));
        }
        if self
            .store()
            .save_prompt_version(save.base_version, save.text)
            .await?
            .is_err()
        {
            return Err(refused(
                "The global prompt changed since you started editing. Review the current version and save again.",
            ));
        }
        self.global_prompt().await
    }

    /// Settles the setup choice or an engine edited outside Artisan.
    pub async fn resolve_global_prompt(&self, resolution: ResolvePrompt) -> Result<GlobalPrompt> {
        let current = self.global_prompt().await?;
        let drifted = |engine: EngineId| {
            current
                .engines
                .iter()
                .find(|state| state.engine == engine && state.status == PromptEngineStatus::Drifted)
                .and_then(|state| state.external_text.clone())
                .ok_or_else(|| {
                    refused(format!(
                        "{}'s global prompt is no longer edited outside Artisan.",
                        engine.display_name()
                    ))
                })
        };
        match resolution {
            ResolvePrompt::Choose { candidate_id } => {
                let candidate = current
                    .candidates
                    .iter()
                    .find(|candidate| candidate.id == candidate_id)
                    .ok_or_else(|| {
                        if current.candidates.is_empty() {
                            refused("The global prompt is already set up.")
                        } else {
                            refused("That prompt is no longer in your engines.")
                        }
                    })?;
                self.store()
                    .save_prompt_version(current.version, candidate.text.clone())
                    .await?
                    .map_err(|_| refused("The global prompt was set up meanwhile."))?;
                self.global_prompt().await
            }
            ResolvePrompt::UseEngine { engine } => {
                let text = drifted(engine)?;
                self.store()
                    .save_prompt_version(current.version, text)
                    .await?
                    .map_err(|_| {
                        refused("The global prompt changed meanwhile. Review it and try again.")
                    })?;
                self.global_prompt().await
            }
            ResolvePrompt::RestoreEngine { engine } => {
                drifted(engine)?;
                self.reconcile_prompt(Some(engine)).await
            }
        }
    }
}

/// The distinct non-empty prompts the installed engines hold.
fn candidates(homes: &[(EngineId, Option<PathBuf>)]) -> Vec<PromptCandidate> {
    let mut found: Vec<PromptCandidate> = Vec::new();
    for (engine, home) in homes {
        let Some(home) = home else { continue };
        let Some(file) = forge_engines::read_prompt_file(*engine, home) else {
            continue;
        };
        let Ok(Some(text)) = file.text else { continue };
        let text = forge_engines::normalize_prompt(&text);
        if text.is_empty() {
            continue;
        }
        let id = forge_engines::prompt_digest(&text);
        match found.iter_mut().find(|candidate| candidate.id == id) {
            Some(candidate) => candidate.engines.push(*engine),
            None => found.push(PromptCandidate {
                id,
                engines: vec![*engine],
                text,
            }),
        }
    }
    found
}

fn unavailable(engine: EngineId) -> PromptEngineState {
    PromptEngineState {
        engine,
        status: PromptEngineStatus::Unavailable,
        file: None,
        external_text: None,
        reason: Some(format!(
            "{} reads no global prompt file Artisan can manage.",
            engine.display_name()
        )),
    }
}

fn awaiting(homes: &[(EngineId, Option<PathBuf>)]) -> Vec<PromptEngineState> {
    homes
        .iter()
        .map(|(engine, home)| {
            let probe = home.clone().unwrap_or_default();
            let Some(file) = forge_engines::read_prompt_file(*engine, &probe) else {
                return unavailable(*engine);
            };
            PromptEngineState {
                engine: *engine,
                status: if home.is_some() {
                    PromptEngineStatus::AwaitingChoice
                } else {
                    PromptEngineStatus::EngineAbsent
                },
                file: Some(file.display),
                external_text: None,
                reason: None,
            }
        })
        .collect()
}

fn prompt_state(
    engine: EngineId,
    home: Option<&PathBuf>,
    canonical: &str,
    force: Option<EngineId>,
) -> PromptEngineState {
    let probe = home.cloned().unwrap_or_default();
    let Some(file) = forge_engines::read_prompt_file(engine, &probe) else {
        return unavailable(engine);
    };
    let state = |status, external_text, reason| PromptEngineState {
        engine,
        status,
        file: Some(file.display.clone()),
        external_text,
        reason,
    };
    let Some(home) = home else {
        return state(PromptEngineStatus::EngineAbsent, None, None);
    };
    match forge_engines::apply_prompt(engine, home, canonical, force == Some(engine)) {
        Some(PromptApplied::Synced) | None => state(PromptEngineStatus::Synced, None, None),
        Some(PromptApplied::Drifted(text)) => state(PromptEngineStatus::Drifted, Some(text), None),
        Some(PromptApplied::Failed(reason)) => {
            state(PromptEngineStatus::Failed, None, Some(reason))
        }
    }
}
