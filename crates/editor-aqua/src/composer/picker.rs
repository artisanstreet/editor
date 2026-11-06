use artisan_api::{EngineConfig, EngineId, Model, Permissions};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Variant,
    Thinking,
    Speed,
    Context,
    Permission,
}

impl Axis {
    pub const ALL: [Self; 5] = [
        Self::Variant,
        Self::Thinking,
        Self::Speed,
        Self::Context,
        Self::Permission,
    ];
    pub const fn title(self) -> &'static str {
        match self {
            Self::Variant => "Variants",
            Self::Thinking => "Efforts",
            Self::Speed => "Speed",
            Self::Context => "Context Window",
            Self::Permission => "Permissions",
        }
    }
    pub const fn shown(self, options: usize) -> bool {
        match self {
            Self::Thinking | Self::Context => options > 0,
            Self::Variant | Self::Speed | Self::Permission => options > 1,
        }
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

fn context_label(tokens: u64) -> String {
    if tokens >= 1_000_000 && tokens.is_multiple_of(1_000_000) {
        format!("{}M", tokens / 1_000_000)
    } else {
        format!("{}K", tokens / 1_000)
    }
}

fn humanize(word: &str) -> String {
    if word == "xhigh" {
        return "Extra High".to_owned();
    }
    let mut out = String::with_capacity(word.len());
    for (index, part) in word.split(['_', '-']).enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            if index == 0 {
                out.extend(first.to_uppercase());
            } else {
                out.push(first);
            }
            out.push_str(chars.as_str());
        }
    }
    out
}

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

pub fn axis_options(axis: Axis, config: &EngineConfig, model: Option<&Model>) -> Vec<AxisOption> {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelRow {
    Group { name: String, open: bool },
    Model { id: String, index: usize },
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
    pub menu: Option<Menu>,
    pub browsing: Option<EngineId>,
    pub axis: Option<Axis>,
    pub axis_cursor: Option<AxisCursor>,
    pub models: Vec<ModelRow>,
    pub projects: Vec<artisan_api::ProjectId>,
    pub preview_model: Option<(EngineId, String)>,
    pub project_edges: crate::ui::scroll_edge::ScrollEdges,
    groups: std::collections::HashMap<(EngineId, String), bool>,
}
impl PickerState {
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
        let engine = self
            .browsing
            .or_else(|| current.map(|config| config.engine));
        let chosen = current
            .filter(|config| Some(config.engine) == engine)
            .and_then(|config| config.model.as_deref());
        self.models = engine
            .and_then(|engine| Some((engine, host.engines.get(&engine)?)))
            .map(|(engine, status)| {
                let mut models: Vec<_> = status.models.iter().enumerate().collect();
                if models.iter().all(|(_, model)| model.group.is_none()) {
                    models
                        .sort_by_key(|(index, model)| (chosen != Some(model.id.as_str()), *index));
                    return models
                        .into_iter()
                        .map(|(index, model)| ModelRow::Model {
                            id: model.id.clone(),
                            index,
                        })
                        .collect();
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
                let mut rows = Vec::new();
                for (position, (name, members)) in groups.into_iter().enumerate() {
                    let open = self
                        .groups
                        .get(&(engine, name.to_owned()))
                        .copied()
                        .or_else(|| saved?.get(name)?.as_bool())
                        .unwrap_or_else(|| selected.unwrap_or(0) == position);
                    rows.push(ModelRow::Group {
                        name: name.to_owned(),
                        open,
                    });
                    if open {
                        rows.extend(members.into_iter().map(|(index, model)| ModelRow::Model {
                            id: model.id.clone(),
                            index,
                        }));
                    }
                }
                rows
            })
            .unwrap_or_default();
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
        let current = current.filter(|config| config.engine == engine);
        let mut config = match current {
            Some(config) if config.model.as_deref() == Some(id) => config,
            current => host
                .setting(artisan_api::MODEL_CONFIGS_KEY)
                .and_then(|setting| artisan_api::saved_model_config(setting, engine, id))
                .unwrap_or_else(|| {
                    let mut config = current.unwrap_or_else(|| EngineConfig::new(engine));
                    config.model = Some(id.to_owned());
                    config.effort.clone_from(&model.default_effort);
                    config.speed = None;
                    config.variant.clone_from(&model.default_variant);
                    config
                }),
        };
        model.fit(&mut config);
        Some(config)
    }
    pub fn preview_config(
        &self,
        host: &crate::core::HostData,
        current: Option<EngineConfig>,
    ) -> Option<EngineConfig> {
        let engine = self
            .browsing
            .or_else(|| current.as_ref().map(|config| config.engine))?;
        let id = self
            .preview_model
            .as_ref()
            .filter(|(preview_engine, _)| *preview_engine == engine)
            .map(|(_, id)| id.as_str())
            .or_else(|| {
                current
                    .as_ref()
                    .filter(|config| config.engine == engine)
                    .and_then(|config| config.model.as_deref())
            })
            .or_else(|| {
                self.models.iter().find_map(|row| match row {
                    ModelRow::Model { id, .. } => Some(id.as_str()),
                    _ => None,
                })
            })?;
        Self::model_config(host, current.clone(), engine, id)
    }
}
