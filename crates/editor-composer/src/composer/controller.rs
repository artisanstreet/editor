use super::{
    attachment::{Attachments, Source},
    draft_session::Submission,
    mentions::Mentions,
    persistence::{DraftLease, DraftSessions, SessionEvent},
    picker::{Menu, PickerState},
    questionnaire::QuestionnairePanel,
};
use editor_core::{Core, HostData, HostId, HostInstance, delivery::Delivery, runtime::runtime};
use editor_ui::text::editor::{Editor, ListKey, Mode};
use artisan_api::{
    BlobHash, CreateThread, EngineConfig, ItemId, ProjectId, SendMessage, SignInState, ThreadId,
};
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, num::NonZeroUsize, path::PathBuf, rc::Rc};

#[derive(Clone, Copy)]
enum TurnAction {
    Stop,
    Continue,
    ClearGoal,
    Unpark,
    Permissions,
}
struct TurnCompletion {
    instance: HostInstance,
    generation: u64,
    thread: ThreadId,
    action: TurnAction,
    error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerTarget {
    Thread {
        host: HostId,
        thread: ThreadId,
    },
    NewThread {
        host: HostId,
        project: Option<ProjectId>,
    },
}
impl ComposerTarget {
    pub fn host(&self) -> &HostId {
        match self {
            Self::Thread { host, .. } | Self::NewThread { host, .. } => host,
        }
    }
    pub fn scope(&self) -> String {
        match self {
            Self::Thread { thread, .. } => format!("thread:{thread}"),
            Self::NewThread { project, .. } => {
                format!("new:{}", project.as_ref().map_or("", ProjectId::as_str))
            }
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DraftBody {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub images: Vec<BlobHash>,
    #[serde(default)]
    pub files: Vec<DraftFile>,
    #[serde(default)]
    pub config: Option<EngineConfig>,
    #[serde(default)]
    pub project: Option<ProjectId>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DraftFile {
    #[serde(skip)]
    pub key: u64,
    pub path: PathBuf,
    #[serde(default)]
    pub folder: bool,
}

pub struct Composer {
    pub target: ComposerTarget,
    pub editor: Rc<RefCell<Editor>>,
    pub questionnaire: QuestionnairePanel,
    pub strip: super::goal_strip::StripState,
    pub mentions: Mentions,
    pub picker: PickerState,
    pub config: Option<EngineConfig>,
    pub project: Option<ProjectId>,
    pub attachments: Option<Attachments>,
    pub files: Vec<DraftFile>,
    next_file_key: u64,
    placeholder_generation: u64,
    placeholder_was_visible: bool,
    pub file_shake: Option<(u64, std::time::Instant)>,
    pub notice: Option<String>,
    pub session: Option<DraftLease>,
    host_instance: Option<HostInstance>,
    target_generation: u64,
    pub cancelling: bool,
    pub continuing: bool,
    turn_completions: Delivery<TurnCompletion>,
    edit_revision: u64,
    restore_revision: Option<u64>,
}

impl Composer {
    pub fn new(core: &Core, sessions: &mut DraftSessions, target: ComposerTarget) -> Self {
        let session = core.host(target.host()).and_then(|host| {
            sessions.open(
                host.id.clone(),
                host.instance(),
                target.scope(),
                host.client.clone(),
            )
        });
        let mut editor = Editor::new("", Mode::Prompt);
        editor.set_disabled(session.is_none());
        let notice = (session.is_none() && core.host(target.host()).is_some()).then(|| "There are 64 open or unsaved drafts. Finish saving a draft, then reopen this thread.".to_owned());
        let host_instance = core.host(target.host()).map(HostData::instance);
        let attachments = core
            .host(target.host())
            .map(|host| Attachments::new(host.client.clone()));
        let picker = PickerState::for_target(core, target.host());
        Self {
            target,
            editor: Rc::new(RefCell::new(editor)),
            questionnaire: QuestionnairePanel::default(),
            strip: super::goal_strip::StripState::default(),
            mentions: Mentions::default(),
            picker,
            config: None,
            project: None,
            attachments,
            files: Vec::new(),
            next_file_key: 0,
            placeholder_generation: 0,
            placeholder_was_visible: false,
            file_shake: None,
            notice,
            session,
            host_instance,
            target_generation: 0,
            cancelling: false,
            continuing: false,
            turn_completions: Delivery::new(NonZeroUsize::MIN),
            edit_revision: 0,
            restore_revision: Some(0),
        }
    }

    pub fn set_target(
        &mut self,
        core: &Core,
        sessions: &mut DraftSessions,
        target: ComposerTarget,
    ) {
        if target == self.target
            && core.host(target.host()).map(HostData::instance) == self.host_instance
        {
            return;
        }
        if let Some(session) = &self.session {
            session.flush();
        }
        self.questionnaire = QuestionnairePanel::default();
        self.strip = super::goal_strip::StripState::default();
        self.mentions = Mentions::default();
        self.picker = PickerState::for_target(core, target.host());
        self.target = target;
        self.target_generation = self.target_generation.wrapping_add(1);
        self.edit_revision = 0;
        self.cancelling = false;
        self.continuing = false;
        {
            let mut editor = self.editor.borrow_mut();
            editor.set_text("");
            editor.set_disabled(false);
            editor.set_list_open(false);
            editor.set_suggestion(None);
            editor.set_tokenizer(None);
        }
        self.session = None;
        self.files.clear();
        self.file_shake = None;
        self.restore_revision = Some(0);
        self.notice = None;
        self.config = None;
        self.project = None;
        self.host_instance = core.host(self.target.host()).map(HostData::instance);
        self.attachments = core
            .host(self.target.host())
            .map(|host| Attachments::new(host.client.clone()));
        self.session = core.host(self.target.host()).and_then(|host| {
            sessions.open(
                host.id.clone(),
                host.instance(),
                self.target.scope(),
                host.client.clone(),
            )
        });
        if self.session.is_none() && self.host_instance.is_some() {
            self.notice = Some("There are 64 open or unsaved drafts. Finish saving a draft, then reopen this thread.".into());
            self.editor.borrow_mut().set_disabled(true);
        }
    }

    pub fn host_instance(&self) -> Option<HostInstance> {
        self.host_instance
    }

    pub fn matches_host(&self, host: &HostData) -> bool {
        self.host_instance == Some(host.instance())
    }

    pub fn generation(&self) -> u64 {
        self.target_generation
    }

    pub fn mount_editor(
        &self,
        event: impl Fn(editor_ui::text::mount::EditorEvent) + 'static,
    ) -> editor_ui::text::mount::EditorController {
        editor_ui::text::mount::EditorController::from_shared(Rc::clone(&self.editor), event)
    }

    pub fn turn_wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.turn_completions.wake_mailbox()
    }

    pub fn stop(&mut self, host: &HostData) {
        self.turn_action(host, TurnAction::Stop);
    }

    pub fn clear_goal(&mut self, host: &HostData) {
        self.turn_action(host, TurnAction::ClearGoal);
    }
    pub fn unpark(&mut self, host: &HostData) {
        self.turn_action(host, TurnAction::Unpark);
    }

    pub fn continue_turn(&mut self, host: &HostData) {
        self.turn_action(host, TurnAction::Continue);
    }

    fn turn_action(&mut self, host: &HostData, action: TurnAction) {
        let ComposerTarget::Thread { thread, .. } = &self.target else {
            return;
        };
        if self.host_instance != Some(host.instance()) {
            return;
        }
        match action {
            TurnAction::Stop if self.cancelling => return,
            TurnAction::Continue if self.continuing => return,
            TurnAction::Stop => self.cancelling = true,
            TurnAction::Continue => self.continuing = true,
            TurnAction::ClearGoal | TurnAction::Unpark | TurnAction::Permissions => {}
        }
        let thread = thread.clone();
        let client = host.client.clone();
        let instance = host.instance();
        let generation = self.target_generation;
        let delivery = self.turn_completions.sender();
        runtime().spawn(async move {
            let result = match action {
                TurnAction::Stop => client.cancel(&thread).await,
                TurnAction::Continue => client.continue_turn(&thread).await,
                TurnAction::ClearGoal => client.clear_goal(&thread).await,
                TurnAction::Unpark => client.cancel(&thread).await,
                TurnAction::Permissions => unreachable!(),
            };
            let error = result.err().map(|error| format!("{error:#}"));
            let _ = delivery
                .send(TurnCompletion {
                    instance,
                    generation,
                    thread,
                    action,
                    error,
                })
                .await;
        });
    }

    pub fn apply_permissions(&self, host: &HostData, permissions: artisan_api::Permissions) {
        let ComposerTarget::Thread { thread, .. } = &self.target else {
            return;
        };
        if !self.matches_host(host) {
            return;
        }
        let Some(mut config) = host.threads.get(thread).map(|thread| thread.config.clone()) else {
            return;
        };
        if config.permissions == permissions {
            return;
        }
        config.permissions = permissions;
        let client = host.client.clone();
        let thread = thread.clone();
        let instance = host.instance();
        let generation = self.target_generation;
        let delivery = self.turn_completions.sender();
        runtime().spawn(async move {
            let update = artisan_api::UpdateThread {
                config: Some(config),
                ..Default::default()
            };
            let error = client
                .update_thread(&thread, &update)
                .await
                .err()
                .map(|error| format!("{error:#}"));
            let _ = delivery
                .send(TurnCompletion {
                    instance,
                    generation,
                    thread,
                    action: TurnAction::Permissions,
                    error,
                })
                .await;
        });
    }

    pub fn drain_turn_actions(&mut self, core: &Core) -> bool {
        let before = (self.cancelling, self.continuing, self.notice.clone());
        for completion in self.turn_completions.drain() {
            if core.host(self.target.host()).map(HostData::instance) != Some(completion.instance)
                || self.target_generation != completion.generation
                || !matches!(&self.target, ComposerTarget::Thread { thread, .. } if *thread == completion.thread)
            {
                continue;
            }
            match completion.action {
                TurnAction::Stop => {
                    self.cancelling = false;
                    if let Some(error) = completion.error {
                        self.notice = Some(format!("Couldn't stop the run: {error}"));
                    }
                }
                TurnAction::Continue => {
                    self.continuing = false;
                    if let Some(error) = completion.error {
                        self.notice = Some(format!("Couldn't continue: {error}"));
                    }
                }
                TurnAction::ClearGoal => {
                    if let Some(error) = completion.error {
                        self.notice = Some(format!("Couldn't clear the goal: {error}"));
                    }
                }
                TurnAction::Unpark => {
                    if let Some(error) = completion.error {
                        self.notice = Some(format!("Couldn't cancel the pickup: {error}"));
                    }
                }
                TurnAction::Permissions => {
                    if let Some(error) = completion.error {
                        self.notice = Some(format!("Couldn't change access: {error}"));
                    }
                }
            }
        }
        before != (self.cancelling, self.continuing, self.notice.clone())
    }

    pub fn draft_editable(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.read(|state| !state.sending()))
    }

    pub fn metadata_ready(&self, host: &HostData) -> bool {
        match &self.target {
            ComposerTarget::Thread { thread, .. } => host.threads.contains_key(thread),
            ComposerTarget::NewThread { .. } => true,
        }
    }

    pub fn open_picker(&mut self, menu: Menu, host: &HostData) {
        if self.host_instance != Some(host.instance())
            || !self.metadata_ready(host)
            || !self.draft_editable()
        {
            return;
        }
        self.picker.menu = Some(menu);
        if menu == Menu::Project {
            self.picker.project_edges.reset();
        }
        if menu == Menu::Model {
            self.picker.open_serial += 1;
            self.picker.axis = None;
            self.picker.axis_cursor = None;
            self.picker.model_edges.reveal();
        }
        self.refresh_picker(host);
    }

    pub fn refresh_picker(&mut self, host: &HostData) {
        if self.host_instance != Some(host.instance()) {
            return;
        }
        let config = self.effective_config(host);
        self.picker.refresh(host, config.as_ref());
    }

    pub fn choose_model(
        &mut self,
        host: &HostData,
        engine: artisan_api::EngineId,
        id: &str,
    ) -> Option<EngineConfig> {
        if !self.matches_host(host) || !self.metadata_ready(host) || !self.draft_editable() {
            return None;
        }
        let config = PickerState::model_config(host, self.effective_config(host), engine, id)?;
        self.config = Some(config.clone());
        self.edited();
        Some(config)
    }

    pub fn choose_project(&mut self, id: ProjectId) {
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.read(|state| state.sending()))
        {
            return;
        }
        self.project = Some(id);
        self.picker.menu = None;
        self.edited();
    }

    pub fn refresh_mentions(&mut self, host: &HostData) {
        if self.host_instance != Some(host.instance()) {
            return;
        }
        let engine = self.effective_config(host).map(|config| config.engine);
        let own = match &self.target {
            ComposerTarget::Thread { thread, .. } => Some(thread),
            _ => None,
        };
        self.mentions.refresh(host, engine, own);
        self.sync_mentions();
    }

    pub fn sync_mentions(&mut self) {
        let caret = {
            let editor = self.editor.borrow();
            (!editor.is_disabled()).then(|| editor.caret()).flatten()
        };
        if let Some(caret) = caret {
            self.mentions.sync(self.editor.borrow().text(), caret);
        } else {
            self.mentions.clear_trigger();
        }
        self.editor
            .borrow_mut()
            .set_list_open(!self.mentions.rows.is_empty());
    }

    pub fn mention_key(&mut self, key: ListKey) {
        if self.mentions.rows.is_empty() {
            return;
        }
        match key {
            ListKey::Up => self.mentions.up(),
            ListKey::Down => self.mentions.down(),
            ListKey::Dismiss => self.mentions.dismiss(),
            ListKey::Accept => {
                let replacement = {
                    let editor = self.editor.borrow();
                    self.mentions
                        .replacement(editor.text(), self.mentions.highlighted)
                };
                if let Some((range, text)) = replacement {
                    self.editor.borrow_mut().replace(range, &text);
                    self.edited();
                }
            }
        }
        self.sync_mentions();
    }

    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.read(|state| state.sending()))
        {
            return;
        }
        for path in paths {
            let folder = path.is_dir();
            let image = !folder
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
                        )
                    });
            if image {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                self.add_image(Source::Path(path), name);
            } else if let Some(existing) = self.files.iter().find(|file| file.path == path) {
                self.file_shake = Some((existing.key, std::time::Instant::now()));
            } else {
                if self.files.len() >= 32 {
                    self.notice = Some("A message can link at most 32 files.".into());
                    return;
                }
                let key = self.next_file_key;
                self.next_file_key = self.next_file_key.wrapping_add(1);
                self.files.push(DraftFile { key, path, folder });
                self.edited();
            }
        }
    }

    pub fn remove_image(&mut self, key: u64) {
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.read(|state| state.sending()))
        {
            return;
        }
        if let Some(attachments) = &mut self.attachments {
            attachments.remove(key);
            self.edited();
        }
    }

    pub fn remove_file(&mut self, key: u64) {
        if !self.draft_editable() {
            return;
        }
        let Some(index) = self.files.iter().position(|file| file.key == key) else {
            return;
        };
        self.files.remove(index);
        self.edited();
    }

    pub fn add_image(&mut self, source: Source, name: String) {
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.read(|state| state.sending()))
        {
            return;
        }
        if self
            .attachments
            .as_mut()
            .is_some_and(|attachments| attachments.add(source, name))
        {
            self.edited();
        }
    }

    pub fn drain_attachments(&mut self, core: &Core, assets: &aqua::images::ImageAssets) -> bool {
        if core.host(self.target.host()).map(HostData::instance) != self.host_instance {
            return false;
        }
        if self
            .attachments
            .as_mut()
            .is_some_and(|attachments| attachments.drain(assets))
        {
            self.save();
            return true;
        }
        false
    }

    pub fn edited(&mut self) {
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.save();
    }

    pub fn save(&self) {
        let Some(session) = &self.session else {
            return;
        };
        let body = DraftBody {
            text: self
                .questionnaire
                .aside_text()
                .map(str::to_owned)
                .unwrap_or_else(|| self.editor.borrow().text().to_owned()),
            images: self
                .attachments
                .as_ref()
                .map(|a| {
                    a.items
                        .iter()
                        .filter_map(|item| item.state.hash().cloned())
                        .collect()
                })
                .unwrap_or_default(),
            files: self.files.clone(),
            config: self.config.clone(),
            project: self.project.clone(),
        };
        let value = if body.text.is_empty()
            && body.images.is_empty()
            && body.files.is_empty()
            && body.config.is_none()
            && body.project.is_none()
        {
            serde_json::Value::Null
        } else {
            serde_json::to_value(body).expect("DraftBody serializes")
        };
        session.queue(value);
    }

    pub fn sync(&mut self, core: &Core) -> Vec<SessionEvent> {
        if core.host(self.target.host()).map(HostData::instance) != self.host_instance {
            self.session = None;
            self.editor.borrow_mut().set_disabled(true);
            return Vec::new();
        }
        let Some(session) = &mut self.session else {
            return Vec::new();
        };
        let events = session.drain_events();
        for event in &events {
            match event {
                SessionEvent::Accepted { .. } => {
                    self.config = None;
                    self.editor.borrow_mut().set_text("");
                    if let Some(attachments) = &mut self.attachments {
                        attachments.clear();
                    }
                    self.files.clear();
                    self.restore_revision = None;
                }
                SessionEvent::Reloaded => {
                    self.editor.borrow_mut().set_text("");
                    if let Some(attachments) = &mut self.attachments {
                        attachments.clear();
                    }
                    self.files.clear();
                    self.restore_revision = Some(self.edit_revision);
                }
                SessionEvent::Changed => {}
            }
        }
        let (loaded, sending, body) =
            session.read(|state| (state.loaded, state.sending(), state.body.clone()));
        self.editor
            .borrow_mut()
            .set_disabled(sending || self.questionnaire.sending.is_some());
        if loaded
            && self.restore_revision.take() == Some(self.edit_revision)
            && body != serde_json::Value::Null
        {
            match serde_json::from_value::<DraftBody>(body) {
                Ok(body)
                    if self.editor.borrow().text().trim().is_empty()
                        && self.attachments.as_ref().is_none_or(|a| a.items.is_empty())
                        && self.files.is_empty() =>
                {
                    if !self.questionnaire.restore_aside_text(&body.text) {
                        self.editor.borrow_mut().set_text(&body.text);
                    }
                    self.config = body.config;
                    self.project = body.project;
                    if let Some(attachments) = &mut self.attachments {
                        for hash in body.images {
                            attachments.restore(hash);
                        }
                    }
                    self.files = body
                        .files
                        .into_iter()
                        .map(|mut file| {
                            file.key = self.next_file_key;
                            self.next_file_key = self.next_file_key.wrapping_add(1);
                            file
                        })
                        .collect();
                }
                Ok(_) => {}
                Err(error) => self.notice = Some(format!("Couldn't read this draft: {error}")),
            }
        }
        events
    }

    pub fn project(&self, host: &HostData) -> Option<ProjectId> {
        match &self.target {
            ComposerTarget::NewThread { project, .. } => self
                .project
                .clone()
                .or_else(|| project.clone())
                .or_else(|| last_project(host)),
            ComposerTarget::Thread { .. } => self.project.clone(),
        }
    }

    pub fn effective_config(&self, host: &HostData) -> Option<EngineConfig> {
        self.config.clone().or_else(|| match &self.target {
            ComposerTarget::Thread { thread, .. } => {
                host.threads.get(thread).map(|thread| thread.config.clone())
            }
            ComposerTarget::NewThread { .. } => {
                remembered_config(host).or_else(|| default_config(host))
            }
        })
    }

    pub fn placeholder(&mut self, question: Option<&artisan_api::Question>) -> &'static str {
        let visible = self.editor.borrow().text().trim().is_empty()
            && self.files.is_empty()
            && self
                .attachments
                .as_ref()
                .is_none_or(|items| items.items.is_empty());
        let answer = if self.questionnaire.writing_answer() {
            Some(match question {
                None => "Loading question…",
                Some(question) if question.options.is_empty() => {
                    "Type your answer, then press Enter"
                }
                Some(_) => "Pick an answer above, or type your own",
            })
        } else {
            None
        };
        if answer.is_none() && visible && !self.placeholder_was_visible {
            self.placeholder_generation = self.placeholder_generation.wrapping_add(1);
        }
        self.placeholder_was_visible = visible && !self.questionnaire.writing_answer();
        answer.unwrap_or_else(|| super::placeholder::phrase(self.placeholder_generation))
    }

    pub fn blocked(&self, host: &HostData) -> Option<&'static str> {
        if matches!(&self.target, ComposerTarget::Thread { thread, .. } if !host.threads.contains_key(thread))
        {
            return Some("Loading thread settings…");
        }
        let Some(session) = &self.session else {
            return Some("Draft unavailable");
        };
        if session.read(|state| state.sending()) {
            return Some("Sending…");
        }
        let items = self
            .attachments
            .as_ref()
            .map(|attachments| attachments.items.as_slice())
            .unwrap_or_default();
        if self.editor.borrow().text().trim().is_empty()
            && items.is_empty()
            && self.files.is_empty()
        {
            return Some("");
        }
        if items
            .iter()
            .any(|attachment| attachment.state.failure().is_some())
        {
            return Some("Remove the images that failed");
        }
        if items
            .iter()
            .any(|attachment| attachment.state.hash().is_none())
        {
            return Some("");
        }
        if matches!(self.target, ComposerTarget::NewThread { .. }) && self.project(host).is_none() {
            return Some("Choose a project");
        }
        if self.effective_config(host).is_none() {
            return Some("Choose a model");
        }
        None
    }

    pub fn primary(&self, host: &HostData) -> (super::view::Primary, bool) {
        let working = match &self.target {
            ComposerTarget::Thread { thread, .. } => host.is_working(thread),
            _ => false,
        };
        let ready = if self.questionnaire.writing_answer() {
            !self.editor.borrow().text().trim().is_empty()
        } else {
            self.blocked(host).is_none()
        };
        let can_continue = matches!(&self.target, ComposerTarget::Thread { thread, .. } if !self.questionnaire.writing_answer() && self.editor.borrow().text().trim().is_empty() && self.files.is_empty() && self.attachments.as_ref().is_none_or(|items| items.items.is_empty()) && host.can_continue(thread));
        if working && !ready {
            (super::view::Primary::Stop, self.cancelling)
        } else if !working && !ready && can_continue {
            (super::view::Primary::Continue, self.continuing)
        } else {
            (super::view::Primary::Send, !ready)
        }
    }

    pub fn send(&mut self, host: &HostData) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        if self.host_instance != Some(host.instance())
            || self.questionnaire.writing_answer()
            || matches!(&self.target, ComposerTarget::Thread { thread, .. } if !host.threads.contains_key(thread))
        {
            return false;
        }
        if session.read(|state| state.sending())
            || (self.editor.borrow().text().trim().is_empty()
                && self.attachments.as_ref().is_none_or(|a| a.items.is_empty())
                && self.files.is_empty())
        {
            return false;
        }
        let Some(images) = self
            .attachments
            .as_ref()
            .and_then(Attachments::ready_hashes)
        else {
            return false;
        };
        let Some(config) = self.effective_config(host) else {
            return false;
        };
        let (create, thread_config) = match &self.target {
            ComposerTarget::Thread { thread, .. } => (
                None,
                host.threads.get(thread).map(|thread| thread.config.clone()),
            ),
            ComposerTarget::NewThread { .. } => {
                let Some(project_id) = self.project(host) else {
                    return false;
                };
                (
                    Some(CreateThread {
                        id: ThreadId::new(),
                        project_id,
                        config: config.clone(),
                        title: None,
                        parent_thread_id: None,
                        spawned_by: None,
                    }),
                    Some(config.clone()),
                )
            }
        };
        let thread = match (&create, &self.target) {
            (Some(create), _) => create.id.clone(),
            (None, ComposerTarget::Thread { thread, .. }) => thread.clone(),
            _ => return false,
        };
        let message = SendMessage {
            id: ItemId::new(),
            text: file_links(self.editor.borrow().text().trim(), &self.files),
            images,
            config: (thread_config.as_ref() != Some(&config)).then_some(config),
            draft: None,
        };
        self.save();
        if let Some(session) = &self.session {
            session.submit(Submission {
                message,
                create,
                thread,
            });
        }
        true
    }
}

fn file_links(text: &str, files: &[DraftFile]) -> String {
    if files.is_empty() {
        return text.to_owned();
    }
    let links = files
        .iter()
        .map(|file| {
            let slash = if file.folder { "/" } else { "" };
            let name = file
                .path
                .file_name()
                .map_or_else(
                    || file.path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                )
                .replace(['[', ']'], "");
            format!("[{name}{slash}](<{}>)", file.path.display())
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        links
    } else {
        format!("{text}\n\n{links}")
    }
}

fn last_project(host: &HostData) -> Option<ProjectId> {
    let id = ProjectId(
        host.setting(artisan_api::LAST_PROJECT_KEY)?
            .as_str()?
            .to_owned(),
    );
    host.projects
        .get(&id)
        .is_some_and(|project| !project.removed)
        .then_some(id)
}

fn remembered_config(host: &HostData) -> Option<EngineConfig> {
    let mut config: EngineConfig =
        serde_json::from_value(host.setting("default_engine_config")?.clone()).ok()?;
    if let Some(saved) = config.model.as_deref().and_then(|model| {
        artisan_api::saved_model_config(
            host.setting(artisan_api::MODEL_CONFIGS_KEY)?,
            config.engine,
            model,
        )
    }) {
        config = saved;
    }
    fitted_remembered_config(host, config)
}

pub fn remembered_harness_config(
    host: &HostData,
    engine: artisan_api::EngineId,
) -> Option<EngineConfig> {
    if let Some(config) = host.pending_harness_config(engine) {
        return fitted_remembered_config(host, config.clone());
    }
    let value = host
        .setting(artisan_api::HARNESS_CONFIGS_KEY)
        .and_then(|configs| configs.get(engine.as_str()))
        .or_else(|| host.setting("default_engine_config"))?;
    let config: EngineConfig = serde_json::from_value(value.clone()).ok()?;
    if config.engine != engine {
        return None;
    }
    fitted_remembered_config(host, config)
}

fn fitted_remembered_config(host: &HostData, mut config: EngineConfig) -> Option<EngineConfig> {
    let status = host.engines.get(&config.engine)?;
    if !matches!(status.install, artisan_api::InstallState::Installed { .. })
        || matches!(
            status.sign_in,
            SignInState::SignedOut | SignInState::LoggingIn { .. } | SignInState::Failed { .. }
        )
    {
        return None;
    }
    if let Some(id) = &config.model
        && !status.models.is_empty()
    {
        status
            .models
            .iter()
            .find(|model| &model.id == id)?
            .fit(&mut config);
    }
    Some(config)
}

fn default_config(host: &HostData) -> Option<EngineConfig> {
    let engines = || host.engines.values();
    let Some(status) = engines()
        .find(|status| {
            matches!(status.sign_in, SignInState::SignedIn { .. }) && !status.models.is_empty()
        })
        .or_else(|| engines().find(|status| !status.models.is_empty()))
    else {
        let status = engines()
            .find(|status| matches!(status.install, artisan_api::InstallState::Installed { .. }))?;
        return Some(EngineConfig::new(status.engine));
    };
    let model = status
        .default_model
        .as_ref()
        .and_then(|id| status.models.iter().find(|model| &model.id == id))
        .or_else(|| status.models.first());
    let mut config = EngineConfig::new(status.engine);
    config.model = model.map(|model| model.id.clone());
    config.effort = None;
    Some(config)
}
