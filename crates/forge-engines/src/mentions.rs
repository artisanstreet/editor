//! Skill and thread mentions in a user message, handed to each engine in
//! its own form.
//!
//! The Editor writes a skill mention as `$name` in the message text and
//! Forge stores that text unchanged. Codex reads `$name` itself. Claude
//! Code and `OpenCode` do not, so the input they receive gets
//! one closing line naming the skills the user asked for. A mention counts
//! only as the composer paints it: a `$` at the start of a word, followed by
//! the name of an installed skill and then a word end.

use artisan_api::{EngineId, ThreadId};

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
    let mentioned = artisan_mentions::skill_mentions(text, &names)
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

/// The text with a closing line naming the threads it mentions as `@title`.
///
/// The engine can read them through the Artisan MCP. Each title names the
/// most recently active thread that has it: `threads` comes newest first.
/// No engine reads these by itself.
#[must_use]
pub fn with_thread_mentions(text: &str, threads: &[(ThreadId, String)]) -> String {
    let titles = threads
        .iter()
        .map(|(_, title)| title.as_str())
        .collect::<Vec<_>>();
    let mut seen = std::collections::HashSet::new();
    let mentioned = artisan_mentions::thread_mentions(text, &titles)
        .into_iter()
        .filter(|mention| seen.insert(mention.name.clone()))
        .filter_map(|mention| threads.iter().find(|(_, title)| *title == mention.name))
        .map(|(id, title)| format!("\"{title}\" (`{id}`)"))
        .collect::<Vec<_>>();
    if mentioned.is_empty() {
        return text.to_owned();
    }
    let noun = if mentioned.len() == 1 {
        "the Artisan thread"
    } else {
        "the Artisan threads"
    };
    format!(
        "{text}\n\n(The user mentioned {noun} {}: read what this request needs with the artisan thread_read tool.)",
        mentioned.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_mentioned_thread_reaches_the_engine_by_id_and_the_newest_title_wins() {
        let threads = [
            (ThreadId::from("new"), "Fix sidebar lag".to_owned()),
            (ThreadId::from("old"), "Fix sidebar lag".to_owned()),
        ];
        let text = with_thread_mentions("Like @Fix sidebar lag, twice: @Fix sidebar lag", &threads);
        assert!(
            text.ends_with(
                "(The user mentioned the Artisan thread \"Fix sidebar lag\" (`new`): read what this request needs with the artisan thread_read tool.)"
            ),
            "{text}"
        );
        assert_eq!(with_thread_mentions("mail me@Fix", &threads), "mail me@Fix");
    }
}
