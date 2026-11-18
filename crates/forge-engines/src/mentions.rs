use artisan_api::{EngineId, ThreadId};

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

#[must_use]
pub fn with_thread_mentions(text: &str, threads: &[(ThreadId, String)]) -> String {
    let titles = threads
        .iter()
        .map(|(_, title)| title.as_str())
        .collect::<Vec<_>>();
    let mut seen = std::collections::HashSet::new();
    let mentioned = artisan_mentions::ThreadTitles::new(&titles)
        .mentions(text)
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
#[path = "../tests/mentions/unit.rs"]
mod tests;
