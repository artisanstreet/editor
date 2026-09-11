//! Grok Build model discovery (`grok models`).
//!
//! The CLI lists the account-visible models under a `models:` header. The
//! command is the same bounded invocation the readiness probe already uses
//! (`--no-auto-update models`); here its stdout is parsed into rows. Unknown
//! columns after the id are kept as the display label.

use std::time::Duration;

use super::process::run_bounded;
use super::{DiscoveredModel, DiscoveredThinking, engine_executable};

/// Deadline for the listing command.
const DEADLINE: Duration = Duration::from_secs(4);
/// Output bound for the listing command.
const MAX_BYTES: usize = 1024 * 1024;

/// Probes Grok Build; `None` when it is absent or does not answer.
pub(super) async fn discover_grok() -> Option<Vec<DiscoveredModel>> {
    let executable = engine_executable("ARTISAN_GROK_EXECUTABLE", "grok");
    let output = run_bounded(
        &executable,
        &["--no-auto-update", "models"],
        DEADLINE,
        MAX_BYTES,
    )
    .await?;
    if !output.success {
        return None;
    }
    Some(parse_models(&output.stdout))
}

/// Parses the `models:` listing. Lines before the header are ignored when the
/// header is present; ids must be single tokens.
fn parse_models(output: &str) -> Vec<DiscoveredModel> {
    let lines = output.lines().collect::<Vec<_>>();
    let header = lines
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case("models:"));
    let candidates = match header {
        Some(index) => &lines[index + 1..],
        None => &lines[..],
    };
    let mut rows = Vec::new();
    for line in candidates {
        let indented =
            line.starts_with(char::is_whitespace) || line.starts_with('*') || line.starts_with('-');
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let without_marker = trimmed
            .strip_prefix('*')
            .or_else(|| trimmed.strip_prefix('-'))
            .map(str::trim_start)
            .unwrap_or(trimmed);
        let mut parts = without_marker.split_whitespace();
        let Some(id) = parts.next() else {
            continue;
        };
        if !is_model_id(id) || !(indented || id_has_signal(id)) {
            continue;
        }
        let label = parts.collect::<Vec<_>>().join(" ");
        rows.push(DiscoveredModel {
            engine_id: "grok",
            provider: if id.to_ascii_lowercase().starts_with("composer-") {
                "cursor".to_owned()
            } else {
                "xai".to_owned()
            },
            native_model_id: id.to_owned(),
            upstream_model_id: None,
            name: if label.is_empty() {
                title_case(id)
            } else {
                label
            },
            description: None,
            hidden: false,
            default: trimmed.starts_with('*'),
            thinking: DiscoveredThinking::Native {
                description:
                    "Grok Build manages reasoning effort for this model through the harness; the catalogue does not report a separate effort control."
                        .to_owned(),
            },
            fast: false,
            context_window_tokens: None,
            max_context_window_tokens: None,
            output_tokens: None,
            image_input: false,
            tools: true,
            web_search: true,
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
            .all(|character| character.is_ascii_alphanumeric() || "._:-/".contains(character))
}

/// Model identifiers normally carry a digit or a separator; free prose does
/// not. Used to ignore unindented header/prose lines without a model signal.
fn id_has_signal(id: &str) -> bool {
    id.chars()
        .any(|character| character.is_ascii_digit() || "-._".contains(character))
}

fn title_case(id: &str) -> String {
    id.split('-')
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_listing() {
        let rows = parse_models("models:\n  grok-4.6\n  grok-4.5\n");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].native_model_id, "grok-4.6");
        assert_eq!(rows[0].name, "Grok 4.6");
        assert_eq!(rows[0].provider, "xai");
        assert!(rows[0].web_search);
    }

    #[test]
    fn parses_default_marker_and_composer_provider() {
        let rows = parse_models("models:\n* composer-2.5 Composer Fast\n");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].default);
        assert_eq!(rows[0].provider, "cursor");
        assert_eq!(rows[0].name, "Composer Fast");
    }

    #[test]
    fn ignores_prose_and_missing_header() {
        assert!(parse_models("not authenticated\n").is_empty());
        let rows = parse_models("grok-4.6\n");
        assert_eq!(rows.len(), 1);
    }
}
