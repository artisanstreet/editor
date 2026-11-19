use artisan_api::{EngineId, SlashCommand};

const MAX_COMMANDS: usize = 1024;

pub(crate) fn own(engine: EngineId) -> Vec<SlashCommand> {
    if engine != EngineId::Claude {
        return Vec::new();
    }
    [
        ("ultracode", "Ultracode for this message"),
        ("ultrathink", "Think hardest about this message"),
    ]
    .into_iter()
    .map(|(name, description)| SlashCommand {
        name: name.to_owned(),
        description: description.to_owned(),
        hint: Some("<prompt>".to_owned()),
    })
    .collect()
}

pub(crate) fn reported(commands: Vec<SlashCommand>) -> Vec<SlashCommand> {
    let mut seen = std::collections::HashSet::new();
    commands
        .into_iter()
        .map(|mut command| {
            command.name = command.name.trim().trim_start_matches('/').to_owned();
            command.description = command.description.trim().to_owned();
            command
        })
        .filter(|command| {
            !command.name.is_empty()
                && !command.name.contains(char::is_whitespace)
                && seen.insert(command.name.clone())
        })
        .take(MAX_COMMANDS)
        .collect()
}

#[derive(Debug, thiserror::Error)]
#[error("Usage: /{keyword} <prompt>")]
pub struct MissingPrompt {
    pub keyword: &'static str,
}

pub fn normalize_command(engine: EngineId, text: &str) -> Result<Option<String>, MissingPrompt> {
    if engine != EngineId::Claude {
        return Ok(None);
    }
    let text = text.trim_start();
    let (name, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let keyword = match name {
        "/ultrathink" => "ultrathink",
        "/ultracode" => "ultracode",
        _ => return Ok(None),
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return Err(MissingPrompt { keyword });
    }
    Ok(Some(format!("{keyword}\n{rest}")))
}

#[cfg(test)]
#[path = "../tests/commands/unit.rs"]
mod tests;
