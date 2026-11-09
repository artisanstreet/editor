use artisan_api::{EngineId, Model};
use editor_aqua::composer::model_line::visible_models;

fn catalog(ids: &[&str]) -> Vec<Model> {
    ids.iter()
        .map(|id| Model {
            id: (*id).to_owned(),
            ..Model::default()
        })
        .collect()
}

fn shown(engine: EngineId, models: &[Model], show_all: bool) -> Vec<String> {
    visible_models(engine, models, show_all)
        .into_iter()
        .map(|index| models[index].id.clone())
        .collect()
}

#[test]
fn claude_shows_the_latest_of_each_class_with_the_top_class_first() {
    let models = catalog(&[
        "claude-opus-5-5",
        "claude-fable-5-1",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
        "claude-haiku-4-5-20251001",
        "claude-sonnet-5",
        "claude-opus-5",
        "claude-fable-5",
        "claude-opus-4-8",
    ]);
    assert_eq!(
        shown(EngineId::Claude, &models, false),
        ["claude-fable-5-1", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"],
    );
}

#[test]
fn a_dated_claude_id_does_not_count_its_date_as_a_version() {
    let models = catalog(&["claude-haiku-4-5-20251001", "claude-haiku-4-6"]);
    assert_eq!(shown(EngineId::Claude, &models, false), ["claude-haiku-4-6"]);
}

#[test]
fn fable_and_mythos_compete_as_one_class() {
    let models = catalog(&["claude-fable-5-1", "claude-mythos-6"]);
    assert_eq!(shown(EngineId::Claude, &models, false), ["claude-mythos-6"]);
}

#[test]
fn codex_shows_the_latest_of_each_class_with_astra_first() {
    let models = catalog(&[
        "gpt-6.1-sol",
        "gpt-6-astra",
        "gpt-6.1-terra",
        "gpt-6-sol",
        "gpt-reserve",
        "gpt-6-luna",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "codex-auto-review",
    ]);
    assert_eq!(
        shown(EngineId::Codex, &models, false),
        ["gpt-6-astra", "gpt-6.1-sol", "gpt-6.1-terra", "gpt-6-luna", "gpt-reserve", "codex-auto-review"],
    );
}

#[test]
fn a_minor_version_beats_the_same_major_without_one() {
    let models = catalog(&["gpt-6-sol", "gpt-6.1-sol", "gpt-5.6-sol"]);
    assert_eq!(shown(EngineId::Codex, &models, false), ["gpt-6.1-sol"]);
}

#[test]
fn the_setting_shows_the_whole_catalog_in_its_own_order() {
    let models = catalog(&["claude-opus-5", "claude-opus-5-5", "claude-haiku-5-5"]);
    assert_eq!(
        shown(EngineId::Claude, &models, true),
        ["claude-opus-5", "claude-opus-5-5", "claude-haiku-5-5"],
    );
}

#[test]
fn a_catalog_with_no_recognised_line_stays_fully_visible() {
    let models = catalog(&["some-new-scheme", "another-model"]);
    assert_eq!(
        shown(EngineId::Codex, &models, false),
        ["some-new-scheme", "another-model"],
    );
}

#[test]
fn opencode_has_no_standard_line_and_keeps_everything() {
    let models = catalog(&["anthropic/claude-opus-5-5", "openai/gpt-6-sol"]);
    assert_eq!(
        shown(EngineId::OpenCode, &models, false),
        ["anthropic/claude-opus-5-5", "openai/gpt-6-sol"],
    );
}

#[test]
fn newly_discovered_families_are_not_hidden_by_the_known_family_filter() {
    let models = catalog(&["gpt-6-sol", "gpt-6.1-sol", "new-native-model"]);
    assert_eq!(shown(EngineId::Codex, &models, false), ["gpt-6.1-sol", "new-native-model"]);
}
