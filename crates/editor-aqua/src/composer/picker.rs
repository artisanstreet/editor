use artisan_api::{EngineConfig, EngineId, Model};
pub use artisan_api::ModelAxis as Axis;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TooltipPlacement {
    Above,
    Beside,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TooltipContent {
    pub anchor: String,
    pub advisory: Option<String>,
    pub text: String,
    pub placement: TooltipPlacement,
}

#[derive(Default)]
pub struct PickerTooltip {
    pending: Option<(TooltipContent, std::time::Instant)>,
    visible: bool,
    revision: u64,
}

impl PickerTooltip {
    pub fn begin(&mut self, content: TooltipContent, now: std::time::Instant) {
        self.pending = Some((content, now + std::time::Duration::from_millis(500)));
        self.visible = false;
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn clear(&mut self) {
        if self.pending.take().is_some() {
            self.visible = false;
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn clear_for(&mut self, anchor: &str) {
        if self
            .pending
            .as_ref()
            .is_some_and(|(content, _)| content.anchor == anchor)
        {
            self.clear();
        }
    }

    pub fn deadline(&self) -> Option<std::time::Instant> {
        self.pending
            .as_ref()
            .filter(|_| !self.visible)
            .map(|(_, deadline)| *deadline)
    }

    pub fn show_due(&mut self, now: std::time::Instant) -> bool {
        if !self.visible
            && self
                .pending
                .as_ref()
                .is_some_and(|(_, deadline)| now >= *deadline)
        {
            self.visible = true;
            self.revision = self.revision.wrapping_add(1);
            return true;
        }
        false
    }

    pub fn shown(&self) -> Option<&TooltipContent> {
        self.pending
            .as_ref()
            .filter(|_| self.visible)
            .map(|(content, _)| content)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

pub struct AxisOption {
    pub label: String,
    pub special: bool,
    pub description: Option<String>,
    pub advisory: Option<String>,
    pub selected: bool,
    pub apply: EngineConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelRow {
    Engine(EngineId),
    Gate(EngineId),
    Group {
        engine: EngineId,
        name: String,
        open: bool,
    },
    Model {
        engine: EngineId,
        id: String,
        index: usize,
    },
}

impl ModelRow {
    #[must_use]
    pub const fn engine(&self) -> EngineId {
        match self {
            Self::Engine(engine) | Self::Gate(engine) => *engine,
            Self::Group { engine, .. } | Self::Model { engine, .. } => *engine,
        }
    }
}

#[must_use]
pub fn gate(status: &artisan_api::EngineStatus) -> Option<&'static str> {
    use artisan_api::{InstallState, SignInState};
    if !matches!(status.install, InstallState::Installed { .. }) {
        return Some("Not installed");
    }
    match &status.sign_in {
        SignInState::SignedOut
            if status.engine == EngineId::OpenCode && !status.models.is_empty() =>
        {
            None
        }
        SignInState::SignedOut | SignInState::Failed { .. } => Some("Sign in"),
        SignInState::LoggingIn { .. } => Some("Signing in…"),
        SignInState::Unknown if status.models.is_empty() => Some("Checking…"),
        SignInState::SignedIn { .. } if status.models.is_empty() => match status.discovery {
            artisan_api::DiscoveryState::Unknown | artisan_api::DiscoveryState::Refreshing { .. } => Some("Discovering models…"),
            artisan_api::DiscoveryState::Unavailable { .. } | artisan_api::DiscoveryState::Stale { .. } => Some("Discovery failed"),
            artisan_api::DiscoveryState::Fresh { .. } => Some("No models"),
        },
        SignInState::Unknown | SignInState::SignedIn { .. } => None,
    }
}

#[must_use]
pub fn offered_engines(
    host: &crate::core::HostData,
    current: Option<EngineId>,
) -> Vec<&artisan_api::EngineStatus> {
    host.engines
        .values()
        .filter(|status| gate(status).is_none() || current == Some(status.engine))
        .collect()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Model,
    Project,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisKey {
    Home,
    End,
    Up,
    Down,
    Accept,
    Dismiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisKeyResult {
    Moved,
    Choose(usize),
    Dismiss,
    Ignored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisCursor {
    Pointer(usize),
    Keys(usize),
}
impl AxisCursor {
    fn index(self) -> usize {
        match self {
            Self::Pointer(index) | Self::Keys(index) => index,
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct PickerState {
    pub show_all_models: bool,
    pub hide_bad_model_options: bool,
    pub customizations: Vec<artisan_api::ModelCustomization>,
    pub menu: Option<Menu>,
    pub open_serial: u64,
    pub axis: Option<Axis>,
    pub axis_cursor: Option<AxisCursor>,
    pub models: Vec<ModelRow>,
    pub projects: Vec<artisan_api::ProjectId>,
    pub project_edges: crate::ui::scroll_edge::ScrollEdges,
    pub model_edges: crate::ui::scroll_edge::ScrollEdges,
    groups: std::collections::HashMap<(EngineId, String), bool>,
}
impl PickerState {
    pub fn refresh_settings(&mut self, core: &crate::core::Core, host: &crate::core::HostData) {
        self.show_all_models = core.settings
            .get(artisan_api::SHOW_ALL_MODELS_KEY).and_then(serde_json::Value::as_bool).unwrap_or(false);
        self.hide_bad_model_options = core.settings
            .get(artisan_api::HIDE_BAD_MODEL_OPTIONS_KEY).and_then(serde_json::Value::as_bool).unwrap_or(true);
        self.customizations = host.setting(artisan_api::MODEL_CUSTOMIZATIONS_KEY)
            .and_then(|value| artisan_api::model_customizations(value).ok()).unwrap_or_default();
    }

    pub fn for_target(core: &crate::core::Core, host: &crate::core::HostId) -> Self {
        let mut picker = Self::default();
        if let Some(host) = core.host(host) {
            picker.refresh_settings(core, host);
        }
        picker
    }

    pub fn axis_options(
        &self,
        axis: Axis,
        config: &EngineConfig,
        model: Option<&Model>,
    ) -> Vec<AxisOption> {
        let Some(model) = model else {
            return Vec::new();
        };
        let mut effective = model.resolve_config(config, &self.customizations).effective;
        effective.effort = effective.effort.or_else(|| model.default_effort.clone());
        effective.variant = effective.variant.or_else(|| model.default_variant.clone());
        effective.speed = effective.speed.or_else(|| {
            model
                .speeds
                .iter()
                .find(|speed| speed.default)
                .map(|speed| speed.id.clone())
        });
        model
            .specification(config.engine, &self.customizations)
            .visible_choices(axis, &effective, self.hide_bad_model_options)
            .into_iter()
            .filter(|choice| {
                choice.semantic != artisan_api::ChoiceSemantic::Default
                    || matches!(choice.binding, artisan_api::ConfigBinding::Context(_))
            })
            .map(|choice| {
                let description = match choice.hidden {
                    true => Some(format!(
                        "Selected option hidden by preference. {}",
                        choice.description.as_deref().unwrap_or_default()
                    )),
                    false => choice.description.clone(),
                };
                AxisOption {
                    label: choice.label.clone(),
                    special: matches!(choice.semantic, artisan_api::ChoiceSemantic::Special),
                    description,
                    advisory: choice.advisory.clone(),
                    selected: choice.binding.selected(&effective),
                    apply: choice.apply(config),
                }
            })
            .collect()
    }

    pub fn keyboard_axis_cursor(&self) -> Option<usize> {
        match self.axis_cursor {
            Some(AxisCursor::Keys(index)) => Some(index),
            _ => None,
        }
    }

    pub fn axis_key(&mut self, key: AxisKey, selected: usize, count: usize) -> AxisKeyResult {
        if count == 0 {
            return AxisKeyResult::Ignored;
        }
        let current = self
            .axis_cursor
            .map_or(selected, AxisCursor::index)
            .min(count - 1);
        let next = match key {
            AxisKey::Home => 0,
            AxisKey::End => count - 1,
            AxisKey::Up => current.saturating_sub(1),
            AxisKey::Down => (current + 1).min(count - 1),
            AxisKey::Accept => return AxisKeyResult::Choose(current),
            AxisKey::Dismiss => return AxisKeyResult::Dismiss,
        };
        self.axis_cursor = Some(AxisCursor::Keys(next));
        AxisKeyResult::Moved
    }

    pub fn refresh(&mut self, host: &crate::core::HostData, current: Option<&EngineConfig>) {
        if self.menu.is_none() {
            return;
        }
        let current_engine = current.map(|config| config.engine);
        let chosen = current.and_then(|config| config.model.as_deref());
        let mut rows = Vec::new();
        for status in offered_engines(host, current_engine) {
            self.push_engine_rows(host, status, chosen, current_engine, &mut rows);
        }
        self.models = rows;
        let mut projects: Vec<_> = host
            .projects
            .values()
            .filter(|project| !project.removed && !project.id.is_scratch())
            .collect();
        let mut activity = std::collections::HashMap::new();
        for thread in host.threads.values() {
            let at = thread.activity_at.max(thread.created_at);
            activity
                .entry(&thread.project_id)
                .and_modify(|latest: &mut i64| *latest = (*latest).max(at))
                .or_insert(at);
        }
        projects.sort_by(|a, b| {
            let recent = |project: &artisan_api::Project| {
                activity
                    .get(&project.id)
                    .copied()
                    .unwrap_or_default()
                    .max(project.created_at)
            };
            recent(b)
                .cmp(&recent(a))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });
        self.projects = projects
            .into_iter()
            .map(|project| project.id.clone())
            .collect();
    }
    pub fn toggle_group(&mut self, engine: EngineId, name: String, open: bool) {
        self.groups.insert((engine, name), !open);
    }
    pub fn model_config(
        host: &crate::core::HostData,
        current: Option<EngineConfig>,
        engine: EngineId,
        id: &str,
    ) -> Option<EngineConfig> {
        let model = host
            .engines
            .get(&engine)?
            .models
            .iter()
            .find(|model| model.id == id)?;
        let preference = current.clone();
        let current = current.filter(|config| config.engine == engine);
        let saved = host.setting(artisan_api::MODEL_CONFIGS_KEY)
            .and_then(|setting| artisan_api::saved_model_config(setting, engine, id));
        let carry = saved.is_none() && preference.as_ref().is_some_and(|config| config.model.as_deref() != Some(id) || config.engine != engine);
        let mut config = match current {
            Some(config) if config.model.as_deref() == Some(id) => config,
            current => host
                .setting(artisan_api::MODEL_CONFIGS_KEY)
                .and_then(|setting| artisan_api::saved_model_config(setting, engine, id))
                .unwrap_or_else(|| {
                    let mut config = current.unwrap_or_else(|| EngineConfig::new(engine));
                    config.model = Some(id.to_owned());
                    config.effort = None;
                    config.speed = None;
                    config.variant = None;
                    config
                }),
        };
        let customizations = host.setting(artisan_api::MODEL_CUSTOMIZATIONS_KEY)
            .and_then(|value| artisan_api::model_customizations(value).ok()).unwrap_or_default();
        if carry && let Some(preference) = &preference {
            config = model.carry_thinking_preference(preference, &config, &customizations);
        }
        config = model.resolve_config(&config, &customizations).effective;
        Some(config)
    }
    pub fn pane_config(
        &self,
        host: &crate::core::HostData,
        current: Option<EngineConfig>,
    ) -> Option<EngineConfig> {
        let engine = current.as_ref()?.engine;
        let id = current
            .as_ref()
            .and_then(|config| config.model.as_deref())
            .map(str::to_owned)
            .or_else(|| {
                self.models.iter().find_map(|row| match row {
                    ModelRow::Model {
                        engine: row_engine,
                        id,
                        ..
                    } if *row_engine == engine => Some(id.clone()),
                    _ => None,
                })
            })?;
        Self::model_config(host, current, engine, &id)
    }

    fn push_engine_rows(
        &self,
        host: &crate::core::HostData,
        status: &artisan_api::EngineStatus,
        chosen: Option<&str>,
        current_engine: Option<EngineId>,
        rows: &mut Vec<ModelRow>,
    ) {
        let engine = status.engine;
        let gated = gate(status);
        if matches!(gated, Some("Not installed" | "Sign in")) || (gated.is_some() && status.models.is_empty()) {
            rows.push(ModelRow::Engine(engine));
            rows.push(ModelRow::Gate(engine));
            return;
        }
        let chosen = chosen.filter(|_| current_engine == Some(engine));
        let models: Vec<_> =
            super::model_line::visible_models(engine, &status.models, self.show_all_models)
                .into_iter()
                .map(|index| (index, &status.models[index]))
                .collect();
        if models.iter().all(|(_, model)| model.group.is_none()) {
            rows.push(ModelRow::Engine(engine));
            rows.extend(models.into_iter().map(|(index, model)| ModelRow::Model {
                engine,
                id: model.id.clone(),
                index,
            }));
            return;
        }
        let mut groups: Vec<(&str, Vec<(usize, &Model)>)> = Vec::new();
        for (index, model) in models {
            let name = model.group.as_deref().unwrap_or_default();
            match groups.iter_mut().find(|(group, _)| *group == name) {
                Some((_, members)) => members.push((index, model)),
                None => groups.push((name, vec![(index, model)])),
            }
        }
        let selected = groups.iter().position(|(_, members)| {
            members
                .iter()
                .any(|(_, model)| chosen == Some(model.id.as_str()))
        });
        let saved = host
            .setting(artisan_api::MODEL_GROUPS_KEY)
            .and_then(|groups| groups.get(engine.as_str()));
        for (position, (name, members)) in groups.into_iter().enumerate() {
            let open = self
                .groups
                .get(&(engine, name.to_owned()))
                .copied()
                .or_else(|| saved?.get(name)?.as_bool())
                .unwrap_or_else(|| selected.unwrap_or(0) == position);
            rows.push(ModelRow::Group {
                engine,
                name: name.to_owned(),
                open,
            });
            if open {
                rows.extend(members.into_iter().map(|(index, model)| ModelRow::Model {
                    engine,
                    id: model.id.clone(),
                    index,
                }));
            }
        }
    }
}
