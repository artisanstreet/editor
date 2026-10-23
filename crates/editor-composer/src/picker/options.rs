//! What each policy control offers.

use super::*;

/// What a permission level does on `engine`, as Forge launches it.
const fn permissions_description(engine: EngineId, permissions: Permissions) -> &'static str {
    match (engine, permissions) {
        (EngineId::Codex, Permissions::Auto) => {
            "Sandboxed to the project; Codex's reviewer decides what else may run."
        }
        (EngineId::Claude, Permissions::Auto) => {
            "Claude's auto mode: safe actions run, risky ones are checked first."
        }
        (EngineId::OpenCode, Permissions::Auto) => "Edits run; it asks before terminal commands.",
        (_, Permissions::Full) => "Everything runs, nothing asks. No sandbox.",
    }
}

/// The model's variants; a config without one shows the model's default
/// as chosen.
fn variant_options(config: &EngineConfig, model: Option<&Model>) -> Vec<AxisOption> {
    let Some(model) = model else {
        return Vec::new();
    };
    let chosen = model.variant_for(config.variant.as_deref());
    model
        .variants
        .iter()
        .map(|variant| AxisOption {
            special: false,
            label: variant.label.clone(),
            description: None,
            advisory: None,
            selected: chosen.as_ref() == Some(&variant.id),
            apply: EngineConfig {
                variant: Some(variant.id.clone()),
                ..config.clone()
            },
        })
        .collect()
}

/// The model's speed tiers; with none chosen, its default is.
fn speed_options(config: &EngineConfig, model: Option<&Model>) -> Vec<AxisOption> {
    model
        .map(|model| model.speeds.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|speed| AxisOption {
            special: false,
            label: speed.label.clone(),
            description: speed.description.clone(),
            advisory: None,
            selected: config
                .speed
                .as_ref()
                .map_or(speed.default, |chosen| chosen == &speed.id),
            apply: EngineConfig {
                speed: Some(speed.id.clone()),
                ..config.clone()
            },
        })
        .collect()
}

/// The options one policy control offers, each with the settings it
/// applies.
pub(super) fn axis_options(
    axis: Axis,
    config: &EngineConfig,
    model: Option<&Model>,
) -> Vec<AxisOption> {
    let engine = config.engine;
    match axis {
        Axis::Variant => variant_options(config, model),
        Axis::Thinking => {
            let Some(model) = model else {
                return Vec::new();
            };
            let options = model.efforts.iter().map(|effort| {
                let note = model
                    .effort_notes
                    .iter()
                    .find(|note| &note.effort == effort);
                AxisOption {
                    special: note.is_some_and(|note| note.special),
                    label: humanize(effort),
                    description: note.and_then(|note| note.description.clone()),
                    advisory: note.and_then(|note| note.advisory.clone()),
                    selected: config.effort.as_ref() == Some(effort),
                    apply: EngineConfig {
                        effort: Some(effort.clone()),
                        ..config.clone()
                    },
                }
            });
            let (mut regular, special): (Vec<_>, Vec<_>) =
                options.partition(|option| !option.special);
            regular.extend(special);
            regular
        }
        Axis::Context => model.map_or_else(Vec::new, |model| {
            model
                .context_windows
                .iter()
                .map(|window| {
                    let mut apply = config.clone();
                    if let Some(tokens) = window.override_tokens {
                        apply.options.insert("context_window".into(), tokens.into());
                    } else {
                        apply.options.remove("context_window");
                    }
                    AxisOption {
                        special: false,
                        label: context_label(window.tokens),
                        description: None,
                        advisory: None,
                        selected: config.context_window_override() == window.override_tokens,
                        apply,
                    }
                })
                .collect()
        }),
        Axis::Speed => speed_options(config, model),
        Axis::Permission => [
            (Permissions::Auto, "Auto"),
            (Permissions::Full, "Full access"),
        ]
        .into_iter()
        .map(|(permissions, label)| AxisOption {
            special: false,
            label: label.to_owned(),
            description: Some(permissions_description(engine, permissions).to_owned()),
            advisory: None,
            selected: config.permissions == permissions,
            apply: EngineConfig {
                permissions,
                ..config.clone()
            },
        })
        .collect(),
    }
}

#[cfg(test)]
mod effort_presentation_tests {
    use super::*;

    #[test]
    fn special_metadata_partitions_efforts_without_changing_canonical_values() {
        let model: Model = serde_json::from_value(serde_json::json!({
            "id": "test-model", "name": "Test model",
            "efforts": ["custom_mode", "xhigh", "ultra", "low", "another_mode"],
            "effort_notes": [
                {"effort": "custom_mode", "special": true, "description": "First mode"},
                {"effort": "ultra", "special": false},
                {"effort": "another_mode", "special": true, "advisory": "Additional usage"}
            ]
        }))
        .unwrap();
        for engine in [EngineId::Claude, EngineId::Codex, EngineId::OpenCode] {
            let config = EngineConfig {
                effort: Some("custom_mode".into()),
                ..EngineConfig::new(engine)
            };
            let options = axis_options(Axis::Thinking, &config, Some(&model));
            let canonical: Vec<_> = options
                .iter()
                .map(|option| option.apply.effort.as_deref().unwrap())
                .collect();
            assert_eq!(
                canonical,
                ["xhigh", "ultra", "low", "custom_mode", "another_mode"]
            );
            assert_eq!(
                options
                    .iter()
                    .map(|option| option.special)
                    .collect::<Vec<_>>(),
                [false, false, false, true, true]
            );
            assert_eq!(options[0].label, "Extra High");
            assert_eq!(
                options[1].label, "Ultra",
                "names alone do not make an effort special"
            );
            assert!(options[3].selected);
            assert_eq!(options[3].description.as_deref(), Some("First mode"));
            assert_eq!(options[4].advisory.as_deref(), Some("Additional usage"));
            assert!(options.iter().all(|option| option.apply.engine == engine));
        }
    }

    #[test]
    fn advertised_ultra_mode_keeps_its_submitted_effort_id() {
        let model: Model = serde_json::from_value(serde_json::json!({
            "id": "test-model", "name": "Test model",
            "efforts": ["low", "medium", "high", "xhigh", "ultra"],
            "effort_notes": [{"effort": "ultra", "special": true}]
        }))
        .unwrap();
        let config = EngineConfig {
            effort: Some("ultra".into()),
            ..EngineConfig::new(EngineId::Codex)
        };
        let options = axis_options(Axis::Thinking, &config, Some(&model));
        let ultra = options.last().unwrap();
        assert!(ultra.special && ultra.selected);
        assert_eq!(ultra.label, "Ultra");
        assert_eq!(ultra.apply.effort.as_deref(), Some("ultra"));
        assert_eq!(options[3].apply.effort.as_deref(), Some("xhigh"));
        assert_eq!(options[3].label, "Extra High");
    }
}
