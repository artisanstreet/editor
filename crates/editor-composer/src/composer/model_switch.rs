use artisan_api::{EngineConfig, EngineStatus, Run, RunState};

const UNCACHED_REREAD_TOKENS: u64 = 100_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchPrompt {
    pub config: EngineConfig,
    pub from: String,
    pub to: String,
    pub context_tokens: u64,
    pub compaction: Compaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compaction {
    Available,
    BusyRun,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchChoice {
    CompactFirst,
    SwitchAnyway,
    Cancel,
}

#[must_use]
pub fn prompt(run: &Run, engine: Option<&EngineStatus>, to: EngineConfig) -> Option<SwitchPrompt> {
    let from_model = run.model.as_deref()?;
    let to_model = to.model.as_deref()?;
    let context_tokens = run.usage.context_used?;
    if to.engine != run.engine || to_model == from_model || context_tokens < UNCACHED_REREAD_TOKENS
    {
        return None;
    }
    let name = |id: &str| {
        engine
            .and_then(|status| status.models.iter().find(|model| model.id == id))
            .map_or_else(|| id.to_owned(), |model| model.name.clone())
    };
    let (from, to_name) = (name(from_model), name(to_model));
    Some(SwitchPrompt {
        config: to,
        from,
        to: to_name,
        context_tokens,
        compaction: match run.state {
            RunState::Running => Compaction::BusyRun,
            RunState::Finished { .. } => Compaction::Available,
        },
    })
}

#[cfg(test)]
#[path = "../../tests/composer/model_switch.rs"]
mod tests;
