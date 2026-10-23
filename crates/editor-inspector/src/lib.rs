//! Current thread state comes from Forge summaries, independently of transcript paging.
mod model;
mod terminal_dialog;
use artisan_api::{Collection, InspectorSummary, ItemId, Status, StepStatus, ThreadId};
use editor_conversation::{Conversation, ConversationEvent};
use editor_core::{Core, CoreEvent, HostData, HostEvent, HostId};
use editor_ui::theme::{ActiveTheme as _, RadiusStep, RadiusTokens};
use editor_ui::{AssetId, button::Button, terminal_text::TerminalScreen, theme::ArtisanTheme};
use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ListAlignment,
    ListState, ParentElement as _, PulseExt as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, list,
    prelude::FluentBuilder as _, px,
};
use std::sync::Arc;
pub use terminal_dialog::{TERMINAL_DIALOG_SELECTOR, TerminalDialog};

#[derive(Clone)]
enum Entry {
    Fact(&'static str),
    Heading(&'static str),
    Agent(usize),
    Plan(usize),
    PlanCollection,
    Terminal(usize),
    Workflow(usize),
}
pub struct Inspector {
    core: Entity<Core>,
    conversation: Entity<Conversation>,
    host: Option<(Entity<HostData>, Subscription)>,
    open: Option<(HostId, ThreadId)>,
    summary: Arc<InspectorSummary>,
    entries: Vec<Entry>,
    list: ListState,
    #[cfg(test)]
    test_rendered_entries: std::cell::RefCell<Vec<usize>>,
    #[cfg(test)]
    test_plan_presented: std::rc::Rc<std::cell::RefCell<Vec<u64>>>,
    error: Option<String>,
    dialog: Entity<TerminalDialog>,
    source: Option<Entity<editor_sources::ContentText>>,
    plan: Option<Entity<editor_sources::CollectionBrowser<artisan_api::PlanStep>>>,
    _subscriptions: [Subscription; 2],
}
impl Inspector {
    pub fn new(
        core: Entity<Core>,
        conversation: Entity<Conversation>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.subscribe(&conversation, |this, _, _: &ConversationEvent, cx| {
                this.follow(cx);
            }),
            cx.subscribe(&core, |this, _, _: &CoreEvent, cx| this.follow(cx)),
        ];
        let dialog = cx.new(|cx| TerminalDialog::new(core.clone(), conversation.clone(), cx));
        let mut this = Self {
            core,
            conversation,
            host: None,
            open: None,
            summary: Arc::default(),
            entries: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(0.0)),
            #[cfg(test)]
            test_rendered_entries: std::cell::RefCell::new(Vec::new()),
            #[cfg(test)]
            test_plan_presented: std::rc::Rc::default(),
            error: None,
            dialog,
            source: None,
            plan: None,
            _subscriptions: subscriptions,
        };
        this.follow(cx);
        this
    }
    pub const fn core(&self) -> &Entity<Core> {
        &self.core
    }
    pub const fn conversation(&self) -> &Entity<Conversation> {
        &self.conversation
    }
    pub const fn terminal_dialog(&self) -> &Entity<TerminalDialog> {
        &self.dialog
    }
    pub fn open_terminal(&self, id: &ItemId, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog
            .update(cx, |dialog, cx| dialog.open(id, window, cx));
    }
    fn follow(&mut self, cx: &mut Context<Self>) {
        let open = self.conversation.read(cx).open_thread().cloned();
        if self.open == open {
            return;
        }
        self.source = None;
        self.plan = None;
        self.open = open;
        self.summary = Arc::default();
        self.entries.clear();
        self.list.reset(0);
        self.error = None;
        self.host = self
            .open
            .as_ref()
            .and_then(|(id, _)| self.core.read(cx).host(id).cloned())
            .map(|host| {
                let subscription = cx.subscribe(&host, |this, _, event: &HostEvent, cx| {
                    let Some((_, thread)) = &this.open else {
                        return;
                    };
                    match event {
                        HostEvent::Reset => this.sync_summary(cx),
                        HostEvent::ThreadSummaryChanged { thread_id } if thread_id == thread => {
                            this.sync_summary(cx);
                        }
                        HostEvent::Changed | HostEvent::ThreadChanged { .. } => cx.notify(),
                        HostEvent::ImagesChanged
                        | HostEvent::EngineInstall { .. }
                        | HostEvent::ThreadSummaryChanged { .. }
                        | HostEvent::WslDistro { .. }
                        | HostEvent::Item(_)
                        | HostEvent::Live { .. }
                        | HostEvent::EditorCall { .. } => {}
                    }
                });
                (host, subscription)
            });
        self.sync_summary(cx);
    }
    fn sync_summary(&mut self, cx: &mut Context<Self>) {
        let state = self.host.as_ref().and_then(|(host, _)| {
            self.open.as_ref().and_then(|(_, thread)| {
                host.read(cx)
                    .thread_summary(thread)
                    .map(|state| (state.value.clone(), state.error.clone()))
            })
        });
        let (value, error) = state.unwrap_or_default();
        self.error = error;
        let changed = value
            .as_ref()
            .is_none_or(|value| !Arc::ptr_eq(value, &self.summary));
        if changed {
            self.summary = value.unwrap_or_default();
            match (&self.plan, self.summary.plan.stored()) {
                (Some(plan), Some(source)) => {
                    plan.update(cx, |plan, cx| plan.set_source(source.clone(), cx));
                }
                (_, None) => self.plan = None,
                _ => {}
            }
            self.rebuild();
        }
        cx.notify();
    }
    fn retry_summary(&self, cx: &mut Context<Self>) {
        if let (Some((host, _)), Some((_, thread))) = (&self.host, &self.open) {
            host.update(cx, |host, cx| host.load_thread_summary(thread, true, cx));
        }
    }
    fn rebuild(&mut self) {
        let previous_count = self.entries.len();
        let mut scroll = self.list.logical_scroll_top();
        self.entries = ["Project", "Forge", "Branch", "Worktree"]
            .into_iter()
            .map(Entry::Fact)
            .collect();
        if self.summary.edits.is_some() {
            self.entries.push(Entry::Fact("Edit operations"));
        }
        let summary = &self.summary;
        let terminals = |background: bool| {
            (0..summary.terminals.len())
                .filter(move |&index| summary.terminals[index].background == background)
                .map(Entry::Terminal)
        };
        let section = |entries: &mut Vec<Entry>, heading, rows: Vec<Entry>| {
            if !rows.is_empty() {
                entries.push(Entry::Heading(heading));
                entries.extend(rows);
            }
        };
        // Every agent, waited on or not, lives under Agents.
        section(
            &mut self.entries,
            "Agents",
            (0..summary.agents.len()).map(Entry::Agent).collect(),
        );
        if !self.summary.plan.is_empty() {
            self.entries.push(Entry::Heading("Checklist"));
            match &self.summary.plan {
                Collection::Inline(steps) => self.entries.extend((0..steps.len()).map(Entry::Plan)),
                Collection::Stored(_) => self.entries.push(Entry::PlanCollection),
            }
        }
        section(&mut self.entries, "Terminals", terminals(false).collect());
        // What runs on with nothing waiting for it: workflows, commands and
        // watchers.
        section(
            &mut self.entries,
            "In the background",
            (0..summary.workflows.len())
                .map(Entry::Workflow)
                .chain(terminals(true))
                .collect(),
        );
        self.list.splice(0..previous_count, self.entries.len());
        if !self.entries.is_empty() {
            scroll.item_ix = scroll.item_ix.min(self.entries.len() - 1);
            self.list.scroll_to(scroll);
        }
    }
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per entry kind, each a short element tree"
    )]
    fn render_entry(
        &self,
        index: usize,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        // Row closures outlive this call and keep their own copy.
        let theme = *theme;
        #[cfg(test)]
        self.test_rendered_entries.borrow_mut().push(index);

        let row = div()
            .py(px(8.0))
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(RadiusTokens::value(RadiusStep::Md))
            .text_size(theme.typography.control_text);
        #[cfg(test)]
        let row = row.debug_selector(move || format!("inspector-entry-{index}"));
        let Some(entry) = self.entries.get(index) else {
            return row.into_any_element();
        };
        // Section rules run edge to edge across the panel; rows keep the
        // panel's inset inside them. The project facts always close with one.
        let fact = |entry: Option<&Entry>| matches!(entry, Some(Entry::Fact(_)));
        let after_facts = index > 0 && fact(self.entries.get(index - 1));
        let rule = match entry {
            Entry::Fact(_) if !fact(self.entries.get(index + 1)) => Some(false),
            Entry::Heading(_) if !after_facts => Some(true),
            Entry::Fact(_)
            | Entry::Heading(_)
            | Entry::Agent(_)
            | Entry::Plan(_)
            | Entry::PlanCollection
            | Entry::Terminal(_)
            | Entry::Workflow(_) => None,
        };
        let line = theme.colors.line.to_paint();
        let content = (|| match entry {
            Entry::Heading(label) => row
                .pt(px(10.0))
                .pb(px(4.0))
                .text_size(theme.typography.label_text)
                .text_color(theme.colors.text_muted.to_paint())
                .child(div().flex_1().min_w_0().truncate().child(*label))
                .into_any_element(),
            Entry::Fact(label) => {
                let value = self.host.as_ref().and_then(|(host, _)| {
                    let host = host.read(cx);
                    let project = self
                        .open
                        .as_ref()
                        .and_then(|(_, thread)| host.threads.get(thread))
                        .and_then(|thread| host.projects.get(&thread.project_id));
                    match *label {
                        "Project" => project.map(|project| project.name.clone()),
                        "Forge" => Some(host.name.clone()),
                        "Branch" => project
                            .and_then(|project| host.repository(&project.id))
                            .and_then(|repo| repo.branch.clone()),
                        "Worktree" => project
                            .and_then(|project| host.repository(&project.id))
                            .and_then(|repo| repo.worktree.clone()),
                        "Edit operations" => self
                            .summary
                            .edits
                            .as_ref()
                            .map(|edits| format!("+{} / −{}", edits.added, edits.removed)),
                        _ => None,
                    }
                });
                let (icon, label) = match *label {
                    "Project" => (AssetId::TABLER_FOLDER, "Project"),
                    "Forge" => (AssetId::TABLER_DEVICE_LAPTOP, "Machine"),
                    "Branch" => (AssetId::TABLER_GIT_BRANCH, "Branch"),
                    "Worktree" => (AssetId::TABLER_FOLDER_CODE, "Worktree"),
                    "Edit operations" => (AssetId::TABLER_FILE_DIFF, "Changes"),
                    _ => (AssetId::TABLER_FOLDER, *label),
                };
                if label == "Changes"
                    && let Some(edits) = &self.summary.edits
                {
                    return row
                        .child(inspector_glyph(&theme, icon))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .child(
                            div()
                                .text_color(theme.colors.green.text.to_paint())
                                .child(format!("+{}", edits.added)),
                        )
                        .child(
                            div()
                                .text_color(theme.colors.red.text.to_paint())
                                .child(format!("−{}", edits.removed)),
                        )
                        .into_any_element();
                }
                row.child(inspector_glyph(&theme, icon))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .child(
                        div()
                            .min_w_0()
                            .max_w(px(150.0))
                            .truncate()
                            .child(value.unwrap_or_else(|| "—".into())),
                    )
                    .into_any_element()
            }
            Entry::Agent(index) => {
                let agent = &self.summary.agents[*index];
                let id = agent.id.clone();
                let thread = agent.thread.clone();
                let owner = cx.entity().downgrade();
                let color = match agent.status {
                    Status::Running | Status::AwaitingUser => theme.colors.text,
                    Status::Done => theme.colors.unread,
                    Status::Failed => theme.colors.red.solid,
                    Status::Declined | Status::Cancelled => theme.colors.text_muted,
                };
                let dot = div().size(px(6.0)).rounded_full().bg(color.to_paint());
                let dot = if agent.status == Status::Running && !cx.reduce_motion() {
                    // Breathes on the GPU, so the inspector never re-renders for it.
                    dot.with_pulse(
                        ("inspector-agent", *index),
                        std::time::Duration::from_millis(1600),
                        0.4,
                    )
                    .into_any_element()
                } else {
                    dot.into_any_element()
                };
                let card = self.agent_card(agent, cx);
                row.id(SharedString::from(format!("inspector-agent-{id}")))
                    .tooltip(move |_, cx| {
                        let card = card.clone();
                        editor_ui::hover_card::tooltip(
                            move |_, _| card.render(&theme).into_any_element(),
                            cx,
                        )
                    })
                    .focusable()
                    .tab_index(0)
                    .role(gpui::Role::Button)
                    .aria_label(agent.label.clone())
                    .cursor_pointer()
                    .hover(move |style| {
                        editor_ui::hover_card::hover_fill(
                            style,
                            &theme,
                            editor_ui::hover_card::HoverFill::Plain,
                        )
                    })
                    .on_click(move |_, _, cx| {
                        owner
                            .update(cx, |this, cx| {
                                // A harness agent has its own thread; open it.
                                if let (Some(thread), Some((host, _))) = (&thread, &this.host) {
                                    let screen = editor_core::Screen::Thread {
                                        host: host.read(cx).id.clone(),
                                        thread: thread.clone(),
                                    };
                                    this.core.update(cx, |core, cx| core.navigate(screen, cx));
                                    return;
                                }
                                this.conversation.update(cx, |conversation, cx| {
                                    conversation.open_agent(Some(id.clone()), cx);
                                });
                            })
                            .ok();
                    })
                    .child(agent.engine.map_or_else(
                        || inspector_glyph(&theme, AssetId::TABLER_BOT_ID).into_any_element(),
                        // Harness logos keep their own colours.
                        |engine| {
                            editor_ui::asset_seam::asset_glyph(editor_ui::engine_icon(
                                &theme, engine,
                            ))
                            .size(px(16.0))
                            .flex_shrink_0()
                            .into_any_element()
                        },
                    ))
                    .child(div().min_w_0().truncate().child(agent.label.clone()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.colors.text_muted.to_paint())
                            .child(match &agent.description {
                                artisan_api::TextContent::Inline(text) => {
                                    text.lines().next().unwrap_or_default().to_owned()
                                }
                                artisan_api::TextContent::Stored(_) => {
                                    "Stored description. Open agent to read.".into()
                                }
                            }),
                    )
                    .child(
                        div()
                            .size(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(dot),
                    )
                    .into_any_element()
            }
            Entry::PlanCollection => {
                let count = self.summary.plan.len();
                row.child(
                    Button::text("inspector-checklist", theme, format!("Read {count} steps"))
                        .debug_selector("inspector-checklist")
                        .on_activate(cx.listener(move |this, _, _, cx| {
                            let Some(source) = this.summary.plan.stored().cloned() else {
                                return;
                            };
                            let Some((host, _)) = &this.host else { return };
                            #[cfg(test)]
                            let presented = std::rc::Rc::clone(&this.test_plan_presented);
                            let present = std::rc::Rc::new(
                                move |step: &artisan_api::PlanStep, index, _: &gpui::App| {
                                    #[cfg(test)]
                                    presented.borrow_mut().push(index);
                                    editor_sources::CollectionPreview {
                                        label: match &step.text {
                                            artisan_api::TextContent::Inline(text) => format!(
                                                "{}. {}",
                                                index + 1,
                                                text.chars().take(160).collect::<String>()
                                            )
                                            .into(),
                                            artisan_api::TextContent::Stored(_) => {
                                                format!("Step {}", index + 1).into()
                                            }
                                        },
                                        detail: Some(step.text.clone()),
                                        selected: false,
                                        disabled: false,
                                    }
                                },
                            );
                            this.plan = Some(cx.new(|cx| {
                                editor_sources::CollectionBrowser::new(
                                    host,
                                    source,
                                    "Checklist",
                                    theme,
                                    present,
                                    None,
                                    cx,
                                )
                            }));
                            cx.notify();
                        })),
                )
                .into_any_element()
            }
            Entry::Plan(index) => {
                let Collection::Inline(steps) = &self.summary.plan else {
                    return row.child("Checklist is updating…").into_any_element();
                };
                let Some(step) = steps.get(*index) else {
                    return row.child("Step is no longer available.").into_any_element();
                };
                let status = match step.status {
                    StepStatus::Pending => "○",
                    StepStatus::InProgress => "●",
                    StepStatus::Done => "✓",
                    StepStatus::Cancelled => "−",
                };
                match &step.text {
                    artisan_api::TextContent::Inline(text) => {
                        // Every unresolved step stands in the foreground;
                        // resolved ones are crossed out and muted.
                        let resolved =
                            matches!(step.status, StepStatus::Done | StepStatus::Cancelled);
                        let row = if resolved { row.line_through() } else { row };
                        row.text_color(if resolved {
                            theme.colors.text_muted.to_paint()
                        } else {
                            theme.colors.text.to_paint()
                        })
                        .child(div().flex_1().min_w_0().truncate().child(text.clone()))
                        .into_any_element()
                    }
                    artisan_api::TextContent::Stored(source) => {
                        let source = source.clone();
                        row.child(
                            Button::text(
                                format!("inspector-plan-{index}"),
                                theme,
                                format!("{status} Read step"),
                            )
                            .on_activate(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some((host, _)) = &this.host {
                                        this.source = Some(cx.new(|cx| {
                                            editor_sources::ContentText::new(
                                                host,
                                                source.clone(),
                                                theme,
                                                cx,
                                            )
                                        }));
                                        cx.notify();
                                    }
                                },
                            )),
                        )
                        .into_any_element()
                    }
                }
            }
            Entry::Terminal(index) => {
                let terminal = &self.summary.terminals[*index];
                let id = terminal.id.clone();
                let owner = cx.entity().downgrade();
                row.id(SharedString::from(format!("inspector-terminal-{id}")))
                    .focusable()
                    .tab_index(0)
                    .role(gpui::Role::Button)
                    .aria_label(match &terminal.command {
                        artisan_api::TextContent::Inline(text) => text
                            .lines()
                            .find(|line| !line.trim().is_empty())
                            .unwrap_or("Shell")
                            .to_owned(),
                        artisan_api::TextContent::Stored(_) => "Stored shell command".into(),
                    })
                    .cursor_pointer()
                    .hover(move |style| {
                        editor_ui::hover_card::hover_fill(
                            style,
                            &theme,
                            editor_ui::hover_card::HoverFill::Plain,
                        )
                    })
                    .on_click(move |_, window, cx| {
                        owner
                            .update(cx, |this, cx| this.open_terminal(&id, window, cx))
                            .ok();
                    })
                    .child(inspector_glyph(
                        &theme,
                        if terminal.watch {
                            AssetId::TABLER_EYE
                        } else {
                            AssetId::TABLER_TERMINAL_2
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(theme.typography.mono.family)
                            .text_size(theme.typography.label_text)
                            .child(match &terminal.command {
                                artisan_api::TextContent::Inline(text) => text
                                    .lines()
                                    .find(|line| !line.trim().is_empty())
                                    .unwrap_or("Shell")
                                    .to_owned(),
                                artisan_api::TextContent::Stored(_) => {
                                    "Stored shell command".into()
                                }
                            }),
                    )
                    .into_any_element()
            }
            Entry::Workflow(index) => {
                let workflow = &self.summary.workflows[*index];
                row.child(inspector_glyph(&theme, AssetId::TABLER_SITEMAP))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(workflow.name.clone().unwrap_or_else(|| "Workflow".into())),
                    )
                    .into_any_element()
            }
        })();
        div()
            .w_full()
            .px(px(10.0))
            .map(|entry| match rule {
                // Above a section heading.
                Some(true) => entry.mt(px(10.0)).border_t_1().border_color(line),
                // Below the project facts.
                Some(false) => entry.pb(px(10.0)).border_b_1().border_color(line),
                None => entry,
            })
            .child(content)
            .into_any_element()
    }
}
/// What an agent's hover card says, resolved when its row renders.
#[derive(Clone)]
struct AgentCard {
    engine: Option<artisan_api::EngineId>,
    model: Option<String>,
    /// Effort, speed, context window and permissions, each with its icon.
    settings: Vec<(AssetId, String)>,
    purpose: String,
}
impl Inspector {
    fn agent_card(&self, agent: &artisan_api::InspectorAgent, cx: &gpui::App) -> AgentCard {
        let config = agent.config.as_ref();
        let engine = config.map(|config| config.engine).or(agent.engine);
        let id = config
            .and_then(|config| config.model.clone())
            .or_else(|| agent.model.clone());
        let model = self.host.as_ref().and_then(|(host, _)| {
            let status = host.read(cx).engines.get(&engine?)?;
            let id = id.as_ref()?;
            status.models.iter().find(|model| &model.id == id).cloned()
        });
        let mut settings = Vec::new();
        if let Some(effort) = config.and_then(|config| config.effort.clone()).or_else(|| {
            model
                .as_ref()
                .and_then(|model| model.default_effort.clone())
        }) {
            settings.push((AssetId::TABLER_BRAIN, editor_ui::effort_label(&effort)));
        }
        if let Some(config) = config {
            let speeds = model
                .as_ref()
                .map(|model| model.speeds.as_slice())
                .unwrap_or_default();
            let speed = speeds
                .iter()
                .find(|speed| {
                    config
                        .speed
                        .as_ref()
                        .map_or(speed.default, |id| &speed.id == id)
                })
                .map_or_else(|| "Standard".to_owned(), |speed| speed.label.clone());
            settings.push((AssetId::TABLER_BOLT_FILLED, speed));
        }
        if let Some(tokens) = config
            .and_then(artisan_api::EngineConfig::context_window_override)
            .or_else(|| model.as_ref().and_then(|model| model.context_window))
        {
            settings.push((
                AssetId::TABLER_ARROWS_HORIZONTAL,
                editor_ui::context_label(tokens),
            ));
        }
        if let Some(config) = config {
            let permissions = match config.permissions {
                artisan_api::Permissions::Auto => "Auto",
                artisan_api::Permissions::Full => "Full access",
            };
            settings.push((AssetId::TABLER_LOCK, permissions.to_owned()));
        }
        AgentCard {
            engine,
            model: model.map(|model| model.name).or(id),
            settings,
            purpose: match &agent.description {
                artisan_api::TextContent::Inline(text) => text.trim().to_owned(),
                artisan_api::TextContent::Stored(_) => "Open the agent to read its task.".into(),
            },
        }
    }
}
impl AgentCard {
    fn render(&self, theme: &ArtisanTheme) -> impl IntoElement {
        // Spaced like the context meter's card.
        let title = div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(8.0))
            .text_color(theme.colors.text.to_paint())
            .child(self.engine.map_or_else(
                || inspector_glyph(theme, AssetId::TABLER_BOT_ID).into_any_element(),
                |engine| {
                    editor_ui::icon::icon(editor_ui::icon::IconStyle::resolve(
                        *theme,
                        editor_ui::engine_icon(theme, engine),
                        editor_ui::icon::IconSize::Default,
                        editor_ui::icon::IconTint::Inherit,
                    ))
                    .size(px(16.0))
                    .flex_shrink_0()
                    .into_any_element()
                },
            ))
            .child(
                editor_ui::hover_card::hover_card_title(
                    theme,
                    self.model
                        .clone()
                        .or_else(|| self.engine.map(|engine| engine.display_name().to_owned()))
                        .unwrap_or_else(|| "Agent".to_owned()),
                )
                .truncate(),
            );
        let settings = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children(self.settings.iter().map(|(icon, value)| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(theme.typography.label_text)
                    .line_height(px(editor_ui::hover_card::HOVER_CARD_LINE_HEIGHT_PX))
                    .text_color(theme.colors.text.to_paint())
                    .child(inspector_glyph(theme, *icon))
                    .child(value.clone())
            }));
        div()
            .max_w(px(320.0))
            .child(editor_ui::hover_card::glass_hover_card(
                theme,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .p(px(16.0))
                    .child(title)
                    .when(!self.settings.is_empty(), |card| card.child(settings))
                    .when(!self.purpose.is_empty(), |card| {
                        card.child(editor_ui::hover_card::hover_card_text(
                            theme,
                            self.purpose.clone(),
                            &[],
                        ))
                    }),
            ))
    }
}
fn inspector_glyph(theme: &ArtisanTheme, id: AssetId) -> impl IntoElement {
    editor_ui::icon::icon(editor_ui::icon::IconStyle::resolve(
        *theme,
        id,
        editor_ui::icon::IconSize::Default,
        editor_ui::icon::IconTint::Muted,
    ))
    .flex_shrink_0()
}
impl Render for Inspector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        if let Some(plan) = &self.plan {
            let height = window.viewport_size().height * 0.4;
            plan.update(cx, |plan, cx| plan.set_max_list_height(height, cx));
        }
        let mut root = div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .py(px(10.0))
            .text_color(theme.colors.text.to_paint());
        if let Some(error) = &self.error {
            let owner = cx.entity().downgrade();
            root = root.child(div().px(px(10.0)).child(error.clone())).child(
                Button::text("inspector-retry", theme, "Retry").on_activate(move |_, _, cx| {
                    owner.update(cx, |this, cx| this.retry_summary(cx)).ok();
                }),
            );
        }
        let list = list(
            self.list.clone(),
            cx.processor(move |this, index, _, cx| this.render_entry(index, &theme, cx)),
        )
        .flex_1()
        .min_h_0();
        root.child(list)
            .children(
                self.source
                    .iter()
                    .map(|source| div().px(px(10.0)).child(source.clone())),
            )
            .children(self.plan.iter().map(|plan| {
                div()
                    .flex_none()
                    .px(px(10.0))
                    .debug_selector(|| "inspector-plan-panel".to_owned())
                    .child(plan.clone())
            }))
    }
}

/// Streams append within one epoch. A reset/reconnect always parses afresh.
pub(crate) struct ShellScreen {
    fed: usize,
    epoch: Option<u64>,
    screen: TerminalScreen,
}
impl ShellScreen {
    fn new() -> Self {
        Self {
            fed: 0,
            epoch: None,
            screen: TerminalScreen::new(),
        }
    }
    fn sync_rope(&mut self, text: &editor_core::Rope, epoch: u64) {
        if self.epoch != Some(epoch) || text.len_bytes() < self.fed {
            self.screen = TerminalScreen::new();
            self.fed = 0;
            self.epoch = Some(epoch);
        }
        if text.len_bytes() > self.fed {
            for chunk in text.byte_slice(self.fed..).chunks() {
                self.screen.feed(chunk.as_bytes());
            }
            self.fed = text.len_bytes();
        }
    }
    fn sync(&mut self, text: &str, epoch: Option<u64>) {
        if self.epoch != epoch || text.len() < self.fed || epoch.is_none() {
            self.screen = TerminalScreen::new();
            self.fed = 0;
            self.epoch = epoch;
        }
        if text.len() > self.fed {
            self.screen.feed(&text.as_bytes()[self.fed..]);
            self.fed = text.len();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::ShellScreen;
    #[test]
    fn rope_appends_feed_only_new_ansi_bytes_and_epoch_resets() {
        let mut shell = ShellScreen::new();
        let mut text = editor_core::Rope::from_str("\x1b[3");
        shell.sync_rope(&text, 1);
        text.insert(text.len_chars(), "1mred🙂\x1b[0m\nnext");
        shell.sync_rope(&text, 1);
        assert_eq!(shell.screen.plain_text(), "red🙂\nnext");
        let fed = shell.fed;
        shell.sync_rope(&text, 1);
        assert_eq!(shell.fed, fed);
        shell.sync_rope(&editor_core::Rope::from_str("new"), 2);
        assert_eq!(shell.screen.plain_text(), "new");
    }
    #[test]
    fn append_epochs_and_authoritative_completion_do_not_keep_a_missing_tail() {
        let mut shell = ShellScreen::new();
        shell.sync("\x1b[3", Some(1));
        shell.sync("\x1b[31mred\x1b[0m\nnext", Some(1));
        assert_eq!(shell.screen.plain_text(), "red\nnext");
        shell.sync("new stream", Some(2));
        assert_eq!(shell.screen.plain_text(), "new stream");
        shell.sync("all done\n", None);
        assert_eq!(shell.screen.plain_text(), "all done");
    }
}
