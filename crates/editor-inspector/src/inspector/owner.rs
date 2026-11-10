use super::{Inspector, InspectorAction, TerminalDialog};
use editor_conversation::controller::Conversation;
use editor_core::{Core, CoreEvent};
use editor_sources::SourceView;
use aqua::UiApp;
use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

#[derive(Clone, Copy)]
pub struct SourceIds {
    pub source_document: SelectionDocumentId,
    pub command_document: SelectionDocumentId,
    pub output_document: SelectionDocumentId,
    pub detail_document: SelectionDocumentId,
    pub source: SelectionSourceId,
    pub command: SelectionSourceId,
    pub output: SelectionSourceId,
    pub detail: SelectionSourceId,
}

pub struct InspectorOwner {
    pub inspector: Inspector,
    pub terminal: Rc<RefCell<TerminalDialog>>,
    pub source: SourceView,
    pub command: SourceView,
    pub output: SourceView,
    pub detail: SourceView,
    ids: SourceIds,
    terminal_hovered: bool,
    copied_until: Option<aqua::Instant>,
    reveal_pending: bool,
    scope: u64,
    pulse_starts: Rc<RefCell<HashMap<usize, u32>>>,
}

impl InspectorOwner {
    pub fn new(core: &Core, ids: SourceIds) -> Self {
        Self {
            inspector: Inspector::default(),
            terminal: Rc::new(RefCell::new(TerminalDialog::new(core))),
            source: SourceView::new(ids.source_document, ids.source),
            command: SourceView::new(ids.command_document, ids.command),
            output: SourceView::new(ids.output_document, ids.output),
            detail: SourceView::new(ids.detail_document, ids.detail),
            ids,
            terminal_hovered: false,
            copied_until: None,
            reveal_pending: false,
            scope: 0,
            pulse_starts: Rc::default(),
        }
    }

    pub fn release(&self, ui: &mut UiApp) {
        self.source.release(ui);
        self.command.release(ui);
        self.output.release(ui);
        self.detail.release(ui);
    }

    pub fn follow(&mut self, core: &Core, conversation: &Conversation, ui: &mut UiApp) -> bool {
        if !self.inspector.follow(core, conversation) {
            return false;
        }
        self.scope = self.scope.wrapping_add(1);
        self.pulse_starts.borrow_mut().clear();
        self.release(ui);
        self.source = SourceView::new(self.ids.source_document, self.ids.source);
        self.command = SourceView::new(self.ids.command_document, self.ids.command);
        self.output = SourceView::new(self.ids.output_document, self.ids.output);
        self.detail = SourceView::new(self.ids.detail_document, self.ids.detail);
        self.terminal
            .borrow_mut()
            .follow(conversation.open.as_ref());
        true
    }

    pub fn dispatch(
        &mut self,
        core: &mut Core,
        conversation: &mut Conversation,
        ui: &mut UiApp,
        action: InspectorAction,
    ) {
        match action {
            InspectorAction::Retry => self.inspector.retry_summary(core),
            InspectorAction::OpenAgent { id, thread } => {
                self.inspector
                    .open_agent_target(core, conversation, id, thread)
            }
            InspectorAction::OpenPlan => {
                let before = self
                    .inspector
                    .plan
                    .as_ref()
                    .map(|plan| plan.borrow().identity());
                self.inspector.open_plan(core);
                if self
                    .inspector
                    .plan
                    .as_ref()
                    .map(|plan| plan.borrow().identity())
                    != before
                {
                    self.detail.release(ui);
                    self.detail = SourceView::new(self.ids.detail_document, self.ids.detail);
                }
            }
            InspectorAction::OpenStep(source) => {
                let before = self
                    .inspector
                    .source
                    .as_ref()
                    .map(editor_sources::ContentText::identity);
                self.inspector.open_source(core, source);
                if self
                    .inspector
                    .source
                    .as_ref()
                    .map(editor_sources::ContentText::identity)
                    != before
                {
                    self.source.release(ui);
                    self.source = SourceView::new(self.ids.source_document, self.ids.source);
                }
            }
            InspectorAction::OpenTerminal(item) => {
                if self
                    .terminal
                    .borrow_mut()
                    .open(core, conversation.open.as_ref(), &item)
                {
                    self.command.release(ui);
                    self.output.release(ui);
                    self.command = SourceView::new(self.ids.command_document, self.ids.command);
                    self.output = SourceView::new(self.ids.output_document, self.ids.output);
                    self.reveal_pending = true;
                    self.copied_until = None;
                }
            }
        }
    }

    #[must_use]
    pub const fn scope(&self) -> u64 {
        self.scope
    }

    pub fn close_terminal(&mut self, ui: &mut UiApp) {
        self.terminal.borrow_mut().close();
        self.reveal_pending = false;
        self.copied_until = None;
        self.command.release(ui);
        self.output.release(ui);
        self.command = SourceView::new(self.ids.command_document, self.ids.command);
        self.output = SourceView::new(self.ids.output_document, self.ids.output);
    }

    pub fn on_core_event(
        &mut self,
        core: &Core,
        conversation: &Conversation,
        ui: &mut UiApp,
        event: &CoreEvent,
    ) -> bool {
        let followed = self.follow(core, conversation, ui);
        let inspector = self.inspector.on_core_event(core, conversation, event);
        let terminal = self.terminal.borrow_mut().on_core_event(core, event);
        if terminal && self.terminal.borrow().following {
            self.reveal_pending = true;
        }
        if terminal && self.terminal.borrow().job.is_none() {
            self.command.release(ui);
            self.output.release(ui);
            self.command = SourceView::new(self.ids.command_document, self.ids.command);
            self.output = SourceView::new(self.ids.output_document, self.ids.output);
        }
        followed || inspector || terminal
    }
}

pub enum SourceTarget {
    Inspector,
    Command,
    Output,
    Detail,
}

pub enum OwnerAction {
    Inspector {
        scope: u64,
        action: super::InspectorAction,
    },
    Terminal {
        generation: u64,
        action: super::TerminalAction,
    },
    Source {
        target: SourceTarget,
        identity: u64,
        action: editor_sources::ContentAction,
    },
    Plan {
        identity: u64,
        action: editor_sources::CollectionAction<artisan_api::PlanStep>,
    },
}

pub struct InspectorElements {
    pub panel: aqua_ui::element::Element,
    pub terminal: Option<aqua_ui::element::Element>,
}

fn present_step(step: &artisan_api::PlanStep, index: u64) -> editor_sources::CollectionPreview {
    editor_sources::CollectionPreview {
        label: match &step.text {
            artisan_api::TextContent::Inline(text) => format!(
                "{}. {}",
                index + 1,
                text.chars().take(160).collect::<String>()
            ),
            artisan_api::TextContent::Stored(_) => format!("Step {}", index + 1),
        },
        detail: Some(step.text.clone()),
        selected: false,
        disabled: false,
    }
}

impl InspectorOwner {
    pub fn build(
        &mut self,
        core: &Core,
        ui: &mut UiApp,
        theme: editor_ui::theme::ArtisanTheme,
        viewport_height: f32,
        reduced_motion: bool,
        now: aqua::Instant,
        action: Rc<dyn Fn(OwnerAction)>,
    ) -> anyhow::Result<InspectorElements> {
        let source = match &self.inspector.source {
            Some(content) => {
                let callback = Rc::clone(&action);
                let identity = content.identity();
                Some(self.source.render(
                    ui,
                    content,
                    theme,
                    Rc::new(move |event| {
                        callback(OwnerAction::Source {
                            target: SourceTarget::Inspector,
                            identity,
                            action: event,
                        })
                    }),
                )?)
            }
            None => {
                self.source.release(ui);
                None
            }
        };
        let plan = if let Some(plan) = &self.inspector.plan {
            let selected = plan.borrow().selected.is_some();
            let plan_callback = Rc::clone(&action);
            let identity = plan.borrow().identity();
            let callback = Rc::new(move |event| {
                plan_callback(OwnerAction::Plan {
                    identity,
                    action: event,
                })
            });
            if selected {
                let browser = plan.borrow();
                let stored = match &browser.detail {
                    Some(content) => {
                        let callback = Rc::clone(&action);
                        let identity = content.identity();
                        Some(self.detail.render(
                            ui,
                            content,
                            theme,
                            Rc::new(move |event| {
                                callback(OwnerAction::Source {
                                    target: SourceTarget::Detail,
                                    identity,
                                    action: event,
                                })
                            }),
                        )?)
                    }
                    None => {
                        self.detail.release(ui);
                        None
                    }
                };
                let detail = editor_sources::collection_detail(
                    &browser,
                    theme,
                    Some(viewport_height * 0.4),
                    self.ids.detail_document,
                    self.ids.detail,
                    stored,
                    callback,
                )
                .map_err(anyhow::Error::msg)?;
                if let Some(selection) = detail.selection {
                    ui.register_selection_source_in(self.ids.detail_document, selection)?;
                }
                Some(detail.element)
            } else {
                self.detail.release(ui);
                Some(editor_sources::collection_list(
                    Rc::clone(plan),
                    "Checklist",
                    theme,
                    Some(viewport_height * 0.4),
                    Rc::new(present_step),
                    false,
                    callback,
                ))
            }
        } else {
            self.detail.release(ui);
            None
        };
        let inspector_callback = Rc::clone(&action);
        let scope = self.scope;
        self.pulse_starts.borrow_mut().retain(|index, _| {
            !reduced_motion
                && self
                    .inspector
                    .summary
                    .agents
                    .get(*index)
                    .is_some_and(|agent| agent.status == artisan_api::Status::Running)
        });
        let panel = super::inspector_view(
            core,
            &self.inspector,
            theme,
            source,
            plan,
            reduced_motion,
            Rc::clone(&self.pulse_starts),
            Rc::new(move |event| {
                inspector_callback(OwnerAction::Inspector {
                    scope,
                    action: event,
                })
            }),
        )
        .map_err(anyhow::Error::msg)?;
        let (opened, command, output) = {
            let terminal = self.terminal.borrow();
            match &terminal.job {
                Some(job) => {
                    let command = match &job.command_source {
                        Some(content) => {
                            let callback = Rc::clone(&action);
                            let identity = content.identity();
                            Some(self.command.render(
                                ui,
                                content,
                                theme,
                                Rc::new(move |event| {
                                    callback(OwnerAction::Source {
                                        target: SourceTarget::Command,
                                        identity,
                                        action: event,
                                    })
                                }),
                            )?)
                        }
                        None => {
                            self.command.release(ui);
                            None
                        }
                    };
                    let output = match &job.output_source {
                        Some(content) => {
                            let callback = Rc::clone(&action);
                            let identity = content.identity();
                            Some(self.output.render(
                                ui,
                                content,
                                theme,
                                Rc::new(move |event| {
                                    callback(OwnerAction::Source {
                                        target: SourceTarget::Output,
                                        identity,
                                        action: event,
                                    })
                                }),
                            )?)
                        }
                        None => {
                            self.output.release(ui);
                            None
                        }
                    };
                    (true, command, output)
                }
                None => {
                    self.command.release(ui);
                    self.output.release(ui);
                    (false, None, None)
                }
            }
        };
        let terminal = if opened {
            let callback = Rc::clone(&action);
            let generation = self.terminal.borrow().generation();
            Some(
                super::terminal_view(
                    Rc::clone(&self.terminal),
                    theme,
                    viewport_height,
                    self.terminal_hovered,
                    self.copied_until.is_some_and(|deadline| now < deadline),
                    command,
                    output,
                    Rc::new(move |event| {
                        callback(OwnerAction::Terminal {
                            generation,
                            action: event,
                        })
                    }),
                )
                .map_err(anyhow::Error::msg)?,
            )
        } else {
            None
        };
        Ok(InspectorElements { panel, terminal })
    }
}

impl InspectorOwner {
    pub fn bind(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        if let Some(content) = &self.inspector.source {
            self.source.bind(content, ctx, window);
        }
        if let Some(plan) = &self.inspector.plan {
            let plan = plan.borrow();
            ctx.bind_mailbox(window, plan.window().wake_mailbox());
            if let Some(content) = &plan.detail {
                self.detail.bind(content, ctx, window);
            }
        }
        if let Some(job) = &self.terminal.borrow().job {
            if let Some(content) = &job.command_source {
                self.command.bind(content, ctx, window);
            }
            if let Some(content) = &job.output_source {
                self.output.bind(content, ctx, window);
            }
        }
    }

    pub fn drain(
        &mut self,
        core: &Core,
        ui: &mut UiApp,
        ctx: &mut aqua::AppCtx<'_>,
    ) -> bool {
        let before = self.view_state();
        if let Some(content) = &mut self.inspector.source {
            self.source.drain(core, content, ctx);
        }
        if let Some(plan) = &self.inspector.plan {
            let mut plan = plan.borrow_mut();
            plan.drain(core);
            if let Some(content) = &mut plan.detail {
                self.detail.drain(core, content, ctx);
            }
        }
        let terminal_changed = self.terminal.borrow_mut().drain(core);
        if terminal_changed && self.terminal.borrow().following {
            self.reveal_pending = true;
        }
        if let Some(job) = &mut self.terminal.borrow_mut().job {
            if let Some(content) = &mut job.command_source {
                self.command.drain(core, content, ctx);
            }
            if let Some(content) = &mut job.output_source {
                self.output.drain(core, content, ctx);
            }
        } else if terminal_changed {
            self.command.release(ui);
            self.output.release(ui);
            self.command = SourceView::new(self.ids.command_document, self.ids.command);
            self.output = SourceView::new(self.ids.output_document, self.ids.output);
        }
        terminal_changed || self.view_state() != before
    }

    pub fn dispatch_action(
        &mut self,
        core: &mut Core,
        conversation: &mut Conversation,
        ui: &mut UiApp,
        ctx: &mut aqua::AppCtx<'_>,
        routes: &editor_ui::dialogs::DialogRoutes,
        window: aqua::prelude::AquaWindowId,
        action: OwnerAction,
    ) -> bool {
        let before = self.view_state();
        let screen = core.screen().clone();
        let agent = conversation.agent.clone();
        let summary_load = self
            .inspector
            .open
            .as_ref()
            .and_then(|(host, thread)| core.host(host)?.thread_summary(thread))
            .map(|state| (state.loading, state.error.clone()));
        match action {
            OwnerAction::Inspector { scope, action } => {
                if scope == self.scope && self.inspector.is_current(core) {
                    self.dispatch(core, conversation, ui, action);
                }
            }
            OwnerAction::Plan { identity, action } => {
                if let Some(plan) = &self.inspector.plan
                    && plan.borrow().identity() == identity
                {
                    let mut plan = plan.borrow_mut();
                    let selected = plan
                        .selected
                        .as_ref()
                        .map(|(index, page)| (*index, std::sync::Arc::as_ptr(page) as usize));
                    let detail = plan
                        .detail
                        .as_ref()
                        .map(editor_sources::ContentText::identity);
                    editor_sources::dispatch_collection(
                        &mut plan,
                        core,
                        action,
                        present_step,
                        None,
                    );
                    let next_selected = plan
                        .selected
                        .as_ref()
                        .map(|(index, page)| (*index, std::sync::Arc::as_ptr(page) as usize));
                    let next_detail = plan
                        .detail
                        .as_ref()
                        .map(editor_sources::ContentText::identity);
                    if selected != next_selected || detail != next_detail {
                        self.detail.release(ui);
                        self.detail = SourceView::new(self.ids.detail_document, self.ids.detail);
                    }
                }
            }
            OwnerAction::Source {
                target,
                identity,
                action,
            } => match target {
                SourceTarget::Inspector => {
                    if let Some(content) = &mut self.inspector.source
                        && content.identity() == identity
                    {
                        self.source.action(core, content, ctx, routes, window, action);
                    }
                }
                SourceTarget::Detail => {
                    if let Some(plan) = &self.inspector.plan {
                        if let Some(content) = &mut plan.borrow_mut().detail
                            && content.identity() == identity
                        {
                            self.detail.action(core, content, ctx, routes, window, action);
                        }
                    }
                }
                SourceTarget::Command => {
                    if let Some(job) = &mut self.terminal.borrow_mut().job {
                        if let Some(content) = &mut job.command_source
                            && content.identity() == identity
                        {
                            self.command.action(core, content, ctx, routes, window, action);
                        }
                    }
                }
                SourceTarget::Output => {
                    if let Some(job) = &mut self.terminal.borrow_mut().job {
                        if let Some(content) = &mut job.output_source
                            && content.identity() == identity
                        {
                            self.output.action(core, content, ctx, routes, window, action);
                        }
                    }
                }
            },
            OwnerAction::Terminal { generation, action }
                if self.terminal.borrow().generation() == generation
                    && self.terminal.borrow().job.is_some() =>
            {
                match action {
                    super::TerminalAction::Retry => self.terminal.borrow_mut().fetch(core),
                    super::TerminalAction::Copy => {
                        let terminal = self.terminal.borrow();
                        if let Some(job) = &terminal.job
                            && core.host_if_instance(&job.host, job.instance).is_some()
                            && job.output_source.is_none()
                        {
                            if let Err(error) = ctx.clipboard().set_text(job.copy_output()) {
                                tracing::warn!(%error, "could not copy shell output");
                            }
                            let deadline = ctx.now() + std::time::Duration::from_millis(1750);
                            self.copied_until = Some(deadline);
                            ctx.request_animation_deadline(window, deadline);
                        }
                    }
                    super::TerminalAction::Hover(hovered) => self.terminal_hovered = hovered,
                    super::TerminalAction::JumpToLatest => {
                        self.terminal.borrow_mut().following = true;
                        self.reveal_pending = true;
                    }
                    super::TerminalAction::Viewport(viewport) => {
                        if !self.reveal_pending {
                            self.terminal.borrow_mut().following =
                                viewport.distance_from_end <= 0.0;
                        }
                    }
                }
            }
            OwnerAction::Terminal { .. } => {}
        }
        let next_summary_load = self
            .inspector
            .open
            .as_ref()
            .and_then(|(host, thread)| core.host(host)?.thread_summary(thread))
            .map(|state| (state.loading, state.error.clone()));
        self.view_state() != before
            || core.screen() != &screen
            || conversation.agent != agent
            || summary_load != next_summary_load
    }

    pub fn tick(&mut self, now: aqua::Instant) -> bool {
        if self.copied_until.is_some_and(|deadline| now >= deadline) {
            self.copied_until = None;
            true
        } else {
            false
        }
    }

    pub fn reveal_tail(&mut self, ui: &mut UiApp) -> bool {
        if !self.reveal_pending {
            return false;
        }
        let count = self
            .terminal
            .borrow()
            .job
            .as_ref()
            .map_or(0, |job| super::Layout::of(job).total());
        if count == 0 {
            self.reveal_pending = false;
            return false;
        }
        if ui.reveal_collection_item("inspector-terminal-lines", count - 1) {
            self.reveal_pending = false;
            true
        } else {
            false
        }
    }
}

#[derive(PartialEq, Eq)]
struct ViewState {
    open: Option<(editor_core::HostId, artisan_api::ThreadId)>,
    summary: usize,
    error: Option<String>,
    source: Option<editor_sources::ContentViewState>,
    plan_identity: Option<u64>,
    plan: Option<editor_sources::CollectionViewState>,
    terminal_item: Option<(editor_core::HostId, artisan_api::ItemId, Option<usize>)>,
    terminal_generation: u64,
    terminal_following: bool,
    terminal_loading: bool,
    terminal_error: Option<String>,
    command: Option<editor_sources::ContentViewState>,
    output: Option<editor_sources::ContentViewState>,
    hovered: bool,
    copied_until: Option<aqua::Instant>,
}

impl InspectorOwner {
    fn view_state(&self) -> ViewState {
        let terminal = self.terminal.borrow();
        ViewState {
            open: self.inspector.open.clone(),
            summary: std::sync::Arc::as_ptr(&self.inspector.summary) as usize,
            error: self.inspector.error.clone(),
            source: self
                .inspector
                .source
                .as_ref()
                .map(editor_sources::ContentText::view_state),
            plan_identity: self
                .inspector
                .plan
                .as_ref()
                .map(|plan| plan.borrow().identity()),
            plan: self
                .inspector
                .plan
                .as_ref()
                .map(|plan| plan.borrow().view_state()),
            terminal_item: terminal.job.as_ref().map(|job| {
                (
                    job.host.clone(),
                    job.item.clone(),
                    job.data
                        .as_ref()
                        .map(|data| std::sync::Arc::as_ptr(data) as usize),
                )
            }),
            terminal_generation: terminal.generation(),
            terminal_following: terminal.following,
            terminal_loading: terminal.loading,
            terminal_error: terminal.error.clone(),
            command: terminal
                .job
                .as_ref()
                .and_then(|job| job.command_source.as_ref())
                .map(editor_sources::ContentText::view_state),
            output: terminal
                .job
                .as_ref()
                .and_then(|job| job.output_source.as_ref())
                .map(editor_sources::ContentText::view_state),
            hovered: self.terminal_hovered,
            copied_until: self.copied_until,
        }
    }
}
