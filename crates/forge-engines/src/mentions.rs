//! Skill mentions in a user message, handed to each engine in its own form.
//!
//! The Editor writes a skill mention as `$name` in the message text and
//! Forge stores that text unchanged. Codex reads `$name` itself. Claude
//! Code, Grok, `OpenCode` and Cursor do not, so the input they receive gets
//! one closing line naming the skills the user asked for. A mention counts
//! only as the composer paints it: a `$` at the start of a word, followed by
//! the name of an installed skill and then a word end.

use artisan_api::EngineId;

/// Characters that end a mention name besides whitespace; the composer's
/// list.
const NAME_TERMINATORS: &[char] = &[',', '.', ';', ':', '!', '?', ')', ']', '}', '"', '\''];

/// The skills `text` mentions, in order and without repeats.
fn skill_mentions<'a>(text: &str, skills: &'a [artisan_api::Skill]) -> Vec<&'a str> {
    let mut mentioned = Vec::new();
    for (index, character) in text.char_indices() {
        if character != '$'
            || !text[..index]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace)
        {
            continue;
        }
        let rest = &text[index + 1..];
        let longest = skills
            .iter()
            .map(|skill| skill.name.as_str())
            .filter(|name| {
                rest.strip_prefix(name).is_some_and(|after| {
                    after
                        .chars()
                        .next()
                        .is_none_or(|next| next.is_whitespace() || NAME_TERMINATORS.contains(&next))
                })
            })
            .max_by_key(|name| name.len());
        if let Some(name) = longest
            && !mentioned.contains(&name)
        {
            mentioned.push(name);
        }
    }
    mentioned
}

/// The text `engine` should receive for a user message: unchanged for
/// Codex or when it mentions no installed skill, else with a closing line
/// naming the mentioned skills.
#[must_use]
pub fn with_skill_mentions(engine: EngineId, text: &str, skills: &[artisan_api::Skill]) -> String {
    if engine == EngineId::Codex {
        return text.to_owned();
    }
    let mentioned = skill_mentions(text, skills);
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
