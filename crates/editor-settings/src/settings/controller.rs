use std::{cell::RefCell, num::NonZeroUsize, rc::Rc};

use artisan_api::{EngineCommand, EngineId, GlobalPrompt};
use serde_json::Value;

use editor_core::delivery::Delivery;
use editor_core::{Core, HostId, HostInstance, Screen, SettingsPage, ThreadPageDirection, runtime};
use editor_ui::text::editor::{Editor, Mode};

use super::archive::{ArchiveForm, ArchiveRequest};
use super::engines::{EngineRequest, EnginesForm};
use super::extensions::{ExtensionsForm, MutationRequest};
use super::preferences::{self, SettingRequest};
use super::prompt::{PromptForm, PromptRequest};

struct HostCompletion {
    host: HostId,
    instance: HostInstance,
    result: Completion,
}

enum Completion {
    Setting(HostId, SettingRequest, Result<(), String>),
    Archive(ArchiveRequest, Result<(), String>),
    Mcp(MutationRequest, Result<(), String>),
    Engine(EngineRequest, Result<(), String>),
    Prompt(PromptRequest, Result<GlobalPrompt, String>),
    McpSignOut(String, Result<(), String>),
}

pub struct SettingsController {
    pub reported_prompt_dirty: bool,
    pub reported_font_menu: bool,
    pub reported_streaming_menu: bool,
    pub reported_retention_menu: bool,
    pub mcp_editors: Vec<Rc<RefCell<Editor>>>,
    pub retention_select: Rc<RefCell<editor_ui::select::SelectState<Option<u32>>>>,
    pub font_select: Rc<RefCell<editor_ui::select::SelectState<&'static str>>>,
    pub streaming_select: Rc<RefCell<editor_ui::select::SelectState<&'static str>>>,
    pub sign_in: super::sign_in::SignInForm,
    pub forge_code: Rc<RefCell<Editor>>,
    pub forge_error: Option<String>,
    pub archive: ArchiveForm,
    pub engines: EnginesForm,
    pub extensions: ExtensionsForm,
    pub prompt: PromptForm,
    pub prompt_editor: Rc<RefCell<Editor>>,
    completion: Delivery<HostCompletion>,
    tasks: Vec<tokio_util::task::AbortOnDropHandle<()>>,
    active_instance: Option<HostInstance>,
}

impl Default for SettingsController {
    fn default() -> Self {
        Self {
            reported_prompt_dirty: false,
            reported_font_menu: false,
            reported_streaming_menu: false,
            reported_retention_menu: false,
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
            sign_in: super::sign_in::SignInForm::default(),
            forge_code: Rc::new(RefCell::new(Editor::new("", Mode::SingleLine))),
            forge_error: None,
            archive: ArchiveForm::default(),
            engines: EnginesForm::default(),
            extensions: ExtensionsForm::default(),
            prompt: PromptForm::default(),
            prompt_editor: Rc::new(RefCell::new(Editor::new("", Mode::Document))),
            completion: Delivery::new(NonZeroUsize::new(64).unwrap()),
            tasks: Vec::new(),
            active_instance: None,
        }
    }
}

impl SettingsController {
    pub fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        self.completion.wake_mailbox().unwrap()
    }

    pub fn sync_editors(&self, ui: &mut aqua::UiApp) {
        let mut editors = vec![
            ("sign-in-email".to_owned(), self.sign_in.email.clone()),
            (
                "sign-in-second-factor".to_owned(),
                self.sign_in.second_factor.clone(),
            ),
            ("forges-code".to_owned(), self.forge_code.clone()),
            (
                "settings-global-prompt-editor".to_owned(),
                self.prompt_editor.clone(),
            ),
        ];
        editors.extend(
            self.mcp_editors
                .iter()
                .enumerate()
                .map(|(index, editor)| (format!("settings-mcp-field-{index}"), editor.clone())),
        );
        editors.extend(self.engines.login_codes.iter().map(|(engine, editor)| {
            (
                format!("settings-engine-login-code-{engine}"),
                editor.clone(),
            )
        }));
        for (key, editor) in editors {
            let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key));
            if let Some(node) = ui.tree().sole_node_with_key(hash) {
                editor_ui::text::mount::EditorController::from_shared(editor, |_| {})
                    .sync(ui.tree_mut(), node);
            }
        }
    }

    pub fn sync(&mut self, core: &mut Core) {
        let instance = core.active_host().map(|host| host.instance());
        if self.active_instance != instance {
            self.active_instance = instance;
            self.engines.sync_host(None);
            self.extensions.sync_host(None);
            self.archive.sync(None);
            self.prompt.sync_host(None);
            self.prompt_editor.borrow_mut().set_text("");
        }
        let host = core.active_host().map(|host| host.id.clone());
        let host_name = host.as_ref().map(|host| host.0.clone());
        self.engines.sync_host(host_name.clone());
        self.extensions.sync_host(host_name.clone());
        let page = match core.screen() {
            Screen::Settings(page) => *page,
            _ => return,
        };
        if let SettingsPage::Engine(engine) = page.parent() {
            self.engines.prepare(engine);
        }
        if page.parent() == SettingsPage::Threads {
            self.archive
                .sync(host.as_ref().and_then(|id| core.host(id)));
            if let Some(host) = host.as_ref().and_then(|id| core.host_mut(id)) {
                let page = host.thread_page(true);
                if !page.loaded && !page.loading {
                    host.load_threads(true, ThreadPageDirection::Latest);
                }
            }
        }
        if page != SettingsPage::GlobalPrompt {
            return;
        }
        if self.prompt.sync_host(host_name) {
            self.prompt_editor.borrow_mut().set_text("");
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

    pub fn core_event(&mut self, core: &mut Core, event: &editor_core::CoreEvent) {
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
            _ => {}
        }
        self.sync(core);
    }

    pub fn claim_forge(&mut self, core: &mut Core) {
        let code = self.forge_code.borrow().text().to_owned();
        core.account_mut().claim_code(code);
        self.forge_code.borrow_mut().set_text("");
    }

    pub fn unarchive(&mut self, core: &Core, thread: artisan_api::ThreadId) {
        if core.active_host().map(|host| host.instance()) != self.active_instance
            || core.active_host().map(|host| &host.id.0) != self.archive.host.as_ref()
        {
            return;
        }
        let Some(request) = self.archive.begin(thread) else {
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
                let update = artisan_api::UpdateThread {
                    archived: Some(false),
                    ..Default::default()
                };
                let result = client
                    .update_thread(&request.thread, &update)
                    .await
                    .map(|_| ())
                    .map_err(|error| format!("{error:#}"));
                let _ = sender
                    .send(HostCompletion {
                        host: id,
                        instance,
                        result: Completion::Archive(request, result),
                    })
                    .await;
            }),
        ));
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
        let request = preferences::begin(&mut host.settings, key, value);
        let sender = self.completion.sender();
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(tokio_util::task::AbortOnDropHandle::new(
            runtime::runtime().spawn(async move {
                let result = client
                    .set_setting(request.key, &request.value)
                    .await
                    .map_err(|error| format!("{error:#}"));
                let _ = sender
                    .send(HostCompletion {
                        host: id.clone(),
                        instance,
                        result: Completion::Setting(id, request, result),
                    })
                    .await;
            }),
        ));
    }

    pub fn open_mcp(&mut self, previous: Option<artisan_api::McpServer>) {
        self.extensions.open(previous);
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

    pub fn save_mcp(&mut self, core: &Core) {
        let fields = &mut self.extensions.fields;
        fields.name = self.mcp_editors[0].borrow().text().to_owned();
        fields.command = self.mcp_editors[1].borrow().text().to_owned();
        fields.args = self.mcp_editors[2].borrow().text().to_owned();
        fields.url = self.mcp_editors[3].borrow().text().to_owned();
        fields.env = self.mcp_editors[4].borrow().text().to_owned();
        fields.headers = self.mcp_editors[5].borrow().text().to_owned();
        if !self.active_form_host(core) {
            return;
        }
        if let Some(request) = self.extensions.save() {
            self.mutate_mcp(core, request);
        }
    }

    pub fn remove_mcp(&mut self, core: &Core, previous: artisan_api::McpServer) {
        if !self.active_form_host(core) {
            return;
        }
        if let Some(request) = self.extensions.remove(previous) {
            self.mutate_mcp(core, request);
        }
    }

    pub fn sign_in_mcp(&mut self, core: &mut Core, server: &str) {
        if !self.active_form_host(core) {
            return;
        }
        let Some(host) = core.active_host().map(|host| host.id.clone()) else {
            return;
        };
        self.extensions.error = None;
        drop(core.start_mcp_sign_in(&host, server, true));
    }

    pub fn cancel_mcp_sign_in(&mut self, core: &mut Core, server: &str) {
        if !self.active_form_host(core) {
            return;
        }
        if let Some(host) = core.active_host().map(|host| host.id.clone()) {
            core.cancel_mcp_sign_in(&host, server);
        }
    }

    pub fn sign_out_mcp(&mut self, core: &Core, server: String) {
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
                    .map_err(|error| format!("{error:#}"));
                let _ = sender
                    .send(HostCompletion {
                        host: id,
                        instance,
                        result: Completion::McpSignOut(server, result),
                    })
                    .await;
            }),
        ));
    }

    pub fn sign_in_claude_ai(&mut self, core: &Core) {
        if core.active_host().map(|host| host.instance()) != self.active_instance {
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
                    let session =
                        claude_ai_browser::capture(claude_ai_browser::Mode::SignIn).await?;
                    client
                        .engine_action(EngineId::Claude, &EngineCommand::ClaudeAiSignIn { session })
                        .await?;
                    Ok::<(), anyhow::Error>(())
                }
                .await
                .map_err(|error| format!("Couldn't sign in to claude.ai: {error:#}"));
                let _ = sender
                    .send(HostCompletion {
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
                    .map_err(|error| format!("{error:#}"));
                let _ = sender
                    .send(HostCompletion {
                        host: id,
                        instance,
                        result: Completion::Mcp(request, result),
                    })
                    .await;
            }),
        ));
    }

    pub fn engine_action(&mut self, core: &Core, engine: EngineId, action: EngineCommand) {
        if core.active_host().map(|host| host.instance()) != self.active_instance
            || core.active_host().map(|host| &host.id.0) != self.engines.host.as_ref()
        {
            return;
        }
        let Some(request) = self.engines.begin(engine) else {
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
                        format!("Couldn't {verb} {}: {error:#}", engine.display_name())
                    });
                let _ = sender
                    .send(HostCompletion {
                        host: id,
                        instance,
                        result: Completion::Engine(request, result),
                    })
                    .await;
            }),
        ));
    }

    pub fn save_prompt(&mut self, core: &Core) {
        self.submit_prompt(core, None);
    }

    pub fn resolve_prompt(&mut self, core: &Core, resolution: artisan_api::ResolvePrompt) {
        self.submit_prompt(core, Some(resolution));
    }

    fn submit_prompt(&mut self, core: &Core, resolution: Option<artisan_api::ResolvePrompt>) {
        if core.active_host().map(|host| host.instance()) != self.active_instance
            || core.active_host().map(|host| &host.id.0) != self.prompt.host.as_ref()
        {
            return;
        }
        let Some(request) = self
            .prompt
            .save(&self.prompt_editor.borrow().text().to_owned())
        else {
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
                .map_err(|error| format!("{error:#}"));
                let _ = sender
                    .send(HostCompletion {
                        host: id,
                        instance,
                        result: Completion::Prompt(request, result),
                    })
                    .await;
            }),
        ));
    }

    fn active_form_host(&self, core: &Core) -> bool {
        core.active_host().map(|host| host.instance()) == self.active_instance
            && core.active_host().map(|host| &host.id.0) == self.extensions.host.as_ref()
    }

    pub fn load_latest_prompt(&mut self, core: &Core) {
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

    #[cfg(feature = "test-support")]
    pub fn complete_setting_for_tests(
        &mut self,
        host: HostId,
        instance: HostInstance,
        request: SettingRequest,
        result: Result<(), String>,
    ) {
        let sender = self.completion.sender();
        runtime::runtime().block_on(async move {
            let _ = sender
                .send(HostCompletion {
                    host: host.clone(),
                    instance,
                    result: Completion::Setting(host, request, result),
                })
                .await;
        });
    }

    pub fn drain(&mut self, core: &mut Core) -> bool {
        let completions = self.completion.drain();
        let changed = !completions.is_empty();
        for completion in completions {
            if !same_host_instance(
                Some((&completion.host, completion.instance)),
                core.host(&completion.host)
                    .map(|host| (&host.id, host.instance())),
            ) {
                continue;
            }
            match completion.result {
                Completion::Setting(id, request, Err(error)) => {
                    tracing::warn!(key = request.key, "saving a Forge setting: {error}");
                    if let Some(host) = core.host_mut(&id) {
                        preferences::rollback(&mut host.settings, request);
                    }
                }
                Completion::Setting(_, _, Ok(())) => {}
                Completion::Archive(request, result) => self.archive.complete(&request, result),
                Completion::Mcp(request, result) => self.extensions.complete(&request, result),
                Completion::McpSignOut(server, result) => {
                    if self.extensions.host.as_deref() == Some(completion.host.0.as_str()) {
                        if let Err(error) = result {
                            self.extensions.error =
                                Some(format!("Couldn't sign out of {server}: {error}"));
                        }
                    }
                }
                Completion::Engine(request, result) => {
                    self.engines.complete(&request, result.err())
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
        EngineCommand::Login {} => "start signing in to",
        EngineCommand::LoginCode { .. } => "finish signing in to",
        EngineCommand::CancelLogin {} => "cancel signing in to",
        EngineCommand::Logout {} => "sign out of",
        EngineCommand::Refresh {} => "refresh",
        EngineCommand::ClaudeAiSignIn { .. } => "sign in to claude.ai for",
        EngineCommand::ClaudeAiSignOut {} => "sign out of claude.ai for",
    }
}

pub fn same_host_instance<I: PartialEq, T: PartialEq>(
    expected: Option<(&I, T)>,
    current: Option<(&I, T)>,
) -> bool {
    matches!((expected, current), (Some((expected_id, expected_instance)), Some((current_id, current_instance))) if expected_id == current_id && expected_instance == current_instance)
}
