//! Grok Build model discovery (`grok models`).
//!
//! The CLI lists the account-visible models under a header ending in
//! `models:` (`Available models:` since 1.0.4x), marking the default with
//! `*` and a trailing `(default)`. The command is the same bounded
//! invocation the readiness probe already uses (`--no-auto-update models`);
//! here its stdout is parsed into rows. Unknown columns after the id are kept
//! as the display label; the `(default)` annotation is a marker, not a label.

use std::time::Duration;

use super::process::run_bounded;
use super::{DiscoveredModel, DiscoveredThinking};

/// Deadline for the listing command.
const DEADLINE: Duration = Duration::from_secs(4);
/// Output bound for the listing command.
const MAX_BYTES: usize = 1024 * 1024;

/// What a Grok Build model listing proved.
#[derive(Debug)]
pub(super) enum GrokListing {
    /// The account's models.
    Models(Vec<DiscoveredModel>),
    /// The CLI is signed out. It still prints a built-in fallback list,
    /// which is not the account's and is discarded.
    SignedOut,
}

/// Probes Grok Build; `None` when it does not answer.
pub(super) async fn discover_grok(program: Option<&super::EngineProgram>) -> Option<GrokListing> {
    let executable = program?;
    let output = Box::pin(run_bounded(
        executable,
        &["--no-auto-update", "models"],
        DEADLINE,
        MAX_BYTES,
    ))
    .await?;
    if !output.success {
        return None;
    }
    if is_signed_out(&output.stdout) {
        return Some(GrokListing::SignedOut);
    }
    Some(GrokListing::Models(parse_models(&output.stdout)))
}

/// Whether the listing opens with the CLI's signed-out notice
/// (`You are not authenticated.` in Grok Build 1.0.46).
fn is_signed_out(output: &str) -> bool {
    output
        .lines()
        .map(str::trim)
        .take_while(|line| !line.to_ascii_lowercase().ends_with("models:"))
        .any(|line| {
            let line = line.to_ascii_lowercase();
            line.contains("not authenticated") || line.contains("not logged in")
        })
}

/// Parses the model listing. Lines before a header ending in `models:` are
/// ignored when one is present; ids must be single tokens.
fn parse_models(output: &str) -> Vec<DiscoveredModel> {
    let lines = output.lines().collect::<Vec<_>>();
    let header = lines
        .iter()
        .position(|line| line.trim().to_ascii_lowercase().ends_with("models:"));
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
            .map_or(trimmed, str::trim_start);
        let mut parts = without_marker.split_whitespace();
        let Some(id) = parts.next() else {
            continue;
        };
        if !is_model_id(id) || !(indented || id_has_signal(id)) {
            continue;
        }
        let mut default = trimmed.starts_with('*');
        let label = parts
            .filter(|part| {
                let marker = part.eq_ignore_ascii_case("(default)");
                default |= marker;
                !marker
            })
            .collect::<Vec<_>>()
            .join(" ");
        rows.push(DiscoveredModel {
            engine_id: "grok",
            provider: if id.to_ascii_lowercase().starts_with("composer-") {
                "cursor".to_owned()
            } else {
                "xai".to_owned()
            },
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
            default,
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
    fn default_annotation_marks_the_default_instead_of_naming_it() {
        // Grok Build 1.0.46 output, verbatim.
        let rows = parse_models(
            "Default model: grok-4.6\n\nAvailable models:\n  * grok-4.6 (default)\n  - grok-4.5\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].native_model_id, "grok-4.6");
        assert_eq!(rows[0].name, "Grok 4.6");
        assert!(rows[0].default);
        assert_eq!(rows[1].native_model_id, "grok-4.5");
        assert_eq!(rows[1].name, "Grok 4.5");
        assert!(!rows[1].default);
    }

    #[test]
    fn signed_out_listing_is_not_the_accounts() {
        assert!(is_signed_out(
            "You are not authenticated.\n\nDefault model: grok-4.6\n\nAvailable models:\n  * grok-4.6 (default)\n  - grok-4.5\n"
        ));
        assert!(!is_signed_out(
            "Default model: grok-4.7\n\nAvailable models:\n  * grok-4.7 (default)\n"
        ));
    }

    #[test]
    fn ignores_prose_and_missing_header() {
        assert!(parse_models("not authenticated\n").is_empty());
        let rows = parse_models("grok-4.6\n");
        assert_eq!(rows.len(), 1);
    }
}
