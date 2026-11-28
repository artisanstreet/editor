use super::{
    attachment::{Attachments, PendingUpload, Source},
    draft_body::{DraftBody, DraftPath, SavedFile},
    draft_session::{DraftSession, LostKey, Submission, Unrecovered},
    mentions::Mentions,
    model_switch::{Compaction, SwitchChoice, SwitchPrompt},
    persistence::{DraftLease, DraftSessions, SessionEvent},
    picker::{Menu, PickerState},
    questionnaire::QuestionnairePanel,
};
use artisan_api::{
    BlobHash, CreateThread, EngineConfig, ItemId, ProjectId, SendMessage, SignInState, ThreadId,
};
use editor_core::{
    Core, HostData, HostId, HostInstance, LocalSend, NewThread, delivery::Delivery,
    runtime::runtime,
};
use editor_ui::text::editor::{Editor, ListKey, Mode};
use std::{cell::RefCell, num::NonZeroUsize, path::PathBuf, rc::Rc};

#[derive(Clone, Copy, Debug)]
enum TurnCall {
    Stop,
    Continue,
    ClearGoal,
    Unpark,
    Compact,
}

impl TurnCall {
    const fn action(self) -> TurnAction {
        match self {
            Self::Stop => TurnAction::Stop,
            Self::Continue => TurnAction::Continue,
            Self::ClearGoal => TurnAction::ClearGoal,
            Self::Unpark => TurnAction::Unpark,
            Self::Compact => TurnAction::Compact,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum TurnAction {
    Stop,
    Continue,
    ClearGoal,
    Unpark,
    Compact,
    Permissions,
}
struct TurnCompletion {
    instance: HostInstance,
    generation: u64,
    thread: ThreadId,
    action: TurnAction,
    error: Option<TurnFailure>,
}

enum TurnFailure {
    Call(editor_core::client::CallError),
    Edit(editor_core::EditThreadError),
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

#[derive(Clone, Debug)]
pub struct DraftFile {
    pub key: u64,
    pub path: DraftPath,
    pub folder: bool,
}

fn open_draft(
    core: &Core,
    sessions: &mut DraftSessions,
    target: &ComposerTarget,
) -> (Option<DraftLease>, Option<editor_core::Problem>) {
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
            (None, Some(editor_core::Problem::new(full.to_string())))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SendState {
    Idle,
    AwaitingImages,
}

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
    held_images: Vec<(Source, String)>,
    pub files: Vec<DraftFile>,
    next_file_key: u64,
    placeholder_generation: u64,
    placeholder_was_visible: bool,
    pub file_shake: Option<(u64, std::time::Instant)>,
    pub(crate) cleared: Option<std::time::Instant>,
    parked: Vec<DraftBody>,
    pub notice: Option<editor_core::Problem>,
    pub(crate) switch_prompt: Option<SwitchPrompt>,
    pub refused: Option<SendRefusal>,
    send_state: SendState,
    pub session: Option<DraftLease>,
    host_instance: Option<HostInstance>,
    target_generation: u64,
    pub(crate) cancelling: bool,
    failed_stops: Vec<ThreadId>,
    pub(crate) continuing: bool,
    turn_completions: Delivery<TurnCompletion>,
    edit_revision: u64,
    restore_revision: Option<u64>,
    shown_epoch: u64,
    lost_chips: Vec<(u64, LostKey)>,
}

impl std::fmt::Debug for Composer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Composer")
            .field("target", &self.target)
            .field("host_instance", &self.host_instance)
            .field("target_generation", &self.target_generation)
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
        editor.set_disabled(
            session
                .as_ref()
                .is_none_or(|session| session.read(DraftSession::recovering)),
        );
        let host_instance = core.host(target.host()).map(HostData::instance);
        let attachments = core
            .host(target.host())
            .map(|host| Attachments::new(host.client.clone()));
        let picker = PickerState::for_target(core, target.host());
        let shown_epoch = session
            .as_ref()
            .map_or(0, |session| session.read(DraftSession::epoch));
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
            held_images: Vec::new(),
            files: Vec::new(),
            next_file_key: 0,
            placeholder_generation: 0,
            placeholder_was_visible: false,
            file_shake: None,
            cleared: None,
            parked: Vec::new(),
            notice,
            switch_prompt: None,
            refused: None,
            send_state: SendState::Idle,
            session,
            host_instance,
            target_generation: 0,
            cancelling: false,
            failed_stops: Vec::new(),
            continuing: false,
            turn_completions: Delivery::new(NonZeroUsize::MIN),
            edit_revision: 0,
            restore_revision: Some(0),
            shown_epoch,
            lost_chips: Vec::new(),
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
        self.switch_prompt = None;
        self.refused = None;
        self.send_state = SendState::Idle;
        self.config = None;
        self.project = None;
        self.host_instance = core.host(self.target.host()).map(HostData::instance);
        self.attachments = core
            .host(self.target.host())
            .map(|host| Attachments::new(host.client.clone()));
        let (session, notice) = open_draft(core, sessions, &self.target);
        if notice.is_some()
            || session
                .as_ref()
                .is_some_and(|session| session.read(DraftSession::recovering))
        {
            self.editor.borrow_mut().set_disabled(true);
        }
        self.shown_epoch = session
            .as_ref()
            .map_or(0, |session| session.read(DraftSession::epoch));
        self.lost_chips.clear();
        self.session = session;
        self.notice = notice;
    }

    fn hand_off_attachments(&mut self) {
        let mut uploads: Vec<PendingUpload> = std::mem::take(&mut self.held_images)
            .into_iter()
            .map(|(source, name)| PendingUpload::held(source, name))
            .collect();
        uploads.extend(
            self.attachments
                .as_mut()
                .map_or_else(Vec::new, Attachments::take_unfinished),
        );
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
            TurnCall::ClearGoal | TurnCall::Unpark | TurnCall::Compact => {}
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
                TurnCall::Compact => client
                    .send_message(
                        &thread,
                        &SendMessage {
                            id: ItemId::new(),
                            text: "/compact".to_owned(),
                            images: Vec::new(),
                            config: None,
                            draft: None,
                        },
                    )
                    .await
                    .map(drop),
            };
            let error = result.err().map(TurnFailure::Call);
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

    pub(crate) fn apply_permissions(
        &self,
        host: &mut HostData,
        permissions: artisan_api::Permissions,
    ) {
        let ComposerTarget::Thread { thread, .. } = &self.target else {
            return;
        };
        if !self.matches_host(host)
            || host
                .threads
                .get(thread)
                .is_none_or(|known| known.config.permissions == permissions)
        {
            return;
        }
        let instance = host.instance();
        let generation = self.target_generation;
        let delivery = self.turn_completions.sender();
        let completed = thread.clone();
        host.edit_thread(
            thread,
            editor_core::ThreadEdit::Permissions(permissions),
            Box::new(move |result| {
                Box::pin(async move {
                    delivery
                        .offer(TurnCompletion {
                            instance,
                            generation,
                            thread: completed,
                            action: TurnAction::Permissions,
                            error: result.err().map(TurnFailure::Edit),
                        })
                        .await;
                })
            }),
        );
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
            match completion.action {
                TurnAction::Stop => self.cancelling = false,
                TurnAction::Continue => self.continuing = false,
                TurnAction::ClearGoal
                | TurnAction::Unpark
                | TurnAction::Compact
                | TurnAction::Permissions => {}
            }
            match &completion.error {
                Some(TurnFailure::Call(error)) => {
                    if let Some(problem) = turn_problem(completion.action, error) {
                        tracing::warn!(thread = %completion.thread, action = ?completion.action, error = %diagnostics::chain(error), "turn action failed");
                        self.notice = Some(problem);
                    }
                }
                Some(TurnFailure::Edit(error)) => {
                    tracing::warn!(thread = %completion.thread, action = ?completion.action, error = %diagnostics::chain(error), "turn action failed");
                    self.notice = Some(error.problem());
                    if let editor_core::ThreadEdit::Permissions(refused) = error.edit() {
                        self.restore_permissions(core, &completion.thread, *refused);
                    }
                }
                None => {}
            }
        }
        before != (self.cancelling, self.continuing, self.notice.clone())
    }

    fn restore_permissions(
        &mut self,
        core: &Core,
        thread: &ThreadId,
        refused: artisan_api::Permissions,
    ) {
        let shown = core
            .host(self.target.host())
            .and_then(|host| host.threads.get(thread))
            .map(|known| known.config.permissions);
        if let (Some(config), Some(shown)) = (&mut self.config, shown)
            && config.permissions == refused
        {
            config.permissions = shown;
        }
    }

    pub(crate) fn draft_editable(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.read(|state| !state.sending() && !state.recovering()))
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
        let prompt = match &self.target {
            ComposerTarget::Thread { thread, .. } => host.runs.get(thread).and_then(|run| {
                super::model_switch::prompt(run, host.engines.get(&run.engine), config.clone())
            }),
            ComposerTarget::NewThread { .. } => None,
        };
        if let Some(prompt) = prompt {
            self.picker.menu = None;
            self.switch_prompt = Some(prompt);
            return None;
        }
        self.config = Some(config.clone());
        self.edited();
        Some(config)
    }

    pub(crate) fn resolve_switch(
        &mut self,
        host: &HostData,
        choice: SwitchChoice,
    ) -> Option<EngineConfig> {
        let prompt = self.switch_prompt.take()?;
        if !self.matches_host(host) || !self.draft_editable() {
            return None;
        }
        match (choice, prompt.compaction) {
            (SwitchChoice::Cancel, _) => return None,
            (SwitchChoice::CompactFirst, Compaction::Available) => {
                self.turn_action(host, TurnCall::Compact);
            }
            (SwitchChoice::CompactFirst, Compaction::BusyRun) | (SwitchChoice::SwitchAnyway, _) => {
            }
        }
        self.config = Some(prompt.config.clone());
        self.edited();
        Some(prompt.config)
    }

    pub(crate) fn choose_project(&mut self, id: ProjectId) {
        if self
            .session
            .as_ref()
            .is_none_or(|session| session.read(DraftSession::sending))
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
            .is_none_or(|session| session.read(DraftSession::sending))
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
                        self.notice = Some(editor_core::Problem::caused_by(
                            "This path can't be linked.",
                            &error,
                        ));
                        continue;
                    }
                };
                if let Some(existing) = self.files.iter().find(|file| file.path == path) {
                    self.file_shake = Some((existing.key, std::time::Instant::now()));
                    continue;
                }
                if self.files.len() >= 32 {
                    self.notice = Some(editor_core::Problem::new(
                        "A message can link at most 32 files.",
                    ));
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
        if !self.accepts_attachments() {
            return;
        }
        if let Some(index) = self.lost_chips.iter().position(|(chip, _)| *chip == key) {
            let (_, lost) = self.lost_chips.remove(index);
            if let Some(session) = &self.session {
                session.dismiss_lost(lost);
            }
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
        if self.draft_busy() {
            self.held_images.push((source, name));
            return;
        }
        if !self.accepts_attachments() {
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
        self.send_state = SendState::Idle;
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
            files: self
                .files
                .iter()
                .map(|file| SavedFile {
                    path: file.path.clone(),
                    folder: file.folder,
                })
                .collect(),
            config: self.config.clone(),
            project: self.project.clone(),
            follow_ups: self.parked.clone(),
        }
    }

    pub fn save(&self) {
        let Some(session) = &self.session else {
            return;
        };
        let uploading = self
            .attachments
            .as_ref()
            .is_some_and(Attachments::uploading);
        session.queue(self.draft_body(), self.shown_epoch, uploading);
    }

    pub fn release_if_host_gone(&mut self, core: &Core) -> bool {
        if core.host(self.target.host()).map(HostData::instance) == self.host_instance {
            return false;
        }
        self.release();
        true
    }

    pub fn release(&mut self) {
        self.hand_off_attachments();
        self.session = None;
        self.editor.borrow_mut().set_disabled(true);
    }

    pub fn sync(&mut self, core: &Core) -> Vec<SessionEvent> {
        if self.release_if_host_gone(core) {
            return Vec::new();
        }
        let Some(session) = &mut self.session else {
            return Vec::new();
        };
        let events = session.drain_events();
        let lost = session.read(|state| state.lost().to_vec());
        let unrecovered = session.take_unrecovered();
        let recovering = session.read(DraftSession::recovering);
        self.reconcile();
        self.settle_refusal(core);
        for event in &events {
            match event {
                SessionEvent::Accepted { .. }
                | SessionEvent::HandedBack { .. }
                | SessionEvent::Changed => {}
                SessionEvent::ImageAdopted(hash) => {
                    if self.restore_revision != Some(self.edit_revision)
                        && let Some(attachments) = &mut self.attachments
                        && !attachments.contains(hash)
                    {
                        attachments.restore(hash.clone());
                    }
                }
            }
        }
        self.editor
            .borrow_mut()
            .set_disabled(self.questionnaire.sending.is_some() || recovering);
        if !unrecovered.is_empty() {
            self.notice = Some(editor_core::Problem::new(
                unrecovered
                    .iter()
                    .map(|unrecovered| match unrecovered {
                        Unrecovered::SetAside(path) => format!(
                            "A draft kept on this device couldn't be read. It was moved to {}.",
                            path.display()
                        ),
                        Unrecovered::Left(path) => format!(
                            "A draft kept on this device couldn't be read. It stays at {}, and this draft is kept beside it.",
                            path.display()
                        ),
                        Unrecovered::Unchecked(path) => format!(
                            "Artisan couldn't check {} for a draft kept on this device. Open this draft again to check once more.",
                            path.display()
                        ),
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            ));
        }
        self.show_lost(lost);
        self.add_held_images();
        events
    }

    pub(crate) fn reconcile(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let (epoch, shown, parked, unreadable) = session.read(|state| {
            (
                state.epoch(),
                state.shown().cloned(),
                state.parked_beside_failed_send(),
                state.unreadable().map(ToString::to_string),
            )
        });
        if epoch != self.shown_epoch {
            self.shown_epoch = epoch;
            self.reset_input();
        }
        if self.restore_revision == Some(self.edit_revision)
            && let Some(body) = shown
        {
            self.restore_revision = None;
            if let Some(error) = unreadable {
                self.notice = Some(editor_core::Problem::detailed(
                    "Couldn't read this draft.",
                    error,
                ));
            } else if parked {
                self.notice = Some(editor_core::Problem::new(
                    "What you typed while sending is kept and comes back once this message is sent.",
                ));
            }
            self.restore_body(body);
        }
    }

    fn draft_busy(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.read(|state| state.sending() || state.recovering()))
    }

    fn add_held_images(&mut self) {
        if self.held_images.is_empty() || self.draft_busy() {
            return;
        }
        for (source, name) in std::mem::take(&mut self.held_images) {
            self.add_image(source, name);
        }
    }

    fn accepts_attachments(&mut self) -> bool {
        let Some((busy, epoch)) = self.session.as_ref().map(|session| {
            session.read(|state| (state.sending() || state.recovering(), state.epoch()))
        }) else {
            return false;
        };
        if busy {
            return false;
        }
        if epoch != self.shown_epoch {
            self.reconcile();
        }
        true
    }

    fn show_lost(&mut self, lost: Vec<super::draft_session::LostImage>) {
        let Some(attachments) = &mut self.attachments else {
            unreachable!(
                "invariant: composer for {} on Forge {} has its Forge's attachments while it has a draft session",
                self.target.scope(),
                self.target.host()
            );
        };
        self.lost_chips.retain(|(chip, key)| {
            let kept = lost.iter().any(|lost| lost.key == *key);
            if !kept {
                attachments.remove(*chip);
            }
            kept
        });
        for image in lost {
            if !self.lost_chips.iter().any(|(_, key)| *key == image.key) {
                let chip = attachments.add_failed(image.name, image.reason);
                self.lost_chips.push((chip, image.key));
            }
        }
    }

    fn reset_input(&mut self) {
        self.parked.clear();
        self.editor.borrow_mut().set_text("");
        if let Some(attachments) = &mut self.attachments {
            attachments.clear();
        }
        self.lost_chips.clear();
        self.files.clear();
        self.restore_revision = Some(self.edit_revision);
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
            .map(|file| {
                let key = self.next_file_key;
                self.next_file_key = self.next_file_key.wrapping_add(1);
                DraftFile {
                    key,
                    path: file.path,
                    folder: file.folder,
                }
            })
            .collect();
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
        if session.read(DraftSession::choosing_draft) {
            return Some("Choose which draft to keep first.");
        }
        if session.read(DraftSession::sending) {
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
            .is_some_and(|session| session.read(DraftSession::sending));
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

    fn settle_refusal(&mut self, core: &Core) {
        let Some(refused) = self.refused else {
            return;
        };
        let standing = core
            .host(self.target.host())
            .and_then(|host| self.readiness(host).err());
        if standing != Some(refused) {
            self.refused = None;
        }
    }

    pub(crate) fn refuse(&mut self, refusal: SendRefusal) {
        self.send_state = match refusal {
            SendRefusal::ImageUploading => SendState::AwaitingImages,
            SendRefusal::Empty
            | SendRefusal::NoDraft
            | SendRefusal::ForgeReconnected
            | SendRefusal::DraftChanged
            | SendRefusal::AnsweringQuestion
            | SendRefusal::ThreadMissing
            | SendRefusal::AlreadySending
            | SendRefusal::ChoosingDraft
            | SendRefusal::DraftLoading
            | SendRefusal::ImageFailed
            | SendRefusal::NoModel
            | SendRefusal::NoProject => SendState::Idle,
        };
        self.refused = match refusal {
            SendRefusal::Empty | SendRefusal::ImageUploading => None,
            refusal @ (SendRefusal::NoDraft
            | SendRefusal::ForgeReconnected
            | SendRefusal::DraftChanged
            | SendRefusal::AnsweringQuestion
            | SendRefusal::ThreadMissing
            | SendRefusal::AlreadySending
            | SendRefusal::ChoosingDraft
            | SendRefusal::DraftLoading
            | SendRefusal::ImageFailed
            | SendRefusal::NoModel
            | SendRefusal::NoProject) => Some(refusal),
        };
    }

    fn readiness(&self, host: &HostData) -> Result<(Vec<BlobHash>, EngineConfig), SendRefusal> {
        let Some(session) = &self.session else {
            return Err(SendRefusal::NoDraft);
        };
        if self.host_instance != Some(host.instance()) {
            return Err(SendRefusal::ForgeReconnected);
        }
        if session.read(DraftSession::epoch) != self.shown_epoch {
            return Err(SendRefusal::DraftChanged);
        }
        if self.questionnaire.writing_answer() {
            return Err(SendRefusal::AnsweringQuestion);
        }
        if matches!(&self.target, ComposerTarget::Thread { thread, .. } if !host.threads.contains_key(thread))
        {
            return Err(SendRefusal::ThreadMissing);
        }
        if session.read(DraftSession::choosing_draft) {
            return Err(SendRefusal::ChoosingDraft);
        }
        if session.read(DraftSession::sending) {
            return Err(SendRefusal::AlreadySending);
        }
        if !session.read(DraftSession::sendable) {
            return Err(SendRefusal::DraftLoading);
        }
        if self.editor.borrow().text().trim().is_empty()
            && self.attachments.as_ref().is_none_or(|a| a.items.is_empty())
            && self.files.is_empty()
        {
            return Err(SendRefusal::Empty);
        }
        let images = match &self.attachments {
            None => Vec::new(),
            Some(attachments) => match attachments.ready_hashes() {
                Some(images) => images,
                None if attachments.uploading() => return Err(SendRefusal::ImageUploading),
                None => return Err(SendRefusal::ImageFailed),
            },
        };
        let config = self.effective_config(host).ok_or(SendRefusal::NoModel)?;
        if matches!(self.target, ComposerTarget::NewThread { .. }) && self.project(host).is_none() {
            return Err(SendRefusal::NoProject);
        }
        Ok((images, config))
    }

    pub(crate) fn images_landed_for_waiting_send(&mut self) -> bool {
        if self.send_state != SendState::AwaitingImages
            || self
                .attachments
                .as_ref()
                .is_some_and(Attachments::uploading)
        {
            return false;
        }
        self.send_state = SendState::Idle;
        self.attachments
            .as_ref()
            .is_none_or(|attachments| attachments.ready_hashes().is_some())
    }

    pub fn send(&mut self, host: &HostData) -> Result<LocalSend, SendRefusal> {
        let (images, config) = self.readiness(host)?;
        self.refused = None;
        self.send_state = SendState::Idle;
        let (create, thread_config, settling, thread) = match &self.target {
            ComposerTarget::Thread { thread, .. } => (
                None,
                host.confirmed_thread(thread)
                    .map(|thread| thread.config.clone()),
                host.permissions_changing(thread),
                thread.clone(),
            ),
            ComposerTarget::NewThread { .. } => {
                let project_id = self.project(host).ok_or(SendRefusal::NoProject)?;
                let thread = ThreadId::new();
                (
                    Some(CreateThread {
                        id: thread.clone(),
                        project_id,
                        config: config.clone(),
                        title: None,
                        parent_thread_id: None,
                        spawned_by: None,
                    }),
                    Some(config.clone()),
                    false,
                    thread,
                )
            }
        };
        let message = SendMessage {
            id: ItemId::new(),
            text: file_links(self.editor.borrow().text().trim(), &self.files),
            images,
            config: (settling || thread_config.as_ref() != Some(&config)).then_some(config),
            draft: None,
        };
        self.save();
        let session = self.session.as_ref().ok_or(SendRefusal::NoDraft)?;
        let gate = host.post_gate(&thread);
        let receipt = host.send_receipt();
        let attempt = receipt.attempt();
        let accepted = session.submit(
            Submission {
                message,
                create,
                thread,
            },
            gate,
            Some(receipt),
        )?;
        let sent = local_send(&accepted, attempt);
        self.clear_input();
        self.cleared = Some(std::time::Instant::now());
        Ok(sent)
    }

    #[must_use]
    pub fn creating(&self) -> Option<ThreadId> {
        self.session
            .as_ref()?
            .read(|state| state.creating().cloned())
    }
}

fn local_send(submission: &Submission, attempt: editor_core::SendAttempt) -> LocalSend {
    LocalSend {
        attempt,
        thread: submission.thread.clone(),
        message: submission.message.id.clone(),
        text: submission.message.text.clone(),
        images: submission.message.images.clone(),
        create: submission.create.as_ref().map(|create| NewThread {
            project: create.project_id.clone(),
            config: create.config.clone(),
        }),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SendRefusal {
    #[error("This message's draft isn't open. Open the thread again, then send it.")]
    NoDraft,
    #[error("This Forge reconnected since the message was written. Try again in a moment.")]
    ForgeReconnected,
    #[error("This draft changed since it was shown. Check it, then send it again.")]
    DraftChanged,
    #[error("A question is being answered. Send the answer first.")]
    AnsweringQuestion,
    #[error("This thread isn't on this Forge.")]
    ThreadMissing,
    #[error("This message is already being sent.")]
    AlreadySending,
    #[error("Choose which draft to keep first.")]
    ChoosingDraft,
    #[error("This draft is still loading. Try again once it has loaded.")]
    DraftLoading,
    #[error("This message has nothing to send. Type it again, then send it.")]
    Empty,
    #[error("This message is sent once its images are ready.")]
    ImageUploading,
    #[error("An image couldn't be uploaded. Remove it or try it again.")]
    ImageFailed,
    #[error("Choose a model, then send it again.")]
    NoModel,
    #[error("Choose a project, then send it again.")]
    NoProject,
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
                || file.path.as_str().to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
            markdown_link(&format!("{name}{slash}"), file.path.as_str())
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

fn turn_problem(
    action: TurnAction,
    error: &editor_core::client::CallError,
) -> Option<editor_core::Problem> {
    use artisan_api::{Failure, RunFailure};
    let headline = match (action, error.refused()) {
        (
            TurnAction::Continue,
            Some(Failure::Run(
                RunFailure::AlreadyWorking { .. } | RunFailure::NothingToContinue { .. },
            )),
        )
        | (TurnAction::ClearGoal, Some(Failure::Run(RunFailure::NoGoal { .. }))) => return None,
        (TurnAction::Stop, _) => "Couldn't stop the run.",
        (TurnAction::Continue, _) => "Couldn't continue.",
        (TurnAction::ClearGoal, _) => "Couldn't clear the goal.",
        (TurnAction::Unpark, _) => "Couldn't cancel the pickup.",
        (TurnAction::Compact, _) => "Couldn't compact the conversation.",
        (TurnAction::Permissions, _) => "Couldn't change access.",
    };
    Some(editor_core::Problem::calling_forge(headline, error))
}

#[cfg(test)]
#[path = "../../tests/composer/controller.rs"]
mod tests;
