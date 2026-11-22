use super::*;

fn listed(models: &[(&str, Option<&str>)]) -> Vec<Model> {
    models
        .iter()
        .map(|(id, section)| Model {
            id: (*id).to_owned(),
            name: (*id).to_owned(),
            section: section.map(str::to_owned),
            ..Model::default()
        })
        .collect()
}

fn shape(rows: &[ModelRow]) -> Vec<&str> {
    rows.iter()
        .map(|row| match row {
            ModelRow::Model { id, .. } => id.as_str(),
            ModelRow::Gap(_) => "gap",
            ModelRow::Engine(_) | ModelRow::Gate(_) | ModelRow::Group { .. } => "other",
        })
        .collect()
}

#[test]
fn gemini_models_sit_apart_from_the_other_labs_behind_one_gap() {
    let models = listed(&[
        ("gemini-4-neon", Some("Gemini")),
        ("gemini-3.8-flash", Some("Gemini")),
        ("claude-opus-5-5", Some("Other")),
        ("gpt-oss-120b-medium", Some("Other")),
    ]);
    let indexed: Vec<_> = models.iter().enumerate().collect();
    assert_eq!(
        shape(&sectioned(EngineId::Antigravity, &indexed)),
        [
            "gemini-4-neon",
            "gemini-3.8-flash",
            "gap",
            "claude-opus-5-5",
            "gpt-oss-120b-medium"
        ]
    );
}

#[test]
fn models_without_sections_list_without_gaps() {
    let models = listed(&[("opus", None), ("sonnet", None), ("haiku", None)]);
    let indexed: Vec<_> = models.iter().enumerate().collect();
    assert_eq!(
        shape(&sectioned(EngineId::Claude, &indexed)),
        ["opus", "sonnet", "haiku"]
    );
}
