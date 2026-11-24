use std::{cell::RefCell, collections::HashMap, num::NonZeroUsize, rc::Rc};

use artisan_api::{EngineCommand, EngineId, GlobalPrompt};
use serde_json::Value;

use editor_core::delivery::OwnedDelivery;
use editor_core::{
    Core, HostId, HostInstance, Problem, Screen, SettingsPage, ThreadPageDirection, runtime,
};
use editor_ui::text::editor::{Editor, Mode};

use super::archive::{ArchiveForm, ArchiveRequest};
use super::engines::{EngineRequest, EnginesForm};
use super::extensions::{ExtensionsForm, MutationRequest};
use super::preferences::{
    SaveSettingError, SettingFailure, SettingKey, SettingRequest, SettingWrites,
};
use super::prompt::{PromptForm, PromptRequest};

struct HostCompletion {
    host: HostId,
    instance: HostInstance,
    result: Completion,
}

enum Completion {
    Setting(
        SettingRequest,
        Result<editor_core::client::SettingAck, SaveSettingError>,
    ),
    Archive(ArchiveRequest, Result<(), Problem>),
    Mcp(Box<MutationRequest>, Result<(), Problem>),
    Engine(EngineRequest, Result<(), Problem>),
    Prompt(PromptRequest, Result<GlobalPrompt, Problem>),
    McpSignOut(String, Result<(), Problem>),
    Skill {
        skill: String,
        mark: editor_core::CatalogMark,
        result: Result<Vec<artisan_api::CatalogSkill>, Problem>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ClaudeAiSignInError {
    #[error("sign in through the browser")]
    Browser(#[source] claude_ai_browser::SignInError),
    #[error("hand the claude.ai session to {host}")]
    Forward {
        host: HostId,
        #[source]
        source: editor_core::client::CallError,
    },
    #[error("let {host} reuse the claude.ai browser profile")]
    Authorize {
        host: HostId,
        #[source]
        source: claude_ai_browser::AuthorizationError,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gear {
    Computer(String),
    Mcp(artisan_api::McpServer),
    Skill(String),
    Account(EngineId),
    AddAccount(EngineId),
}

impl Gear {
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Computer(id) => format!("settings-gear-computer-{id}"),
            Self::Mcp(server) => format!("settings-gear-mcp-{}", server.name),
            Self::Skill(id) => format!("settings-gear-skill-{id}"),
            Self::Account(engine) => format!("settings-gear-account-{engine}"),
            Self::AddAccount(engine) => format!("settings-add-account-{engine}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Menu {
    Version,
    Font,
    Streaming,
    Retention,
    TextSize,
    Contrast,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OpenMenus(std::collections::BTreeSet<Menu>);

impl OpenMenus {
    pub(crate) fn report(&mut self, menu: Menu, open: bool) {
        if open {
            self.0.insert(menu);
        } else {
            self.0.remove(&menu);
        }
    }
}

pub struct SettingsController {
    pub(crate) reported_prompt_dirty: bool,
    pub(crate) reported_menus: OpenMenus,
    pub mcp_editors: Vec<Rc<RefCell<Editor>>>,
    pub retention_select: Rc<RefCell<editor_ui::select::SelectState<Option<u32>>>>,
    pub font_select: Rc<RefCell<editor_ui::select::SelectState<&'static str>>>,
    pub streaming_select: Rc<RefCell<editor_ui::select::SelectState<&'static str>>>,
    pub text_size_select: Rc<RefCell<editor_ui::select::SelectState<Option<u32>>>>,
    pub contrast_select: Rc<RefCell<editor_ui::select::SelectState<Option<bool>>>>,
    pub version_select: Rc<RefCell<editor_ui::select::SelectState<String>>>,
    pub gear: Option<Gear>,
    pub sign_in: super::sign_in::SignInForm,
    pub(crate) forge_code: Rc<RefCell<Editor>>,
    pub(crate) archive: ArchiveForm,
    pub engines: EnginesForm,
    pub extensions: ExtensionsForm,
    pub prompt: PromptForm,
    pub prompt_editor: Rc<RefCell<Editor>>,
    pub(crate) search_editor: Rc<RefCell<Editor>>,
    pub scroll_edges: editor_ui::scroll_edge::ScrollEdges,
    pub(crate) open_licence: Option<usize>,
    pub(crate) diagnostics_result: Option<Result<(), Problem>>,
    pub(crate) skill_pending: std::collections::HashSet<String>,
    pub(crate) skill_error: Option<Problem>,
    pub(crate) editor_setting_error: Option<Problem>,
    parked_prompts: HashMap<String, (PromptForm, String)>,
    setting_writes: SettingWrites,
    completion: OwnedDelivery<HostCompletion>,
    tasks: Vec<tokio_util::task::AbortOnDropHandle<()>>,
    active_instance: Option<HostInstance>,
    pub display_rate: super::display::DisplayRate,
}

impl Default for SettingsController {
    fn default() -> Self {
        Self {
            reported_prompt_dirty: false,
            reported_menus: OpenMenus::default(),
            mcp_editors: (0..6)
                .map(|_| Rc::new(RefCell::new(Editor::new("", Mode::SingleLine))))
                .collect(),
            retention_select: Rc::new(RefCell::new(editor_ui::select::SelectState::new(
                std::time::Instant::now(),
            ))),
            font_select: Rc::new(RefCell::new(editor_ui::select::SelectState::new(
                std::time::Instant::now(),
            ))),
            streaming_select: Rc::new(RefCell::new(editor_ui::select::SelectState::new(
                std::time::Instant::now(),
            ))),
            text_size_select: Rc::new(RefCell::new(editor_ui::select::SelectState::new(
                std::time::Instant::now(),
            ))),
            contrast_select: Rc::new(RefCell::new(editor_ui::select::SelectState::new(
                std::time::Instant::now(),
            ))),
            version_select: Rc::new(RefCell::new(editor_ui::select::SelectState::new(
                std::time::Instant::now(),
            ))),
            gear: None,
            sign_in: super::sign_in::SignInForm::default(),
            forge_code: Rc::new(RefCell::new(Editor::new("", Mode::SingleLine))),
            archive: ArchiveForm::default(),
            engines: EnginesForm::default(),
            extensions: ExtensionsForm::default(),
            prompt: PromptForm::default(),
            prompt_editor: Rc::new(RefCell::new(Editor::new("", Mode::Source))),
            search_editor: Rc::new(RefCell::new(Editor::new("", Mode::SingleLine))),
            scroll_edges: editor_ui::scroll_edge::ScrollEdges::default(),
            open_licence: None,
            diagnostics_result: None,
            skill_pending: std::collections::HashSet::new(),
            skill_error: None,
            editor_setting_error: None,
            parked_prompts: HashMap::new(),
            setting_writes: SettingWrites::default(),
            completion: OwnedDelivery::new(
                NonZeroUsize::new(64)
                    .expect("invariant: the settings delivery capacity is non-zero"),
            ),
            tasks: Vec::new(),
            active_instance: None,
            display_rate: super::display::DisplayRate::Unknown,
        }
    }
}

impl SettingsController {
    pub fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        self.completion.mailbox()
    }

    pub fn clear_search(&self) -> bool {
        let mut search = self.search_editor.borrow_mut();
        if search.text().is_empty() {
            return false;
        }
        search.set_text("");
        true
    }

    pub fn sync_editors(&self, ui: &mut aqua::UiApp) {
        let mut editors = vec![
            ("settings-search".to_owned(), Rc::clone(&self.search_editor)),
            ("sign-in-email".to_owned(), Rc::clone(&self.sign_in.email)),
            (
                "sign-in-second-factor".to_owned(),
                Rc::clone(&self.sign_in.second_factor),
            ),
            ("forges-code".to_owned(), Rc::clone(&self.forge_code)),
            (
                "settings-global-prompt-editor".to_owned(),
                Rc::clone(&self.prompt_editor),
            ),
        ];
        editors.extend(
            self.mcp_editors
                .iter()
                .enumerate()
                .map(|(index, editor)| (format!("settings-mcp-field-{index}"), Rc::clone(editor))),
        );
        editors.extend(self.engines.login_codes.iter().map(|(engine, editor)| {
            (
                format!("settings-engine-login-code-{engine}"),
                Rc::clone(editor),
            )
        }));
        editors.extend(
            self.engines
                .api_key
                .iter()
                .map(|editor| ("settings-codex-api-key".to_owned(), Rc::clone(editor))),
        );
        for (key, editor) in editors {
            let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key));
            if let Some(node) = ui.tree().sole_node_with_key(hash) {
                editor_ui::text::mount::EditorController::from_shared(editor, |_| {})
                    .sync(ui.tree_mut(), node);
            }
        }
    }

    pub fn sync(&mut self, core: &mut Core) {
        let instance = core.active_host().map(editor_core::HostData::instance);
        if self.active_instance != instance {
            self.active_instance = instance;
            self.park_prompt();
            self.engines.sync_host(None);
            self.skill_pending.clear();
            self.skill_error = None;
            self.archive.sync(None);
            self.prompt.sync_host(None);
            self.prompt_editor.borrow_mut().set_text("");
        }
        let host = core.active_host().map(|host| host.id.clone());
        let host_name = host.as_ref().map(|host| host.0.clone());
        if self.prompt.host != host_name {
            self.park_prompt();
        }
        self.engines.sync_host(host_name.clone());
        self.sync_mcp_form(
            core.active_host()
                .map(|host| (host.id.0.clone(), host.instance())),
        );
        let page = match core.screen() {
            Screen::Settings(page) => *page,
            Screen::Home
            | Screen::Thread { .. }
            | Screen::NewThread { .. }
            | Screen::Marketplace
            | Screen::Usage => {
                self.engines.api_key = None;
                self.clear_search();
                return;
            }
        };
        if page.parent() != SettingsPage::Engine(EngineId::Codex) {
            self.engines.api_key = None;
        }
        if let SettingsPage::Engine(engine) = page.parent() {
            self.engines.prepare(engine);
        }
        if page.parent() == SettingsPage::Threads {
            self.archive
                .sync(host.as_ref().and_then(|id| core.host(id)));
            if let Some(host) = host.as_ref().and_then(|id| core.host_mut(id)) {
                let projects: std::collections::BTreeSet<_> = self
                    .archive
                    .ids
                    .iter()
                    .filter_map(|id| host.threads.get(id).map(|thread| thread.project_id.clone()))
                    .collect();
                for project in projects {
                    host.load_project_icon(&project);
                }
                let page = host.thread_page(true);
                if !page.is_loaded() && !page.is_loading() {
                    host.load_threads(true, ThreadPageDirection::Latest);
                }
            }
        }
        if matches!(
            page,
            SettingsPage::Extensions | SettingsPage::Section(editor_core::SettingsSection::Skills)
        ) && let Some(host) = host.as_ref().and_then(|id| core.host_mut(id))
        {
            host.load_skill_catalog(false);
        }
        if page != SettingsPage::GlobalPrompt {
            return;
        }
        if self.prompt.sync_host(host_name.clone()) {
            match host_name.and_then(|name| self.parked_prompts.remove(&name)) {
                Some((form, text)) => {
                    self.prompt = form;
                    self.prompt_editor.borrow_mut().set_text(&text);
                }
                None => self.prompt_editor.borrow_mut().set_text(""),
            }
        }
        if let Some(host) = host.as_ref().and_then(|id| core.host_mut(id)) {
            if let Some(prompt) = host.global_prompt.as_ref() {
                let current_text = self.prompt_editor.borrow().text().to_owned();
                if let Some(text) = self.prompt.load(prompt, &current_text, false) {
                    self.prompt_editor.borrow_mut().set_text(&text);
                }
            } else if !self.prompt.pending {
                host.load_prompt(false);
            }
        }
        self.reported_prompt_dirty = self.prompt_editor.borrow().text() != self.prompt.saved;
    }

    fn park_prompt(&mut self) {
        let Some(host) = self.prompt.host.clone() else {
            return;
        };
        self.parked_prompts.remove(&host);
        let text = self.prompt_editor.borrow().text().to_owned();
        if !self.prompt.loaded || text == self.prompt.saved {
            return;
        }
        let mut form = self.prompt.clone();
        form.park();
        self.parked_prompts.insert(host, (form, text));
    }

    pub(crate) fn set_editor_setting(&mut self, core: &mut Core, key: &'static str, value: Value) {
        match core.set_setting(key, value) {
            Ok(()) => self.editor_setting_error = None,
            Err(error) => {
                let problem = Problem::caused_by("Couldn't save this setting.", &error);
                tracing::error!(key, error = ?problem.detail(), "saving an Editor setting");
                self.editor_setting_error = Some(problem);
            }
        }
    }

    pub(crate) fn reload_editor_settings(&mut self, core: &mut Core) {
        self.editor_setting_error = None;
        core.reload_settings();
    }

    pub fn core_event(&mut self, core: &mut Core, event: &editor_core::CoreEvent) {
        if matches!(event, editor_core::CoreEvent::SettingsChanged)
            && let editor_core::SettingsSave::Failed(failure) = core.settings_save()
        {
            let problem = Problem::caused_by("Couldn't save your settings.", &**failure);
            tracing::error!(error = ?problem.detail(), "saving an Editor setting");
            self.editor_setting_error = Some(problem);
        }
        if matches!(event, editor_core::CoreEvent::ScreenChanged) {
            self.engines.errors.clear();
        }
        match event {
            editor_core::CoreEvent::Host {
                event:
                    editor_core::HostEvent::ThreadChanged { .. }
                    | editor_core::HostEvent::Reset
                    | editor_core::HostEvent::Changed,
                ..
            }
            | editor_core::CoreEvent::HostsChanged
            | editor_core::CoreEvent::ScreenChanged => self.archive.invalidate(),
            editor_core::CoreEvent::Host { .. }
            | editor_core::CoreEvent::SettingsChanged
            | editor_core::CoreEvent::AccessChanged
            | editor_core::CoreEvent::McpSignInChanged
            | editor_core::CoreEvent::OpenUrl(_)
            | editor_core::CoreEvent::Account(_) => {}
        }
        self.sync(core);
    }

    pub(crate) fn claim_forge(&self, core: &mut Core) {
        let forge_code = self.forge_code.borrow().text().to_owned();
        core.account_mut().claim_code(&forge_code);
        self.forge_code.borrow_mut().set_text("");
    }

    pub(crate) fn unarchive(&mut self, core: &mut Core, thread: artisan_api::ThreadId) {
        if core.active_host().map(editor_core::HostData::instance) != self.active_instance
            || core.active_host().map(|host| &host.id.0) != self.archive.host.as_ref()
        {
            return;
        }
        let Some(request) = self.archive.begin(thread) else {
            return;
        };
        let Some(host) = core.host_mut(&HostId(request.host.clone())) else {
            return;
        };
        let instance = host.instance();
        let id = host.id.clone();
        let thread = request.thread.clone();
        let sender = self.completion.sender();
        host.edit_thread(
            &thread,
            editor_core::ThreadEdit::Unarchive,
            Box::new(move |result| {
                Box::pin(async move {
                    sender
                        .offer(HostCompletion {
                            host: id,
                            instance,
                            result: Completion::Archive(
                                request,
                                result.map_err(|error| error.problem()),
                            ),
                        })
                        .await;
                })
            }),
        );
    }

    pub fn set_host_setting_for(
        &mut self,
        core: &mut Core,
        id: HostId,
        instance: HostInstance,
        key: &'static str,
        value: Value,
    ) {
        let Some(host) = core.host_mut_if_instance(&id, instance) else {
            return;
        };
        let client = host.client.clone();
        let target = SettingKey {
            host: id,
            instance,
            key,
        };
        if let Some(request) = self.setting_writes.begin(host, target, value) {
            self.send_setting(client, request);
        }
    }

    pub(crate) fn setting_failures(
        &self,
        host: &editor_core::HostData,
    ) -> Vec<(SettingKey, SettingFailure)> {
        self.setting_writes.failures_for(&host.id, host.instance())
    }

    pub(crate) fn retry_setting(&mut self, core: &mut Core, target: SettingKey) {
        if let Some(failure) = self.setting_writes.failure(&target).cloned() {
            self.set_host_setting_for(
                core,
                target.host,
                target.instance,
                target.key,
                failure.value,
            );
        }
    }

    pub(crate) fn dismiss_setting_failure(&mut self, target: &SettingKey) {
        self.setting_writes.dismiss(target);
    }

    fn send_setting(&mut self, client: editor_core::ForgeClient, request: SettingRequest) {
        let sender = self.completion.sender();
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(tokio_util::task::AbortOnDropHandle::new(
            runtime::runtime().spawn(async move {
                let result = client
                    .set_setting(request.target.key, &request.value)
                    .await
                    .map_err(|source| SaveSettingError::Send {
                        host: request.target.host.clone(),
                        key: request.target.key,
                        source,
                    });
                sender
                    .offer(HostCompletion {
                        host: request.target.host.clone(),
                        instance: request.target.instance,
                        result: Completion::Setting(request, result),
                    })
                    .await;
            }),
        ));
    }

    fn sync_mcp_form(&mut self, host: Option<(String, HostInstance)>) {
        self.read_mcp_fields();
        if self.extensions.sync_host(host) {
            self.show_mcp_fields();
        }
        self.lock_mcp_fields();
    }

    pub fn open_mcp(&mut self, previous: Option<artisan_api::McpServer>) {
        self.extensions.open(previous);
        self.show_mcp_fields();
        self.lock_mcp_fields();
    }

    fn show_mcp_fields(&self) {
        let fields = &self.extensions.fields;
        for (editor, text) in self.mcp_editors.iter().zip([
            &fields.name,
            &fields.command,
            &fields.args,
            &fields.url,
            &fields.env,
            &fields.headers,
        ]) {
            editor.borrow_mut().set_text(text);
        }
    }

    fn lock_mcp_fields(&self) {
        for editor in &self.mcp_editors {
            editor.borrow_mut().set_disabled(self.extensions.saving);
        }
    }

    pub fn cancel_mcp(&mut self) {
        self.extensions.cancel();
        self.lock_mcp_fields();
    }

    pub fn save_mcp(&mut self, core: &Core) {
        self.read_mcp_fields();
        if !self.active_form_host(core) {
            return;
        }
        if let Some(request) = self.extensions.save() {
            self.mutate_mcp(core, request);
        }
        self.lock_mcp_fields();
    }

    fn read_mcp_fields(&mut self) {
        if self.extensions.saving || self.extensions.editing.is_none() {
            return;
        }
        let fields = &mut self.extensions.fields;
        for (field, editor) in [
            &mut fields.name,
            &mut fields.command,
            &mut fields.args,
            &mut fields.url,
            &mut fields.env,
            &mut fields.headers,
        ]
        .into_iter()
        .zip(&self.mcp_editors)
        {
            editor.borrow().text().clone_into(field);
        }
    }

    pub(crate) fn toggle_skill(
        &mut self,
        core: &Core,
        instance: HostInstance,
        skill: String,
        enabled: bool,
    ) {
        let Some(host) = core
            .active_host()
            .filter(|host| host.instance() == instance && self.active_instance == Some(instance))
        else {
            return;
        };
        if !self.skill_pending.insert(skill.clone()) {
            return;
        }
        self.skill_error = None;
        let client = host.client.clone();
        let id = host.id.clone();
        let mark = host.catalog_mark();
        let sender = self.completion.sender();
        let action = if enabled {
            artisan_api::SkillAction::Enable
        } else {
            artisan_api::SkillAction::Disable
        };
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(tokio_util::task::AbortOnDropHandle::new(
            runtime::runtime().spawn(async move {
                let result = client.skill_action(&skill, action).await.map_err(|error| {
                    Problem::calling_forge(
                        &format!("Couldn't {} the skill.", action.as_str()),
                        &error,
                    )
                });
                sender
                    .offer(HostCompletion {
                        host: id,
                        instance,
                        result: Completion::Skill {
                            skill,
                            mark,
                            result,
                        },
                    })
                    .await;
            }),
        ));
    }

    pub(crate) fn toggle_mcp(
        &mut self,
        core: &Core,
        instance: HostInstance,
        previous: artisan_api::McpServer,
        enabled: bool,
    ) {
        if self.active_instance == Some(instance)
            && self.active_form_host(core)
            && let Some(request) = self.extensions.toggle(previous, enabled)
        {
            self.mutate_mcp(core, request);
        }
    }

    pub(crate) fn remove_mcp(&mut self, core: &Core, previous: artisan_api::McpServer) {
        if !self.active_form_host(core) {
            return;
        }
        if let Some(request) = self.extensions.remove(previous) {
            self.mutate_mcp(core, request);
        }
        self.lock_mcp_fields();
    }

    pub(crate) fn sign_in_mcp(&mut self, core: &mut Core, server: &str) {
        if !self.active_form_host(core) {
            return;
        }
        let Some(host) = core.active_host().map(|host| host.id.clone()) else {
            return;
        };
        self.extensions.error = None;
        drop(core.start_mcp_sign_in(&host, server, true));
    }

    pub(crate) fn cancel_mcp_sign_in(&self, core: &mut Core, server: &str) {
        if !self.active_form_host(core) {
            return;
        }
        if let Some(host) = core.active_host().map(|host| host.id.clone()) {
            core.cancel_mcp_sign_in(&host, server);
        }
    }

    pub(crate) fn sign_out_mcp(&mut self, core: &Core, server: String) {
        if !self.active_form_host(core) {
            return;
        }
        let Some(host) = core.active_host() else {
            return;
        };
        let id = host.id.clone();
        let instance = host.instance();
        let client = host.client.clone();
        let sender = self.completion.sender();
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(tokio_util::task::AbortOnDropHandle::new(
            runtime::runtime().spawn(async move {
                let result = client
                    .mcp_sign_out(&artisan_api::McpSignOut {
                        server: server.clone(),
                    })
                    .await
                    .map_err(|error| {
                        Problem::calling_forge(&format!("Couldn't sign out of {server}."), &error)
                    });
                sender
                    .offer(HostCompletion {
                        host: id,
                        instance,
                        result: Completion::McpSignOut(server, result),
                    })
                    .await;
            }),
        ));
    }

    pub(crate) fn sign_in_claude_ai(&mut self, core: &Core) {
        if core.active_host().map(editor_core::HostData::instance) != self.active_instance {
            return;
        }
        let Some(request) = self.engines.begin(EngineId::Claude) else {
            return;
        };
        let Some(host) = core.host(&HostId(request.host.clone())) else {
            return;
        };
        let id = host.id.clone();
        let instance = host.instance();
        let client = host.client.clone();
        let sender = self.completion.sender();
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(tokio_util::task::AbortOnDropHandle::new(
            runtime::runtime().spawn(async move {
                let result = async {
                    let session = claude_ai_browser::sign_in()
                        .await
                        .map_err(ClaudeAiSignInError::Browser)?;
                    client
                        .engine_action(
                            EngineId::Claude,
                            &EngineCommand::ClaudeAiSignIn {
                                session: session.clone(),
                            },
                        )
                        .await
                        .map_err(|source| ClaudeAiSignInError::Forward {
                            host: id.clone(),
                            source,
                        })?;
                    claude_ai_browser::authorize(&id.0, &session).map_err(|source| {
                        ClaudeAiSignInError::Authorize {
                            host: id.clone(),
                            source,
                        }
                    })
                }
                .await
                .map_err(|error| Problem::caused_by("Couldn't sign in to claude.ai.", &error));
                sender
                    .offer(HostCompletion {
                        host: id,
                        instance,
                        result: Completion::Engine(request, result),
                    })
                    .await;
            }),
        ));
    }

    fn mutate_mcp(&mut self, core: &Core, request: MutationRequest) {
        let Some(host) = core.host(&HostId(request.host.clone())) else {
            return;
        };
        let client = host.client.clone();
        let instance = host.instance();
        let id = host.id.clone();
        let sender = self.completion.sender();
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(tokio_util::task::AbortOnDropHandle::new(
            runtime::runtime().spawn(async move {
                let result = client
                    .mutate_mcp(&request.mutation)
                    .await
                    .map(|_| ())
                    .map_err(|error| {
                        Problem::calling_forge("Couldn't save the MCP servers.", &error)
                    });
                sender
                    .offer(HostCompletion {
                        host: id,
                        instance,
                        result: Completion::Mcp(Box::new(request), result),
                    })
                    .await;
            }),
        ));
    }

    pub(crate) fn engine_action(&mut self, core: &Core, engine: EngineId, action: EngineCommand) {
        if core.active_host().map(editor_core::HostData::instance) != self.active_instance
            || core.active_host().map(|host| &host.id.0) != self.engines.host.as_ref()
        {
            return;
        }
        let request = match action {
            EngineCommand::CancelLogin {} => self.engines.begin_cancel(engine),
            EngineCommand::Install {}
            | EngineCommand::Update {}
            | EngineCommand::Rollback {}
            | EngineCommand::Use { .. }
            | EngineCommand::Login {}
            | EngineCommand::LoginApiKey { .. }
            | EngineCommand::LoginCode { .. }
            | EngineCommand::Logout {}
            | EngineCommand::Refresh {}
            | EngineCommand::Versions {}
            | EngineCommand::ClaudeAiSignIn { .. }
            | EngineCommand::ClaudeAiSignOut {} => self.engines.begin(engine),
        };
        let Some(request) = request else {
            return;
        };
        let Some(host) = core.host(&HostId(request.host.clone())) else {
            return;
        };
        let client = host.client.clone();
        let instance = host.instance();
        let id = host.id.clone();
        let sender = self.completion.sender();
        let verb = engine_verb(&action);
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(tokio_util::task::AbortOnDropHandle::new(
            runtime::runtime().spawn(async move {
                let result = client
                    .engine_action(engine, &action)
                    .await
                    .map(|_| ())
                    .map_err(|error| {
                        Problem::calling_forge(
                            &format!("Couldn't {verb} {}.", engine.display_name()),
                            &error,
                        )
                    });
                sender
                    .offer(HostCompletion {
                        host: id,
                        instance,
                        result: Completion::Engine(request, result),
                    })
                    .await;
            }),
        ));
    }

    pub(crate) fn save_prompt(&mut self, core: &Core) {
        self.submit_prompt(core, None);
    }

    pub(crate) fn resolve_prompt(&mut self, core: &Core, resolution: artisan_api::ResolvePrompt) {
        self.submit_prompt(core, Some(resolution));
    }

    fn submit_prompt(&mut self, core: &Core, resolution: Option<artisan_api::ResolvePrompt>) {
        if core.active_host().map(editor_core::HostData::instance) != self.active_instance
            || core.active_host().map(|host| &host.id.0) != self.prompt.host.as_ref()
        {
            return;
        }
        let Some(request) = self.prompt.save(self.prompt_editor.borrow().text()) else {
            return;
        };
        let Some(host) = core.host(&HostId(request.host.clone())) else {
            return;
        };
        let client = host.client.clone();
        let instance = host.instance();
        let id = host.id.clone();
        let sender = self.completion.sender();
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(tokio_util::task::AbortOnDropHandle::new(
            runtime::runtime().spawn(async move {
                let result = match resolution {
                    Some(resolution) => client.resolve_global_prompt(&resolution).await,
                    None => client.save_global_prompt(&request.save).await,
                }
                .map_err(|error| {
                    Problem::calling_forge("Couldn't save the global prompt.", &error)
                });
                sender
                    .offer(HostCompletion {
                        host: id,
                        instance,
                        result: Completion::Prompt(request, result),
                    })
                    .await;
            }),
        ));
    }

    fn active_form_host(&self, core: &Core) -> bool {
        core.active_host().map(editor_core::HostData::instance) == self.active_instance
            && core.active_host().map(|host| &host.id.0) == self.extensions.host.as_ref()
    }

    pub(crate) fn load_latest_prompt(&mut self, core: &Core) {
        if let Some(prompt) = core
            .active_host()
            .and_then(|host| host.global_prompt.as_ref())
        {
            let current_text = self.prompt_editor.borrow().text().to_owned();
            if let Some(text) = self.prompt.load(prompt, &current_text, true) {
                self.prompt_editor.borrow_mut().set_text(&text);
            }
        }
    }

    pub fn drain(&mut self, core: &mut Core) -> bool {
        let completions = self.completion.drain();
        let changed = !completions.is_empty();
        self.setting_writes
            .retain_instances(|id, instance| core.host_if_instance(id, instance).is_some());
        for completion in completions {
            if !same_host_instance(
                Some((&completion.host, completion.instance)),
                core.host(&completion.host)
                    .map(|host| (&host.id, host.instance())),
            ) {
                continue;
            }
            match completion.result {
                Completion::Setting(request, result) => {
                    let host =
                        core.host_mut_if_instance(&request.target.host, request.target.instance);
                    let client = host.as_ref().map(|host| host.client.clone());
                    let next = self.setting_writes.complete(host, &request, result);
                    if let (Some(next), Some(client)) = (next, client) {
                        self.send_setting(client, next);
                    }
                }
                Completion::Archive(request, result) => self.archive.complete(&request, result),
                Completion::Mcp(request, result) => {
                    self.extensions.complete(&request, result);
                    self.lock_mcp_fields();
                }
                Completion::McpSignOut(server, result) => {
                    if self.extensions.host.as_deref() == Some(completion.host.0.as_str())
                        && let Err(problem) = result
                    {
                        tracing::warn!(%server, error = ?problem.detail(), "signing out of an MCP server");
                        self.extensions.error = Some(problem);
                    }
                }
                Completion::Skill {
                    skill,
                    mark,
                    result,
                } => {
                    if self.active_instance == Some(completion.instance) {
                        self.skill_pending.remove(&skill);
                    }
                    match result {
                        Ok(catalog) => {
                            if let Some(host) =
                                core.host_mut_if_instance(&completion.host, completion.instance)
                            {
                                match host.adopt_skill_catalog(catalog, mark) {
                                    editor_core::Adoption::Adopted => {}
                                    editor_core::Adoption::Older
                                    | editor_core::Adoption::Overtaken => {
                                        host.load_skill_catalog(true);
                                    }
                                }
                            }
                        }
                        Err(problem) if self.active_instance == Some(completion.instance) => {
                            self.skill_error = Some(problem);
                        }
                        Err(problem) => {
                            tracing::warn!(%skill, error = ?problem.detail(), "saving skill settings on an inactive host");
                        }
                    }
                }
                Completion::Engine(request, result) => {
                    self.engines.complete(&request, result.err());
                }
                Completion::Prompt(request, result) => {
                    if !self.prompt.accepts(&request) {
                        continue;
                    }
                    let result = result.map(|prompt| {
                        let version = prompt.version;
                        let current =
                            core.host_mut(&HostId(request.host.clone()))
                                .and_then(|host| {
                                    host.remember_prompt(prompt);
                                    host.global_prompt.as_deref().cloned()
                                });
                        (version, current)
                    });
                    let current_text = self.prompt_editor.borrow().text().to_owned();
                    if let Some(text) = self.prompt.complete(&request, result, &current_text) {
                        self.prompt_editor.borrow_mut().set_text(&text);
                    }
                }
            }
        }
        self.reported_prompt_dirty = self.prompt_editor.borrow().text() != self.prompt.saved;
        changed
    }
}

fn engine_verb(action: &EngineCommand) -> &'static str {
    match action {
        EngineCommand::Install {} => "install",
        EngineCommand::Update {} => "update",
        EngineCommand::Rollback {} => "roll back",
        EngineCommand::Use { .. } => "switch the version of",
        EngineCommand::Versions {} => "list the published versions of",
        EngineCommand::Login {} | EngineCommand::LoginApiKey { .. } => "start signing in to",
        EngineCommand::LoginCode { .. } => "finish signing in to",
        EngineCommand::CancelLogin {} => "cancel signing in to",
        EngineCommand::Logout {} => "sign out of",
        EngineCommand::Refresh {} => "refresh",
        EngineCommand::ClaudeAiSignIn { .. } => "sign in to claude.ai for",
        EngineCommand::ClaudeAiSignOut {} => "sign out of claude.ai for",
    }
}

fn same_host_instance<I: PartialEq, T: PartialEq>(
    expected: Option<(&I, T)>,
    current: Option<(&I, T)>,
) -> bool {
    matches!((expected, current), (Some((expected_id, expected_instance)), Some((current_id, current_instance))) if expected_id == current_id && expected_instance == current_instance)
}

impl std::fmt::Debug for SettingsController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsController")
            .field("active_instance", &self.active_instance)
            .field("tasks", &self.tasks.len())
            .finish_non_exhaustive()
    }
}
