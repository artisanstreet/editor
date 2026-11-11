use super::{
    Capability, ChoiceSemantic, ConfigBinding, EngineConfig, EngineId, Model, ModelAxis,
    ModelChoice, ModelCustomization, ModelIdentity, ModelSpecification, OptionOrigin, Permissions,
    ResolutionChange, ResolvedModelConfig, SpeedLevel, ThinkingLevel, native_label, reasoning_axis,
    reasoning_semantic, semantic_label, token_label,
};

impl Model {
    pub fn carry_thinking_preference(
        &self,
        source: &EngineConfig,
        target: &EngineConfig,
        customizations: &[ModelCustomization],
    ) -> EngineConfig {
        let mut result = target.clone();
        let level = source
            .options
            .get("thinking_level")
            .cloned()
            .and_then(|value| serde_json::from_value::<ThinkingLevel>(value).ok())
            .or_else(|| {
                source
                    .effort
                    .as_deref()
                    .or(source.variant.as_deref())
                    .and_then(ThinkingLevel::from_native)
            });
        let enabled = source
            .options
            .get("thinking_enabled")
            .and_then(serde_json::Value::as_bool);
        if let Some(level) = level {
            result
                .options
                .insert("thinking_level".into(), serde_json::json!(level));
        }
        if let Some(enabled) = enabled {
            result
                .options
                .insert("thinking_enabled".into(), enabled.into());
        }
        let spec = self.specification(target.engine, customizations);
        let choice = spec
            .controls
            .iter()
            .flat_map(|control| &control.choices)
            .find(|choice| match (enabled, level, &choice.semantic) {
                (Some(false), _, ChoiceSemantic::Activation(false)) => true,
                (Some(false), _, _) => false,
                (_, Some(level), ChoiceSemantic::Intensity(offered)) => level == *offered,
                _ => false,
            })
            .or_else(|| {
                spec.controls
                    .iter()
                    .flat_map(|control| &control.choices)
                    .find(|choice| {
                        enabled != Some(false)
                            && (level.is_some() || enabled == Some(true))
                            && choice.semantic == ChoiceSemantic::Activation(true)
                    })
            });
        choice.map_or_else(|| result.clone(), |choice| choice.binding.apply(&result))
    }

    pub fn migrate_config(&self, requested: &EngineConfig) -> Option<EngineConfig> {
        let legacy = requested.engine == EngineId::Claude
            && requested.compaction_window_override() == Some(500_000)
            && self
                .context_window
                .is_some_and(|capacity| capacity >= 1_000_000);
        legacy.then(|| {
            let mut migrated = requested.clone();
            migrated.options.remove("compaction_window");
            migrated
                .options
                .entry("context_window")
                .or_insert(500_000.into());
            migrated
        })
    }

    pub fn resolve_config(
        &self,
        requested: &EngineConfig,
        customizations: &[ModelCustomization],
    ) -> ResolvedModelConfig {
        let migrated = self.migrate_config(requested);
        let legacy = migrated.is_some();
        let mut resolved = self
            .specification(requested.engine, customizations)
            .resolve(migrated.as_ref().unwrap_or(requested));
        resolved.requested = requested.clone();
        if legacy {
            resolved.changes.push(ResolutionChange {
                axis: ModelAxis::Context,
                requested: "500000".into(),
                reason: "Migrated the Claude 500K compaction window to the 500K context choice."
                    .into(),
            });
        }
        resolved
    }

    pub fn specification(
        &self,
        harness: EngineId,
        customizations: &[ModelCustomization],
    ) -> ModelSpecification {
        let mut spec = ModelSpecification {
            identity: ModelIdentity {
                harness,
                native_id: self.id.clone(),
                gateway: self.group.clone(),
            },
            images: match self
                .evidence
                .sources
                .iter()
                .any(|source| source.field == "images")
            {
                _ if self.supports_images => Capability::Supported(true),
                true => Capability::Unsupported,
                false => Capability::Unknown,
            },
            context_capacity: self
                .context_windows
                .iter()
                .map(|window| window.tokens)
                .chain(self.context_window)
                .filter(|tokens| (1..=100_000_000).contains(tokens))
                .max()
                .map_or(Capability::Unknown, Capability::Supported),
            controls: Vec::new(),
            evidence: self.evidence.clone(),
        };
        self.add_reasoning(&mut spec);
        self.add_speeds(&mut spec);
        self.add_windows(&mut spec);
        Self::add_permissions(&mut spec, harness);
        self.add_customizations(&mut spec, harness, customizations);
        Self::add_regroup(&mut spec);
        self.add_defaults(&mut spec);
        spec
    }
    fn add_reasoning(&self, spec: &mut ModelSpecification) {
        for effort in &self.efforts {
            let note = self.effort_notes.iter().find(|note| note.effort == *effort);
            let semantic = if note.is_some_and(|note| note.special) {
                ChoiceSemantic::Special
            } else {
                reasoning_semantic(effort)
            };
            spec.push(
                reasoning_axis(&semantic),
                ModelChoice {
                    id: effort.clone(),
                    label: semantic_label(&semantic, &native_label(effort)),
                    semantic,
                    binding: ConfigBinding::Effort(Some(effort.clone())),
                    description: note.and_then(|note| note.description.clone()),
                    advisory: note.and_then(|note| note.advisory.clone()),
                    origin: OptionOrigin::Discovered,
                    hidden: false,
                    visibility_override: None,
                },
            );
        }
        for variant in &self.variants {
            let semantic = reasoning_semantic(&variant.id);
            spec.push(
                reasoning_axis(&semantic),
                ModelChoice {
                    id: variant.id.clone(),
                    label: semantic_label(&semantic, &variant.label),
                    semantic,
                    binding: ConfigBinding::Variant(Some(variant.id.clone())),
                    description: None,
                    advisory: None,
                    origin: OptionOrigin::Discovered,
                    hidden: false,
                    visibility_override: None,
                },
            );
        }
    }

    fn add_speeds(&self, spec: &mut ModelSpecification) {
        for speed in &self.speeds {
            let semantic = SpeedLevel::from_native(&speed.id)
                .map_or(ChoiceSemantic::Native, ChoiceSemantic::Speed);
            spec.push(
                ModelAxis::Speed,
                ModelChoice {
                    id: speed.id.clone(),
                    label: semantic_label(&semantic, &speed.label),
                    semantic,
                    binding: ConfigBinding::Speed(Some(speed.id.clone())),
                    description: speed.description.clone(),
                    advisory: None,
                    origin: OptionOrigin::Discovered,
                    hidden: false,
                    visibility_override: None,
                },
            );
        }
    }

    fn add_windows(&self, spec: &mut ModelSpecification) {
        for window in &self.context_windows {
            if !(1..=100_000_000).contains(&window.tokens)
                || window
                    .override_tokens
                    .is_some_and(|tokens| !(1..=100_000_000).contains(&tokens))
            {
                continue;
            }
            spec.push(
                ModelAxis::Context,
                ModelChoice {
                    id: window.tokens.to_string(),
                    label: token_label(window.tokens),
                    semantic: match window.override_tokens {
                        None => ChoiceSemantic::Default,
                        Some(_) => ChoiceSemantic::Native,
                    },
                    binding: ConfigBinding::Context(window.override_tokens),
                    description: None,
                    advisory: None,
                    origin: OptionOrigin::Discovered,
                    hidden: false,
                    visibility_override: None,
                },
            );
        }
    }

    fn add_permissions(spec: &mut ModelSpecification, harness: EngineId) {
        for (value, label) in [
            (Permissions::Auto, "Auto"),
            (Permissions::Full, "Full access"),
        ] {
            let description = match (harness, value) {
                (EngineId::Codex, Permissions::Auto) => {
                    "Sandboxed to the project; Codex's reviewer decides what else may run."
                }
                (EngineId::Claude, Permissions::Auto) => {
                    "Claude's auto mode: safe actions run, risky ones are checked first."
                }
                (EngineId::OpenCode, Permissions::Auto) => {
                    "Edits run; it asks before terminal commands."
                }
                (_, Permissions::Full) => "Everything runs, nothing asks. No sandbox.",
            };
            spec.push(
                ModelAxis::Permission,
                ModelChoice {
                    id: label.into(),
                    label: label.into(),
                    semantic: ChoiceSemantic::Native,
                    binding: ConfigBinding::Permission(value),
                    description: Some(description.into()),
                    advisory: None,
                    origin: OptionOrigin::HarnessPolicy,
                    hidden: false,
                    visibility_override: None,
                },
            );
        }
    }

    fn add_defaults(&self, spec: &mut ModelSpecification) {
        for control in &mut spec.controls {
            control.choices.sort_by_key(|choice| {
                (
                    match choice.semantic {
                        ChoiceSemantic::Intensity(level) => match level {
                            ThinkingLevel::Light => 0,
                            ThinkingLevel::Medium => 1,
                            ThinkingLevel::High => 2,
                            ThinkingLevel::ExtraHigh => 3,
                            ThinkingLevel::Max => 4,
                        },
                        ChoiceSemantic::Speed(level) => match level {
                            SpeedLevel::Normal => 0,
                            SpeedLevel::Fast => 1,
                            SpeedLevel::Superfast => 2,
                        },
                        ChoiceSemantic::Activation(enabled) => u8::from(enabled),
                        ChoiceSemantic::Default
                        | ChoiceSemantic::Special
                        | ChoiceSemantic::Native => 10,
                    },
                    choice.id == "minimal",
                )
            });
            let first = control.choices.first().map(|choice| choice.binding.clone());
            let default_binding = match first {
                Some(ConfigBinding::Effort(_)) => Some(ConfigBinding::Effort(None)),
                Some(ConfigBinding::Variant(_)) => Some(ConfigBinding::Variant(None)),
                Some(ConfigBinding::Speed(_)) => Some(ConfigBinding::Speed(None)),
                None
                | Some(
                    ConfigBinding::Context(_)
                    | ConfigBinding::Compaction(_)
                    | ConfigBinding::Permission(_),
                ) => None,
            };
            if let Some(binding) = default_binding {
                let reported = match &binding {
                    ConfigBinding::Effort(_) => self.default_effort.as_ref(),
                    ConfigBinding::Variant(_) => self.default_variant.as_ref(),
                    ConfigBinding::Speed(_) => self
                        .speeds
                        .iter()
                        .find(|speed| speed.default)
                        .map(|speed| &speed.id),
                    ConfigBinding::Context(_)
                    | ConfigBinding::Compaction(_)
                    | ConfigBinding::Permission(_) => None,
                };
                let label = reported
                    .and_then(|id| control.choices.iter().find(|choice| &choice.id == id))
                    .map_or_else(
                        || "Default".into(),
                        |choice| format!("Default · {}", choice.label),
                    );
                control.choices.insert(
                    0,
                    ModelChoice {
                        id: "default".into(),
                        label,
                        semantic: ChoiceSemantic::Default,
                        binding,
                        description: Some(
                            "Use the harness default without pinning a value.".into(),
                        ),
                        advisory: None,
                        origin: OptionOrigin::HarnessPolicy,
                        hidden: false,
                        visibility_override: None,
                    },
                );
            }
        }
    }

    fn add_customizations(
        &self,
        spec: &mut ModelSpecification,
        harness: EngineId,
        customizations: &[ModelCustomization],
    ) {
        let mut applicable: Vec<_> = customizations
            .iter()
            .filter(|rule| rule.scope.matches(&spec.identity))
            .collect();
        applicable.sort_by_key(|rule| rule.scope.specificity());
        for rule in applicable {
            if rule.axis == ModelAxis::Compaction
                && harness == EngineId::Claude
                && let Ok(tokens) = rule.native_id.parse::<u64>()
                && tokens > 0
                && tokens <= self.context_window.unwrap_or_default()
                && !spec.controls.iter().any(|control| {
                    control.axis == ModelAxis::Compaction
                        && control
                            .choices
                            .iter()
                            .any(|choice| choice.id == rule.native_id)
                })
            {
                if !spec
                    .controls
                    .iter()
                    .any(|control| control.axis == ModelAxis::Compaction)
                {
                    spec.push(
                        ModelAxis::Compaction,
                        ModelChoice {
                            id: "default".into(),
                            label: "Default".into(),
                            semantic: ChoiceSemantic::Default,
                            binding: ConfigBinding::Compaction(None),
                            description: None,
                            advisory: None,
                            origin: OptionOrigin::HarnessPolicy,
                            hidden: false,
                            visibility_override: None,
                        },
                    );
                }
                spec.push(
                    ModelAxis::Compaction,
                    ModelChoice {
                        id: rule.native_id.clone(),
                        label: token_label(tokens),
                        semantic: ChoiceSemantic::Native,
                        binding: ConfigBinding::Compaction(Some(tokens)),
                        description: Some(
                            "Custom compaction window; model capacity is unchanged.".into(),
                        ),
                        advisory: None,
                        origin: OptionOrigin::Customization(rule.name.clone()),
                        hidden: false,
                        visibility_override: None,
                    },
                );
            }
            if let Some(choice) = spec
                .controls
                .iter_mut()
                .find(|control| control.axis == rule.axis)
                .and_then(|control| {
                    control
                        .choices
                        .iter_mut()
                        .find(|choice| choice.id == rule.native_id)
                })
            {
                if let Some(label) = &rule.label {
                    choice.label.clone_from(label);
                }
                if let Some(semantic) = &rule.semantic {
                    if rule.label.is_none() {
                        choice.label = semantic_label(semantic, &choice.label);
                    }
                    choice.semantic.clone_from(semantic);
                }
                if let Some(hidden) = rule.hidden {
                    choice.visibility_override = Some(hidden);
                }
                choice.origin = OptionOrigin::Customization(rule.name.clone());
            }
        }
    }

    fn add_regroup(spec: &mut ModelSpecification) {
        let controls = std::mem::take(&mut spec.controls);
        for control in controls {
            for choice in control.choices {
                let axis = match (&choice.semantic, control.axis) {
                    (ChoiceSemantic::Default, axis) => axis,
                    (
                        _,
                        ModelAxis::Thinking
                        | ModelAxis::Activation
                        | ModelAxis::Special
                        | ModelAxis::Variant,
                    ) => reasoning_axis(&choice.semantic),
                    (_, axis) => axis,
                };
                spec.push(axis, choice);
            }
        }
    }
}
