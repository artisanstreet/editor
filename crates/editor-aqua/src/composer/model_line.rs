use std::collections::BTreeMap;

use artisan_api::{EngineId, Model};

type Version = (u32, u32);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ClaudeClass {
    FableMythos,
    Opus,
    Sonnet,
    Haiku,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum CodexClass {
    Astra,
    Sol,
    Terra,
    Luna,
}

fn claude_model(id: &str) -> Option<(ClaudeClass, Version)> {
    let mut tokens = id.split('[').next()?.strip_prefix("claude-")?.split('-');
    let class = match tokens.next()? {
        "fable" | "mythos" => ClaudeClass::FableMythos,
        "opus" => ClaudeClass::Opus,
        "sonnet" => ClaudeClass::Sonnet,
        "haiku" => ClaudeClass::Haiku,
        _ => return None,
    };
    let mut numbers = tokens.take_while(|token| token.len() <= 2);
    let major = numbers.next()?.parse().ok()?;
    let minor = numbers.next().map_or(Some(0), |minor| minor.parse().ok())?;
    Some((class, (major, minor)))
}

fn codex_model(id: &str) -> Option<(CodexClass, Version)> {
    let (version, class) = id.strip_prefix("gpt-")?.rsplit_once('-')?;
    let class = match class {
        "astra" => CodexClass::Astra,
        "sol" => CodexClass::Sol,
        "terra" => CodexClass::Terra,
        "luna" => CodexClass::Luna,
        _ => return None,
    };
    let (major, minor) = version.split_once('.').unwrap_or((version, "0"));
    Some((class, (major.parse().ok()?, minor.parse().ok()?)))
}

fn latest_of_each_class<Class: Copy + Ord>(
    models: &[Model],
    identify: impl Fn(&str) -> Option<(Class, Version)>,
) -> Vec<usize> {
    let mut latest: BTreeMap<Class, (Version, usize)> = BTreeMap::new();
    for (index, model) in models.iter().enumerate() {
        let Some((class, version)) = identify(&model.id) else {
            continue;
        };
        latest
            .entry(class)
            .and_modify(|best| {
                if version > best.0 {
                    *best = (version, index);
                }
            })
            .or_insert((version, index));
    }
    latest.into_values().map(|(_, index)| index).collect()
}

#[must_use]
pub fn claude_lineup(models: &[Model]) -> Vec<usize> {
    latest_of_each_class(models, claude_model)
}

#[must_use]
pub fn codex_lineup(models: &[Model]) -> Vec<usize> {
    latest_of_each_class(models, codex_model)
}

#[must_use]
pub fn visible_models(engine: EngineId, models: &[Model], show_all: bool) -> Vec<usize> {
    let lineup = match (show_all, engine) {
        (true, _) | (false, EngineId::OpenCode) => Vec::new(),
        (false, EngineId::Claude) => claude_lineup(models),
        (false, EngineId::Codex) => codex_lineup(models),
    };
    if lineup.is_empty() {
        (0..models.len()).collect()
    } else {
        let mut visible = lineup;
        visible.extend(models.iter().enumerate().filter_map(|(index, model)| {
            let recognized = match engine {
                EngineId::Claude => claude_model(&model.id).is_some(),
                EngineId::Codex => codex_model(&model.id).is_some(),
                EngineId::OpenCode => false,
            };
            (!recognized).then_some(index)
        }));
        visible
    }
}
