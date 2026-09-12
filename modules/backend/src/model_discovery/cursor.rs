//! Cursor CLI model discovery (`agent models`).
//!
//! The Cursor CLI prints one model per line, optionally marking the default
//! with a leading `*`, followed by a human label. Ids are treated as complete
//! native configurations (effort/fast variants are already distinct ids), the
//! same policy the account-catalog adapter applies.

use std::time::Duration;

use super::process::run_bounded;
use super::{DiscoveredModel, DiscoveredThinking};

/// Deadline for the listing command.
const DEADLINE: Duration = Duration::from_secs(4);
/// Output bound for the listing command.
const MAX_BYTES: usize = 1024 * 1024;

/// Probes the Cursor CLI; `None` when it does not answer.
pub(super) async fn discover_cursor(program: Option<&str>) -> Option<Vec<DiscoveredModel>> {
    let executable = program?;
    let output = Box::pin(run_bounded(executable, &["models"], DEADLINE, MAX_BYTES)).await?;
    if !output.success {
        return None;
    }
    let rows = parse_models(&output.stdout);
    Some(rows)
}

/// Parses `agent models` output. Unrecognized lines are ignored rather than
/// guessed at, so a format change degrades to fewer rows.
fn parse_models(output: &str) -> Vec<DiscoveredModel> {
    let mut rows = Vec::new();
    for line in output.lines() {
        let indented = line.starts_with(char::is_whitespace) || line.starts_with('*');
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (is_default, rest) = match trimmed.strip_prefix('*') {
            Some(rest) => (true, rest.trim_start()),
            None => (false, trimmed),
        };
        let mut parts = rest.split_whitespace();
        let Some(id) = parts.next() else {
            continue;
        };
        if !is_model_id(id) || !(indented || id_has_signal(id)) {
            continue;
        }
        let label = parts.collect::<Vec<_>>().join(" ");
        rows.push(DiscoveredModel {
            engine_id: "cursor",
            provider: infer_provider(id).to_owned(),
            native_model_id: id.to_owned(),
            upstream_model_id: None,
            variant_id: None,
            name: if label.is_empty() {
                title_case(id)
            } else {
                label
            },
            description: None,
            hidden: false,
            default: is_default,
            thinking: DiscoveredThinking::Native {
                description: format!(
                    "Cursor exposes {id} as a complete native configuration. Any encoded reasoning level is part of its model ID, not a separately documented CLI control."
                ),
            },
            fast: id.ends_with("-fast"),
            context_window_tokens: None,
            max_context_window_tokens: None,
            output_tokens: None,
            image_input: false,
            tools: true,
            web_search: false,
            cost: None,
            status: "active",
            metadata_confidence: "reported",
        });
    }
    rows
}

fn is_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._:-/[]".contains(character))
}

/// Model identifiers normally carry a digit or a separator; free prose does
/// not. Used to ignore unindented header/prose lines without a model signal.
fn id_has_signal(id: &str) -> bool {
    id.chars()
        .any(|character| character.is_ascii_digit() || "-._".contains(character))
}

/// Provider inference mirroring `cursor-account-catalog.ts`.
fn infer_provider(id: &str) -> &'static str {
    let lower = id.to_ascii_lowercase();
    if lower.starts_with("gpt")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("chatgpt")
    {
        "openai"
    } else if lower.starts_with("claude") {
        "anthropic"
    } else if lower.starts_with("gemini") {
        "google"
    } else if lower.starts_with("grok") {
        "xai"
    } else if lower.starts_with("kimi") {
        "moonshot"
    } else if lower.starts_with("deepseek") {
        "deepseek"
    } else if lower.starts_with("glm") {
        "zai"
    } else if lower.starts_with("minimax") {
        "minimax"
    } else if lower.starts_with("llama") {
        "meta"
    } else {
        "cursor"
    }
}

fn title_case(id: &str) -> String {
    id.split('-')
        .map(|part| {
            let upper = part.to_ascii_uppercase();
            if matches!(
                upper.as_str(),
                "AI" | "GPT" | "GLM" | "KIMI" | "LLAMA" | "R1"
            ) {
                upper
            } else {
                let mut characters = part.chars();
                match characters.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_labelled_lines_and_default_marker() {
        let rows = parse_models(
            "* composer-2.5-fast Fast Composer\n  gpt-5.6-sol GPT 5.6 Sol\n\nnot a model id line\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].native_model_id, "composer-2.5-fast");
        assert!(rows[0].default);
        assert_eq!(rows[0].name, "Fast Composer");
        assert_eq!(rows[0].provider, "cursor");
        assert!(rows[0].fast);
        assert_eq!(rows[1].native_model_id, "gpt-5.6-sol");
        assert_eq!(rows[1].provider, "openai");
        assert!(!rows[1].default);
        assert_eq!(rows[1].name, "GPT 5.6 Sol");
    }

    #[test]
    fn id_without_label_gets_title_case_name() {
        let rows = parse_models("claude-opus-5\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Claude Opus 5");
        assert_eq!(rows[0].provider, "anthropic");
    }
}
