//! Skill mentions in a user message, handed to each engine in its own form.
//!
//! The Editor writes a skill mention as `$name` in the message text and the
//! Forge stores that text unchanged. Codex reads `$name` itself. Claude
//! Code, Grok, and `OpenCode` do not, so the prompt they receive gets one
//! closing line naming the skills the user asked for. A mention counts only
//! as the composer paints it: a sigil at the start of a word, followed by a
//! name the engine is known to load and then a word end.

use artisan_domain::EngineId;

/// Characters that end a mention name besides whitespace; the composer's
/// list.
const NAME_TERMINATORS: &[char] = &[',', '.', ';', ':', '!', '?', ')', ']', '}', '"', '\''];

/// The skills `text` mentions, in order and without repeats.
#[must_use]
pub(crate) fn skill_mentions(text: &str, known: &[String]) -> Vec<String> {
    let mut mentioned: Vec<String> = Vec::new();
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
        let longest = known
            .iter()
            .filter(|name| {
                rest.strip_prefix(name.as_str()).is_some_and(|after| {
                    after
                        .chars()
                        .next()
                        .is_none_or(|next| next.is_whitespace() || NAME_TERMINATORS.contains(&next))
                })
            })
            .max_by_key(|name| name.len());
        if let Some(name) = longest
            && !mentioned.contains(name)
        {
            mentioned.push(name.clone());
        }
    }
    mentioned
}

/// The prompt `engine` should receive for `text`: `None` when it needs no
/// change, else the text with a closing line naming the mentioned skills.
#[must_use]
pub(crate) fn directed_text(engine: EngineId, text: &str, known: &[String]) -> Option<String> {
    if matches!(engine, EngineId::Codex) {
        return None;
    }
    let mentioned = skill_mentions(text, known);
    if mentioned.is_empty() {
        return None;
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
    Some(format!(
        "{text}\n\n(The user asked for the {noun} {names}: load and follow it for this request.)"
    ))
}

/// `payload` as `engine` should receive it.
#[must_use]
pub(crate) fn directed_payload(
    engine: EngineId,
    payload: artisan_domain::QueueMessagePayload,
    known: &[String],
) -> artisan_domain::QueueMessagePayload {
    let Some(directed) = payload
        .text()
        .and_then(|text| directed_text(engine, text.as_str(), known))
    else {
        return payload;
    };
    artisan_domain::AuthoredText::parse(directed)
        .ok()
        .and_then(|text| {
            artisan_domain::QueueMessagePayload::new(Some(text), payload.attachments().to_vec())
                .ok()
        })
        .unwrap_or(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        vec![
            "apple-design".to_owned(),
            "cloudflare:wrangler".to_owned(),
            "cloudflare".to_owned(),
        ]
    }

    #[test]
    fn mentions_follow_the_composer_rules() {
        assert_eq!(
            skill_mentions(
                "$apple-design and $cloudflare:wrangler, again $apple-design",
                &known()
            ),
            ["apple-design", "cloudflare:wrangler"]
        );
        assert!(
            skill_mentions("costs $5 or a$apple-design or $apple-designer", &known()).is_empty()
        );
    }

    #[test]
    fn codex_reads_mentions_itself() {
        assert_eq!(
            directed_text(EngineId::Codex, "$apple-design", &known()),
            None
        );
    }

    #[test]
    fn other_engines_get_a_closing_line() {
        assert_eq!(
            directed_text(EngineId::Claude, "Polish this with $apple-design", &known()).as_deref(),
            Some(
                "Polish this with $apple-design\n\n(The user asked for the skill `apple-design`: load and follow it for this request.)"
            )
        );
        assert_eq!(
            directed_text(EngineId::Claude, "no mentions", &known()),
            None
        );
    }
}
