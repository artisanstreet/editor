use super::{
    attachment::{Attachments, Source},
    draft_session::Submission,
    mentions::Mentions,
    persistence::{DraftLease, DraftSessions, SessionEvent},
    picker::{Menu, PickerState},
    questionnaire::QuestionnairePanel,
};
use artisan_api::{
    BlobHash, CreateThread, EngineConfig, ItemId, ProjectId, SendMessage, SignInState, ThreadId,
};
use editor_core::{Core, HostData, HostId, HostInstance, delivery::Delivery, runtime::runtime};
use editor_ui::text::editor::{Editor, ListKey, Mode};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Clone, Copy, Debug)]
enum TurnCall {
    Stop,
    Continue,
    ClearGoal,
    Unpark,
}

impl TurnCall {
    const fn action(self) -> TurnAction {
        match self {
            Self::Stop => TurnAction::Stop,
            Self::Continue => TurnAction::Continue,
            Self::ClearGoal => TurnAction::ClearGoal,
            Self::Unpark => TurnAction::Unpark,
        }
    }
}

#[derive(Clone, Copy, Debug)]
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
    error: Option<editor_core::client::CallError>,
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

#[derive(Debug)]
enum Outbox {
    Idle,
    InFlight(Box<DraftBody>),
    Restored,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DraftBody {
    #[serde(default)]
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) images: Vec<BlobHash>,
    #[serde(default)]
    pub(crate) files: Vec<DraftFile>,
    #[serde(default)]
    pub(crate) config: Option<EngineConfig>,
    #[serde(default)]
    pub(crate) project: Option<ProjectId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) follow_ups: Vec<Self>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DraftFile {
    #[serde(skip)]
    pub key: u64,
    pub path: DraftPath,
    #[serde(default)]
    pub folder: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DraftPath(String);

#[derive(Debug, thiserror::Error)]
#[error("{} is not a Unicode path, so it cannot be linked in a message", .path.display())]
pub struct NonUnicodePath {
    path: PathBuf,
}

impl DraftPath {
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

impl TryFrom<PathBuf> for DraftPath {
    type Error = NonUnicodePath;

    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        match path.into_os_string().into_string() {
            Ok(path) => Ok(Self(path)),
            Err(path) => Err(NonUnicodePath { path: path.into() }),
        }
    }
}

fn open_draft(
    core: &Core,
    sessions: &mut DraftSessions,
    target: &ComposerTarget,
) -> (Option<DraftLease>, Option<String>) {
    let Some(host) = core.host(target.host()) else {
        return (None, None);
    };
    match sessions.open(
        host.id.clone(),
        host.instance(),
        target.scope(),
        host.client.clone(),
    ) {
        Ok(lease) => (Some(lease), None),
        Err(full) => {
            tracing::warn!(host = %full.host, scope = %full.scope, "draft capacity reached; composer disabled");
            (None, Some(full.to_string()))
        }
    }
}

#[expect(
    clippy::partial_pub_fields,
    reason = "the workspace, conversation adapter and composer tests read and seed composer state directly; narrowing that is a cross-crate redesign tracked in the lint cleanup report"
)]
pub struct Composer {
    pub target: ComposerTarget,
    pub editor: Rc<RefCell<Editor>>,
    pub questionnaire: QuestionnairePanel,
    pub(crate) strip: super::goal_strip::StripState,
    pub mentions: Mentions,
    pub picker: PickerState,
    pub config: Option<EngineConfig>,
    pub(crate) project: Option<ProjectId>,
    pub attachments: Option<Attachments>,
    pub files: Vec<DraftFile>,
    next_file_key: u64,
    placeholder_generation: u64,
    placeholder_was_visible: bool,
    pub file_shake: Option<(u64, std::time::Instant)>,
    pub(crate) cleared: Option<std::time::Instant>,
    outbox: Outbox,
    parked: Vec<DraftBody>,
    pub notice: Option<String>,
    pub session: Option<DraftLease>,
    host_instance: Option<HostInstance>,
    target_generation: u64,
    pub(crate) cancelling: bool,
    failed_stops: Vec<ThreadId>,
    pub(crate) continuing: bool,
    turn_completions: Delivery<TurnCompletion>,
    edit_revision: u64,
    restore_revision: Option<u64>,
}

impl std::fmt::Debug for Composer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Composer")
            .field("target", &self.target)
            .field("host_instance", &self.host_instance)
            .field("target_generation", &self.target_generation)
            .field("outbox", &self.outbox)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl Drop for Composer {
    fn drop(&mut self) {
        self.hand_off_attachments();
    }
}

impl Composer {
    pub fn new(core: &Core, sessions: &mut DraftSessions, target: ComposerTarget) -> Self {
        let (session, notice) = open_draft(core, sessions, &target);
        let mut editor = Editor::new("", Mode::Prompt);
        editor.set_disabled(session.is_none());
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
            cleared: None,
            outbox: Outbox::Idle,
            parked: Vec::new(),
            notice,
            session,
            host_instance,
            target_generation: 0,
            cancelling: false,
            failed_stops: Vec::new(),
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
        self.hand_off_attachments();
        if let Some(session) = &self.session {
            session.flush();
        }
        self.parked.clear();
        self.outbox = Outbox::Idle;
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
        let (session, notice) = open_draft(core, sessions, &self.target);
        if notice.is_some() {
            self.editor.borrow_mut().set_disabled(true);
        }
        self.session = session;
        self.notice = notice;
    }

    fn hand_off_attachments(&mut self) {
        let uploads = self
            .attachments
            .as_mut()
            .map_or_else(Vec::new, Attachments::take_unfinished);
        if uploads.is_empty() {
            return;
        }
        let session = self.session.as_ref().unwrap_or_else(|| {
            panic!(
                "invariant: composer for {} on Forge {} only accepts images while it has a draft session",
                self.target.scope(),
                self.target.host()
            )
        });
        for upload in uploads {
            session.adopt_image(upload);
        }
    }

    pub fn host_instance(&self) -> Option<HostInstance> {
        self.host_instance
    }

    pub(crate) fn matches_host(&self, host: &HostData) -> bool {
        self.host_instance == Some(host.instance())
    }

    pub fn generation(&self) -> u64 {
        self.target_generation
    }

    pub(crate) fn mount_editor(
        &self,
        event: impl Fn(editor_ui::text::mount::EditorEvent) + 'static,
    ) -> editor_ui::text::mount::EditorController {
        editor_ui::text::mount::EditorController::from_shared(Rc::clone(&self.editor), event)
    }

    pub fn turn_wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.turn_completions.wake_mailbox()
    }

    pub(crate) fn stop(&mut self, host: &HostData) -> Option<ThreadId> {
        self.turn_action(host, TurnCall::Stop)
    }

    pub(crate) fn take_failed_stops(&mut self) -> Vec<ThreadId> {
        std::mem::take(&mut self.failed_stops)
    }

    pub(crate) fn clear_goal(&mut self, host: &HostData) {
        self.turn_action(host, TurnCall::ClearGoal);
    }
    pub(crate) fn unpark(&mut self, host: &HostData) {
        self.turn_action(host, TurnCall::Unpark);
    }

    pub(crate) fn continue_turn(&mut self, host: &HostData) {
        self.turn_action(host, TurnCall::Continue);
    }

    fn turn_action(&mut self, host: &HostData, call: TurnCall) -> Option<ThreadId> {
        let ComposerTarget::Thread { thread, .. } = &self.target else {
            return None;
        };
        if self.host_instance != Some(host.instance()) {
            return None;
        }
        match call {
            TurnCall::Stop if self.cancelling => return None,
            TurnCall::Continue if self.continuing => return None,
            TurnCall::Stop => self.cancelling = true,
            TurnCall::Continue => self.continuing = true,
            TurnCall::ClearGoal | TurnCall::Unpark => {}
        }
        let thread = thread.clone();
        let client = host.client.clone();
        let instance = host.instance();
        let generation = self.target_generation;
        let delivery = self.turn_completions.sender();
        let issued = thread.clone();
        runtime().spawn(async move {
            let result = match call {
                TurnCall::Stop | TurnCall::Unpark => client.cancel(&thread).await,
                TurnCall::Continue => client.continue_turn(&thread).await,
                TurnCall::ClearGoal => client.clear_goal(&thread).await,
            };
            let error = result.err();
            delivery
                .offer(TurnCompletion {
                    instance,
                    generation,
                    thread,
                    action: call.action(),
                    error,
                })
                .await;
        });
        matches!(call, TurnCall::Stop).then_some(issued)
    }

    pub(crate) fn apply_permissions(&self, host: &HostData, permissions: artisan_api::Permissions) {
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
            let error = client.update_thread(&thread, &update).await.err();
            delivery
                .offer(TurnCompletion {
                    instance,
                    generation,
                    thread,
                    action: TurnAction::Permissions,
                    error,
                })
                .await;
        });
    }

    pub(crate) fn drain_turn_actions(&mut self, core: &Core) -> bool {
        let before = (self.cancelling, self.continuing, self.notice.clone());
        for completion in self.turn_completions.drain() {
            if matches!(completion.action, TurnAction::Stop) && completion.error.is_some() {
                self.failed_stops.push(completion.thread.clone());
            }
            if core.host(self.target.host()).map(HostData::instance) != Some(completion.instance)
                || self.target_generation != completion.generation
                || !matches!(&self.target, ComposerTarget::Thread { thread, .. } if *thread == completion.thread)
            {
                continue;
            }
            let failed = match completion.action {
                TurnAction::Stop => {
                    self.cancelling = false;
                    "Couldn't stop the run"
                }
                TurnAction::Continue => {
                    self.continuing = false;
                    "Couldn't continue"
                }
                TurnAction::ClearGoal => "Couldn't clear the goal",
                TurnAction::Unpark => "Couldn't cancel the pickup",
                TurnAction::Permissions => "Couldn't change access",
            };
            if let Some(error) = completion
                .error
                .as_ref()
                .and_then(|error| turn_failure(completion.action, error))
            {
                tracing::warn!(thread = %completion.thread, action = ?completion.action, %error, "turn action failed");
                self.notice = Some(format!("{failed}: {error}"));
            }
        }
        before != (self.cancelling, self.continuing, self.notice.clone())
    }

    pub(crate) fn draft_editable(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.read(|state| !state.sending()))
    }

    pub(crate) fn metadata_ready(&self, host: &HostData) -> bool {
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

    pub(crate) fn refresh_picker(&mut self, host: &HostData) {
        if self.host_instance != Some(host.instance()) {
            return;
        }
        let config = self.effective_config(host);
        self.picker.refresh(host, config.as_ref());
    }

    pub(crate) fn choose_model(
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

    pub(crate) fn choose_project(&mut self, id: ProjectId) {
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.read(super::draft_session::DraftSession::sending))
        {
            return;
        }
        self.project = Some(id);
        self.picker.menu = None;
        self.edited();
    }

    pub(crate) fn refresh_mentions(&mut self, host: &HostData) {
        if self.host_instance != Some(host.instance()) {
            return;
        }
        let engine = self.effective_config(host).map(|config| config.engine);
        let own = match &self.target {
            ComposerTarget::Thread { thread, .. } => Some(thread),
            ComposerTarget::NewThread { .. } => None,
        };
        self.mentions.refresh(host, engine, own);
        self.sync_mentions();
    }

    pub(crate) fn sync_mentions(&mut self) {
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

    pub(crate) fn mention_key(&mut self, key: ListKey) {
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
            .is_none_or(|session| session.read(super::draft_session::DraftSession::sending))
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
                            "png"
                                | "jpg"
                                | "jpeg"
                                | "gif"
                                | "bmp"
                                | "webp"
                                | "tif"
                                | "tiff"
                                | "ico"
                                | "avif"
                                | "heic"
                                | "heif"
                        )
                    });
            if image {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                self.add_image(Source::Path(path), name);
            } else {
                let path = match DraftPath::try_from(path) {
                    Ok(path) => path,
                    Err(error) => {
                        self.notice = Some(error.to_string());
                        continue;
                    }
                };
                if let Some(existing) = self.files.iter().find(|file| file.path == path) {
                    self.file_shake = Some((existing.key, std::time::Instant::now()));
                    continue;
                }
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

    pub(crate) fn remove_image(&mut self, key: u64) {
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.read(super::draft_session::DraftSession::sending))
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

    pub(crate) fn add_image(&mut self, source: Source, name: String) {
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.read(super::draft_session::DraftSession::sending))
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

    pub(crate) fn drain_attachments(
        &mut self,
        core: &Core,
        assets: &aqua::images::ImageAssets,
    ) -> bool {
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

    fn draft_body(&self) -> DraftBody {
        DraftBody {
            text: self
                .questionnaire
                .aside_text()
                .map_or_else(|| self.editor.borrow().text().to_owned(), str::to_owned),
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
            follow_ups: self.parked.clone(),
        }
    }

    pub fn save(&self) {
        let Some(session) = &self.session else {
            return;
        };
        let body = self.draft_body();
        let value = if body.text.is_empty()
            && body.images.is_empty()
            && body.files.is_empty()
            && body.config.is_none()
            && body.project.is_none()
            && body.follow_ups.is_empty()
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
        let (loaded, sending, failed, body) = session.read(|state| {
            (
                state.is_loaded(),
                state.sending(),
                state.send_failed(),
                state.body().clone(),
            )
        });
        for event in &events {
            match event {
                SessionEvent::Accepted { .. } => {
                    if matches!(self.outbox, Outbox::Restored) {
                        self.clear_input();
                        self.cleared = Some(std::time::Instant::now());
                    }
                    self.outbox = Outbox::Idle;
                    self.bring_back_follow_up();
                }
                SessionEvent::ImageAdopted(hash) => {
                    if let Some(attachments) = &mut self.attachments
                        && !attachments.contains(hash)
                    {
                        attachments.restore(hash.clone());
                        self.save();
                    }
                }
                SessionEvent::Reloaded => {
                    self.outbox = Outbox::Idle;
                    self.parked.clear();
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
        if failed
            && let Outbox::InFlight(sent) = std::mem::replace(&mut self.outbox, Outbox::Restored)
        {
            self.restore_sent(*sent);
        }
        let holding = sending && matches!(self.outbox, Outbox::Restored);
        self.editor
            .borrow_mut()
            .set_disabled(holding || self.questionnaire.sending.is_some());
        if loaded
            && self.restore_revision.take() == Some(self.edit_revision)
            && body != serde_json::Value::Null
        {
            match serde_json::from_value::<DraftBody>(body) {
                Ok(body) if self.input_empty() => self.restore_body(body),
                Ok(_) => {}
                Err(error) => self.notice = Some(format!("Couldn't read this draft: {error}")),
            }
        }
        events
    }

    fn input_empty(&self) -> bool {
        self.editor.borrow().text().trim().is_empty()
            && self.attachments.as_ref().is_none_or(|a| a.items.is_empty())
            && self.files.is_empty()
    }

    fn clear_input(&mut self) {
        self.config = None;
        self.editor.borrow_mut().set_text("");
        if let Some(attachments) = &mut self.attachments {
            attachments.clear();
        }
        self.files.clear();
        self.restore_revision = None;
    }

    fn restore_body(&mut self, body: DraftBody) {
        self.parked = body.follow_ups;
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

    fn restore_sent(&mut self, sent: DraftBody) {
        let mut parked = std::mem::take(&mut self.parked);
        if !self.input_empty() {
            parked.push(DraftBody {
                follow_ups: Vec::new(),
                ..self.draft_body()
            });
            self.notice = Some(
                "What you typed while sending is kept and comes back once this message is sent."
                    .into(),
            );
        }
        self.clear_input();
        self.restore_body(DraftBody {
            follow_ups: parked,
            ..sent
        });
        self.edited();
    }

    fn bring_back_follow_up(&mut self) {
        if self.parked.is_empty() || !self.input_empty() {
            return;
        }
        let mut parked = std::mem::take(&mut self.parked);
        let next = parked.remove(0);
        self.restore_body(DraftBody {
            follow_ups: parked,
            ..next
        });
        self.edited();
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

    pub(crate) fn effective_config(&self, host: &HostData) -> Option<EngineConfig> {
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
        let answer = self.questionnaire.writing_answer().then(|| match question {
            None => "Loading question…",
            Some(question) if question.options.is_empty() => "Type your answer, then press Enter",
            Some(_) => "Pick an answer above, or type your own",
        });
        if answer.is_none() && visible && !self.placeholder_was_visible {
            self.placeholder_generation = self.placeholder_generation.wrapping_add(1);
        }
        self.placeholder_was_visible = visible && !self.questionnaire.writing_answer();
        answer.unwrap_or_else(|| super::placeholder::phrase(self.placeholder_generation))
    }

    fn blocked(&self, host: &HostData) -> Option<&'static str> {
        if matches!(&self.target, ComposerTarget::Thread { thread, .. } if !host.threads.contains_key(thread))
        {
            return Some("Loading thread settings…");
        }
        let Some(session) = &self.session else {
            return Some("Draft unavailable");
        };
        if session.read(super::draft_session::DraftSession::sending) {
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

    pub(crate) fn primary(&self, host: &HostData) -> (super::view::Primary, bool) {
        let working = match &self.target {
            ComposerTarget::Thread { thread, .. } => host.is_working(thread),
            ComposerTarget::NewThread { .. } => false,
        };
        let ready = if self.questionnaire.writing_answer() {
            !self.editor.borrow().text().trim().is_empty()
        } else {
            self.blocked(host).is_none()
        };
        let sending = self
            .session
            .as_ref()
            .is_some_and(|session| session.read(super::draft_session::DraftSession::sending));
        let can_continue = !sending
            && matches!(&self.target, ComposerTarget::Thread { thread, .. } if !self.questionnaire.writing_answer() && self.editor.borrow().text().trim().is_empty() && self.files.is_empty() && self.attachments.as_ref().is_none_or(|items| items.items.is_empty()) && host.can_continue(thread));
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
        if session.read(super::draft_session::DraftSession::sending)
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
        self.outbox = Outbox::InFlight(Box::new(self.draft_body()));
        self.clear_input();
        self.cleared = Some(std::time::Instant::now());
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
            let path = file.path.as_path();
            let name = path.file_name().map_or_else(
                || file.path.0.clone(),
                |name| name.to_string_lossy().into_owned(),
            );
            markdown_link(&format!("{name}{slash}"), &file.path.0)
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        links
    } else {
        format!("{text}\n\n{links}")
    }
}

fn markdown_link(label: &str, destination: &str) -> String {
    let mut link = String::from("[");
    for character in label.chars() {
        match character {
            '\n' | '\r' => link.push(' '),
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '~' | '|' | '&' | '!' => {
                link.push('\\');
                link.push(character);
            }
            other => link.push(other),
        }
    }
    link.push_str("](<");
    let mut characters = destination.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\n' => link.push_str("%0A"),
            '\r' => link.push_str("%0D"),
            '<' | '>' => {
                link.push('\\');
                link.push(character);
            }
            '\\' if characters.peek().is_some_and(char::is_ascii_punctuation) => {
                link.push_str("\\\\");
            }
            other => link.push(other),
        }
    }
    link.push_str(">)");
    link
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
    let mut config = host.settings.default_engine_config()?.clone();
    if let Some(saved) = config
        .model
        .as_deref()
        .and_then(|model| host.settings.saved_model_config(config.engine, model))
    {
        config = saved.clone();
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

fn turn_failure(action: TurnAction, error: &editor_core::client::CallError) -> Option<String> {
    use artisan_api::{Failure, RunFailure};
    match (action, error.refused()) {
        (
            TurnAction::Continue,
            Some(Failure::Run(
                RunFailure::AlreadyWorking { .. } | RunFailure::NothingToContinue { .. },
            )),
        )
        | (TurnAction::ClearGoal, Some(Failure::Run(RunFailure::NoGoal { .. }))) => None,
        (
            _,
            Some(
                failure @ (Failure::Thread(_)
                | Failure::Run(_)
                | Failure::Draft(_)
                | Failure::Setting(_)
                | Failure::Project(_)
                | Failure::Mcp(_)
                | Failure::Agent(_)
                | Failure::Auth(_)
                | Failure::Resource(_)
                | Failure::Extension(_)
                | Failure::Engine(_)),
            ),
        ) => Some(failure.to_string()),
        (_, Some(Failure::Internal | Failure::Legacy { .. }) | None) => {
            Some(diagnostics::chain(error))
        }
    }
}

#[cfg(test)]
#[path = "../../tests/composer/controller.rs"]
mod tests;
