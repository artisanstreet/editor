//! Adapter-owned slash normalization. The client sends literal user text;
//! the stored message stays as typed and only the engine's prompt changes.
use anyhow::{Result, bail};
use artisan_api::{EngineId, SlashCommand};

/// More than any engine lists; the rest are dropped.
const MAX_COMMANDS: usize = 1024;

/// The commands Forge reads for `engine` itself, listed beside the ones the
/// engine reports: exactly what [`normalize_command`] rewrites.
pub fn own(engine: EngineId) -> Vec<SlashCommand> {
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

/// An engine's own list, as the menu can show it: named, each once, bounded.
pub fn reported(commands: Vec<SlashCommand>) -> Vec<SlashCommand> {
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

/// The prompt `engine` reads for `text`, when a command at its start means
/// something else to it; `None` sends the text as typed.
///
/// `/ultrathink` and `/ultracode` hold for the one turn they start: Claude
/// reads each as a keyword in that prompt, so a message without one runs
/// without it.
pub fn normalize_command(engine: EngineId, text: &str) -> Result<Option<String>> {
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
        bail!("Usage: /{keyword} <prompt>");
    }
    Ok(Some(format!("{keyword}\n{rest}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ultra_commands_are_keywords_of_the_turn_they_start() {
        assert_eq!(
            normalize_command(EngineId::Claude, "/ultracode ship the parser").unwrap(),
            Some("ultracode\nship the parser".to_owned())
        );
        assert_eq!(
            normalize_command(EngineId::Claude, "  /ultrathink why?").unwrap(),
            Some("ultrathink\nwhy?".to_owned())
        );
        for text in ["/ultracode", "/ultrathink  "] {
            assert!(normalize_command(EngineId::Claude, text).is_err(), "{text}");
        }
        for (engine, text) in [
            (EngineId::Codex, "/ultracode ship it"),
            (EngineId::Claude, "/goal ship it"),
            (EngineId::Claude, "plain text"),
        ] {
            assert!(normalize_command(engine, text).unwrap().is_none(), "{text}");
        }
    }
}
