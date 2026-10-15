//! Adapter-owned slash normalization. The client sends literal user text.
use anyhow::{Result, bail};
use artisan_api::EngineId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptCommand {
    /// A thread setting, accepted without starting a model turn.
    Configure {
        option: &'static str,
        label: &'static str,
        enabled: Option<bool>,
    },
    /// Native prompt bytes; the stored user message remains unchanged.
    Prompt { text: String },
}

pub fn normalize_command(engine: EngineId, text: &str) -> Result<Option<PromptCommand>> {
    if engine != EngineId::Claude {
        return Ok(None);
    }
    let text = text.trim_start();
    let (name, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let rest = rest.trim();
    let args = match name {
        "/ultrathink" => {
            if rest.is_empty() {
                bail!("Usage: /ultrathink <prompt>");
            }
            return Ok(Some(PromptCommand::Prompt {
                text: format!("ultrathink\n{rest}"),
            }));
        }
        "/ultracode" => rest,
        "/effort" => {
            let (mode, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            if mode != "ultracode" {
                return Ok(None);
            }
            args.trim()
        }
        _ => return Ok(None),
    };
    let enabled = match args {
        "" => None,
        "on" => Some(true),
        "off" => Some(false),
        _ => bail!("Usage: /ultracode [on|off]"),
    };
    Ok(Some(PromptCommand::Configure {
        option: "ultracode",
        label: "Ultracode",
        enabled,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_commands_are_independent_of_effort_and_validate_arguments() {
        assert_eq!(
            normalize_command(EngineId::Claude, "/ultracode on").unwrap(),
            Some(PromptCommand::Configure {
                option: "ultracode",
                label: "Ultracode",
                enabled: Some(true)
            })
        );
        assert_eq!(
            normalize_command(EngineId::Claude, "/effort ultracode off").unwrap(),
            Some(PromptCommand::Configure {
                option: "ultracode",
                label: "Ultracode",
                enabled: Some(false)
            })
        );
        assert_eq!(
            normalize_command(EngineId::Claude, "/ultracode").unwrap(),
            Some(PromptCommand::Configure {
                option: "ultracode",
                label: "Ultracode",
                enabled: None
            })
        );
        for text in [
            "/ultracode maybe",
            "/ultracode on extra",
            "/effort ultracode on off",
            "/ultrathink  ",
        ] {
            assert!(normalize_command(EngineId::Claude, text).is_err(), "{text}");
        }
        assert!(
            normalize_command(EngineId::Codex, "/ultracode on")
                .unwrap()
                .is_none()
        );
        assert!(
            normalize_command(EngineId::Claude, "/effort high")
                .unwrap()
                .is_none()
        );
    }
}
