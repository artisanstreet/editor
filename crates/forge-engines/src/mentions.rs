//! Skill mentions in a user message, handed to each engine in its own form.
//!
//! The Editor writes a skill mention as `$name` in the message text and
//! Forge stores that text unchanged. Codex reads `$name` itself. Claude
//! Code, Grok, `OpenCode` and Cursor do not, so the input they receive gets
//! one closing line naming the skills the user asked for. A mention counts
//! only as the composer paints it: a `$` at the start of a word, followed by
//! the name of an installed skill and then a word end.

use artisan_api::EngineId;

/// The text `engine` should receive for a user message: unchanged for
/// Codex or when it mentions no installed skill, else with a closing line
/// naming the mentioned skills.
#[must_use]
pub fn with_skill_mentions(engine: EngineId, text: &str, skills: &[artisan_api::Skill]) -> String {
    if engine == EngineId::Codex {
        return text.to_owned();
    }
    let names = skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<Vec<_>>();
    let mut seen = std::collections::HashSet::new();
    let mentioned = artisan_api::skill_mentions(text, &names)
        .into_iter()
        .filter(|mention| seen.insert(mention.name.clone()))
        .map(|mention| mention.name)
        .collect::<Vec<_>>();
    if mentioned.is_empty() {
        return text.to_owned();
    }
    let names = mentioned
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let noun = if mentioned.len() == 1 {
        "skill"
    } else {
        "skills"
    };
    format!(
        "{text}\n\n(The user asked for the {noun} {names}: load and follow it for this request.)"
    )
}
