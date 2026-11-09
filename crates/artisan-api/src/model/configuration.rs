use super::{
    ChoiceSemantic, ConfigBinding, EngineConfig, EngineId, ModelAxis, ModelChoice, ModelControl,
    ModelSpecification, ResolutionChange, ResolvedModelConfig, ThinkingLevel,
};

impl ModelSpecification {
    pub(super) fn push(&mut self, axis: ModelAxis, choice: ModelChoice) {
        match self
            .controls
            .iter_mut()
            .find(|control| control.axis == axis)
        {
            Some(control) => control.choices.push(choice),
            None => self.controls.push(ModelControl {
                axis,
                choices: vec![choice],
            }),
        }
    }

    pub fn visible_choices(
        &self,
        axis: ModelAxis,
        config: &EngineConfig,
        hide_bad: bool,
    ) -> Vec<ModelChoice> {
        let mut choices: Vec<_> = self
            .controls
            .iter()
            .find(|control| control.axis == axis)
            .into_iter()
            .flat_map(|control| &control.choices)
            .filter_map(|choice| {
                let opus = self.identity.harness == EngineId::Claude
                    && self.identity.native_id.split('[').next() == Some("claude-opus-5-5");
                let bad = matches!(self.identity.harness, EngineId::Claude | EngineId::Codex)
                    && (matches!(
                        choice.semantic,
                        ChoiceSemantic::Intensity(ThinkingLevel::Max)
                    ) || choice.id == "ultra"
                        || (opus
                            && matches!(
                                choice.semantic,
                                ChoiceSemantic::Intensity(
                                    ThinkingLevel::Light | ThinkingLevel::ExtraHigh
                                )
                            )));
                let hidden = choice.visibility_override.unwrap_or(hide_bad && bad);
                if hidden && !choice.binding.selected(config) {
                    None
                } else {
                    let mut choice = choice.clone();
                    choice.hidden = hidden;
                    Some(choice)
                }
            })
            .collect();
        let mut preferred = std::collections::BTreeMap::new();
        for choice in &choices {
            if let ChoiceSemantic::Intensity(level) = choice.semantic {
                if choice.binding.selected(config) {
                    preferred.insert(level, choice.id.clone());
                } else {
                    preferred.entry(level).or_insert_with(|| choice.id.clone());
                }
            }
        }
        choices.retain(|choice| match choice.semantic {
            ChoiceSemantic::Intensity(level) => preferred.get(&level) == Some(&choice.id),
            ChoiceSemantic::Default
            | ChoiceSemantic::Activation(_)
            | ChoiceSemantic::Special
            | ChoiceSemantic::Speed(_)
            | ChoiceSemantic::Native => true,
        });
        if choices
            .iter()
            .all(|choice| choice.semantic == ChoiceSemantic::Default)
            && matches!(
                axis,
                ModelAxis::Thinking
                    | ModelAxis::Activation
                    | ModelAxis::Special
                    | ModelAxis::Variant
                    | ModelAxis::Speed
            )
        {
            Vec::new()
        } else {
            choices
        }
    }

    pub fn resolve(&self, requested: &EngineConfig) -> ResolvedModelConfig {
        let mut effective = requested.clone();
        let mut changes = Vec::new();
        for (axis, value, fallback) in [
            (
                ModelAxis::Thinking,
                requested.effort.clone(),
                ConfigBinding::Effort(None),
            ),
            (
                ModelAxis::Variant,
                requested.variant.clone(),
                ConfigBinding::Variant(None),
            ),
            (
                ModelAxis::Speed,
                requested.speed.clone(),
                ConfigBinding::Speed(None),
            ),
        ] {
            if let Some(value) = value {
                let offered = self
                    .controls
                    .iter()
                    .flat_map(|control| &control.choices)
                    .any(|choice| match (&choice.binding, &fallback) {
                        (ConfigBinding::Effort(Some(id)), ConfigBinding::Effort(_))
                        | (ConfigBinding::Variant(Some(id)), ConfigBinding::Variant(_))
                        | (ConfigBinding::Speed(Some(id)), ConfigBinding::Speed(_)) => id == &value,
                        _ => false,
                    });
                if !offered {
                    effective = fallback.apply(&effective);
                    changes.push(ResolutionChange {
                        axis,
                        requested: value,
                        reason: "No longer advertised; using the harness default.".into(),
                    });
                }
            }
        }
        for (axis, key, value) in [
            (
                ModelAxis::Context,
                "context_window",
                requested.context_window_override(),
            ),
            (
                ModelAxis::Compaction,
                "compaction_window",
                requested.compaction_window_override(),
            ),
        ] {
            if requested.options.contains_key(key) {
                let offered = value.is_some()
                    && self
                        .controls
                        .iter()
                        .find(|control| control.axis == axis)
                        .is_some_and(|control| {
                            control
                                .choices
                                .iter()
                                .any(|choice| choice.binding.selected(requested))
                        });
                if !offered {
                    effective.options.remove(key);
                    changes.push(ResolutionChange {
                        axis,
                        requested: requested.options[key].to_string(),
                        reason: "No longer offered; using the harness default.".into(),
                    });
                }
            }
        }
        ResolvedModelConfig {
            requested: requested.clone(),
            effective,
            changes,
        }
    }
}
