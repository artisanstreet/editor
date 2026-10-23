//! The composer: one draft, its attachments and engine settings, and the
//! send button that starts a run or steers the live one.
//!
//! The draft lives in Forge (`/drafts/{scope}`), saved a moment after each
//! edit. Sending posts a client-made message id; the draft is cleared only
//! once Forge accepted it. While the thread's run is live the same send is a
//! steer: Forge decides, the composer only says so on the button.

mod attachment;
mod draft_session;
mod encode;
mod goal_strip;
mod mentions;
mod picker;
mod placeholder;
mod questionnaire;

pub use goal_strip::{ComposerGoal, ComposerGoalState};

/// The latest turn's note ("You should know", "Heads up") for the strip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerNote {
    /// The note's item: closing it closes this one, not the next.
    pub id: SharedString,
    pub heads_up: bool,
    /// The note, on one line.
    pub text: SharedString,
}
pub use questionnaire::ComposerQuestionnaire;

use editor_ui::text_editor::{PastedFiles, TextEditor, TextEditorEvent};
use editor_ui::theme::{ActiveTheme as _, ArtisanTheme, TextStep, TypographyTokens};

use anyhow::Context as _;
use artisan_api::{
    BlobHash, CreateThread, EngineConfig, EngineId, ItemId, Permissions, ProjectId, SendMessage,
    SignInState, ThreadId, UpdateThread,
};
use draft_session::{DraftSession, SessionEvent, Submission};
use editor_core::{Core, DraggedThread, HostData, HostEvent, HostId, Screen};
use editor_ui::AssetId;
use editor_ui::button::{Button, ButtonShape, ButtonVariant};
use editor_ui::glass::ComposerFrame;
use editor_ui::tab_card::{TabCard, TabCardStrip};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, RenderImage, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, px,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// What the composer sends to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerTarget {
    Thread {
        host: HostId,
        thread: ThreadId,
    },
    /// Sending creates the thread first.
    NewThread {
        host: HostId,
        project: Option<ProjectId>,
    },
}

impl ComposerTarget {
    const fn host(&self) -> &HostId {
        match self {
            Self::Thread { host, .. } | Self::NewThread { host, .. } => host,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ComposerEvent {
    Sent {
        thread: ThreadId,
        item: ItemId,
    },
    /// The project menu's "New project" row: the shell adds a project.
    NewProject,
}

/// Forge persists this default when it accepts a new message. Settings uses
/// the same value when choosing the configuration for a new thread.
const DEFAULT_CONFIG_KEY: &str = "default_engine_config";

/// What `/drafts/{scope}` holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct DraftBody {
    #[serde(default)]
    text: String,
    #[serde(default)]
    images: Vec<BlobHash>,
    /// Dropped files and folders, sent as markdown links.
    #[serde(default)]
    files: Vec<DraftFile>,
    #[serde(default)]
    config: Option<EngineConfig>,
    /// Choosing another project carries this draft; its original slot stays stable.
    #[serde(default)]
    project: Option<ProjectId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DraftFile {
    path: PathBuf,
    #[serde(default)]
    folder: bool,
}

/// A dropped file or folder. The engine gets a markdown link to its path
/// after the message text; nothing is uploaded.
pub(crate) struct FileAttachment {
    key: u64,
    name: SharedString,
    path: PathBuf,
    folder: bool,
}

pub(crate) struct Attachment {
    key: u64,
    name: SharedString,
    state: attachment::State,
    /// Decoded pixels shared by the tile and the full-window preview.
    preview: Option<Arc<RenderImage>>,
    /// Encoded bytes move into the upload once conversion finishes.
    encoded: Option<Vec<u8>>,
    preview_error: Option<String>,
    /// When the attachment became ready: it sharpens out of the blur from then.
    revealed: Option<Instant>,
    /// Hidden until the blurred still decodes, so the tile never swaps
    /// an empty face for a picture.
    shown: bool,
    /// Hash and length of the picture as given, to spot it pasted twice.
    source: Option<(u64, usize)>,
    task: Task<()>,
}

/// What the composer's primary button does now.
#[derive(Clone, Copy)]
enum Primary {
    Send,
    Stop,
    Continue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Model,
    Project,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent view flags (requests in flight, text, strip and placeholder motion)"
)]
pub struct Composer {
    core: Entity<Core>,
    target: ComposerTarget,
    target_generation: u64,
    edit_revision: u64,
    cancelling: bool,
    /// A continue request is on its way to Forge.
    continuing: bool,
    #[cfg(test)]
    cancel_completion_gate: Option<(
        Arc<std::sync::atomic::AtomicBool>,
        futures::channel::oneshot::Receiver<bool>,
    )>,
    #[cfg(test)]
    cancel_requests: Vec<ThreadId>,
    #[cfg(test)]
    cancel_completions: usize,
    editor: Entity<TextEditor>,
    has_text: bool,
    attachments: Vec<Attachment>,
    files: Vec<FileAttachment>,
    next_key: u64,
    /// The attachment a duplicate paste or drop shook, and when; every
    /// repeat restarts it.
    shake: Option<(u64, Instant)>,
    session: Option<Entity<DraftSession>>,
    session_lease: Option<std::rc::Rc<()>>,
    session_subscription: Option<Subscription>,
    restore_revision: Option<u64>,
    /// Failures outside sending (stop, attachments read from disk).
    notice: Option<String>,
    /// The user's engine choice; `None` follows the thread.
    config: Option<EngineConfig>,
    /// Chosen for a new thread whose target has no project.
    project: Option<ProjectId>,
    menu: Option<Menu>,
    /// The engine whose models the open model menu shows.
    browsing: Option<EngineId>,
    /// The active model and project choices.
    picker: picker::PickerState,
    axis: Option<picker::Axis>,
    viewport_width: f32,
    viewport_height: f32,
    /// The thread's goal, on the strip along the card's top edge.
    goal: goal_strip::GoalStrip,
    /// The goal the thread has, shown when no note takes its place.
    thread_goal: Option<ComposerGoal>,
    /// The latest turn's note, which the strip shows ahead of the goal.
    note: Option<ComposerNote>,
    /// The note the reader closed; it stays closed.
    closed_note: Option<SharedString>,
    /// When Forge picks the thread's parked turn up again; the strip says
    /// so ahead of everything else.
    parked_until: Option<artisan_api::Timestamp>,
    /// The open thread's goal and note have arrived since the target
    /// changed. Until then the strip is placed, never grown: it was already
    /// there before the reader came.
    strip_settled: bool,
    /// Which placeholder phrase shows: the next one each time the draft
    /// empties again.
    placeholder_generation: u64,
    placeholder_was_visible: bool,
    /// Skills and connectors named in the draft, and their menu.
    mentions: mentions::Mentions,
    /// The agent's open questions, answered from the card.
    questionnaire: questionnaire::QuestionnairePanel,

    host_subscription: Option<Subscription>,
    _editor_subscriptions: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Focusable for Composer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Composer {
    pub fn new(
        core: Entity<Core>,
        target: ComposerTarget,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(Self::clear_attachments).detach();
        let editor = cx.new(|cx| TextEditor::new("", placeholder::phrase(0), cx).prompt());
        let editor_subscriptions = vec![
            cx.subscribe(&editor, Self::on_editor_event),
            cx.subscribe(&editor, |this, _, pasted: &PastedFiles, cx| {
                for image in &pasted.images {
                    this.add_image(
                        Source::Bytes(image.bytes.clone()),
                        "Pasted image".into(),
                        cx,
                    );
                }
                this.add_paths(pasted.paths.clone(), cx);
            }),
            cx.subscribe(&editor, Self::on_list_key),
            // The menu follows the caret, which moves without an edit.
            cx.observe(&editor, |this, _, cx| this.sync_mention_menu(cx)),
        ];
        let mut composer = Self {
            core,
            target,
            target_generation: 0,
            edit_revision: 0,
            cancelling: false,
            continuing: false,
            #[cfg(test)]
            cancel_completion_gate: None,
            #[cfg(test)]
            cancel_requests: Vec::new(),
            #[cfg(test)]
            cancel_completions: 0,
            editor,
            has_text: false,
            attachments: Vec::new(),
            files: Vec::new(),
            next_key: 0,
            shake: None,
            session: None,
            session_lease: None,
            session_subscription: None,
            restore_revision: Some(0),
            notice: None,
            config: None,
            project: None,
            menu: None,
            browsing: None,
            picker: picker::PickerState::default(),
            axis: None,
            viewport_width: 1280.0,
            viewport_height: 800.0,
            goal: goal_strip::GoalStrip::default(),
            thread_goal: None,
            parked_until: None,
            note: None,
            closed_note: None,
            strip_settled: false,
            placeholder_generation: 0,
            placeholder_was_visible: false,
            mentions: mentions::Mentions::default(),
            questionnaire: questionnaire::QuestionnairePanel::default(),
            host_subscription: None,
            _editor_subscriptions: editor_subscriptions,
        };
        composer.open_target(cx);
        composer
    }

    /// Shows the thread's goal on the strip along the card's top edge, or
    /// retracts it.
    pub fn set_goal(&mut self, goal: Option<ComposerGoal>, cx: &mut Context<Self>) {
        self.thread_goal = goal;
        self.show_strip(cx);
    }

    /// Shows the latest turn's note on the strip, ahead of the goal, or
    /// takes it away (`None` once a new turn starts).
    pub fn set_note(&mut self, note: Option<ComposerNote>, cx: &mut Context<Self>) {
        self.note = note;
        self.show_strip(cx);
    }

    /// Shows when Forge picks the thread's parked turn up again, ahead of
    /// the note and goal, or takes it away once the turn goes.
    pub fn set_parked(&mut self, until: Option<artisan_api::Timestamp>, cx: &mut Context<Self>) {
        if self.parked_until != until {
            self.parked_until = until;
            self.show_strip(cx);
        }
    }

    /// The thread's goal and note as they stand when it opens have arrived:
    /// from now on the strip moves when they change.
    pub const fn settle_strip(&mut self) {
        self.strip_settled = true;
    }

    /// The reader closed the note: the goal, if any, comes back.
    fn close_note(&mut self, cx: &mut Context<Self>) {
        self.closed_note = self.note.as_ref().map(|note| note.id.clone());
        self.show_strip(cx);
    }

    /// The strip shows a parked turn's pickup, else the open note, else
    /// the goal.
    fn show_strip(&mut self, cx: &mut Context<Self>) {
        let parked = self.parked_until.map(|at| ComposerGoal {
            objective: format!(
                "This thread will pick up at {}",
                editor_ui::local_time::moment(at)
            )
            .into(),
            state: ComposerGoalState::Parked,
            detail: None,
        });
        let note = self
            .note
            .as_ref()
            .filter(|note| self.closed_note.as_ref() != Some(&note.id))
            .map(|note| ComposerGoal {
                objective: note.text.clone(),
                state: if note.heads_up {
                    ComposerGoalState::HeadsUp
                } else {
                    ComposerGoalState::Note
                },
                detail: None,
            });
        let shown = parked.or(note).or_else(|| self.thread_goal.clone());
        let place = cx.reduce_motion() || !self.strip_settled;
        if self.goal.set(shown, std::time::Instant::now(), place) {
            cx.notify();
        }
    }

    pub fn set_target(
        &mut self,
        target: ComposerTarget,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if target == self.target {
            return;
        }
        if let Some(session) = &self.session {
            session.update(cx, |session, cx| session.flush(cx));
        }
        self.leave_questionnaires();
        // A target switch is a scope change, never a goal-strip transition.
        self.goal = goal_strip::GoalStrip::default();
        self.thread_goal = None;
        self.note = None;
        self.closed_note = None;
        self.parked_until = None;
        self.strip_settled = false;
        self.mentions.forget_skills();
        self.target = target;
        self.target_generation = self.target_generation.wrapping_add(1);
        self.edit_revision = 0;
        self.cancelling = false;
        self.continuing = false;
        self.editor.update(cx, |editor, cx| {
            editor.set_text("", cx);
            editor.set_disabled(false, cx);
        });
        self.has_text = false;
        self.clear_attachments(cx);
        self.session = None;
        self.session_lease = None;
        self.session_subscription = None;
        self.restore_revision = Some(0);
        self.notice = None;
        self.config = None;
        self.project = None;
        self.reset_picker();
        self.open_target(cx);
        cx.notify();
    }

    #[must_use]
    pub const fn target(&self) -> &ComposerTarget {
        &self.target
    }

    #[must_use]
    pub const fn core(&self) -> &Entity<Core> {
        &self.core
    }

    #[cfg(test)]
    fn text<'a>(&self, cx: &'a App) -> &'a str {
        self.editor.read(cx).text()
    }

    fn host(&self, cx: &App) -> Option<Entity<HostData>> {
        self.core.read(cx).host(self.target.host()).cloned()
    }

    /// Follows the target's host and loads the target's draft.
    fn open_target(&mut self, cx: &mut Context<Self>) {
        let host = self.host(cx);
        self.host_subscription = host.as_ref().map(|host| {
            cx.subscribe(host, |this, host, event: &HostEvent, cx| {
                if matches!(event, HostEvent::Changed | HostEvent::Reset) {
                    // A draft that failed to load while Forge was away loads
                    // once it is back.
                    if host.read(cx).is_live()
                        && let Some(session) = this.session.clone()
                        && session.read(cx).error.is_some()
                        && !session.read(cx).loaded
                    {
                        session.update(cx, |session, cx| session.retry(cx));
                    }
                    // Settings may have added or removed an MCP server.
                    this.refresh_mention_catalog(cx);
                    this.refresh_picker_catalog(cx);
                    cx.notify();
                } else if matches!(event, HostEvent::ImagesChanged) {
                    this.sync_attachment_previews(cx);
                    // The project menu shows each project's icon.
                    if this.menu == Some(Menu::Project) {
                        cx.notify();
                    }
                } else if matches!(event, HostEvent::ThreadChanged { .. }) {
                    // A renamed thread is mentioned by its new title.
                    this.refresh_mention_catalog(cx);
                    cx.notify();
                }
            })
        });
        self.refresh_mention_catalog(cx);
        let Some(host) = host else {
            return;
        };
        self.load_skills(cx);
        let session = draft_session::session(
            &self.core,
            self.target.host().clone(),
            self.scope(),
            host.read(cx).client.clone(),
            cx,
        );
        let Some((session, lease)) = session else {
            self.notice=Some("There are 64 open or unsaved drafts. Finish saving a draft, then reopen this thread.".into());
            self.editor
                .update(cx, |editor, cx| editor.set_disabled(true, cx));
            return;
        };
        self.session_lease = Some(lease);
        self.session_subscription = Some(cx.subscribe(&session, Self::session_changed));
        self.session = Some(session);
        self.sync_session(cx);
    }

    fn session_changed(
        &mut self,
        _: Entity<DraftSession>,
        event: &SessionEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            SessionEvent::Accepted { thread, item } => {
                // This draft's settings have been consumed. Follow the thread's
                // authoritative settings, including changes made by commands.
                self.config = None;
                self.editor.update(cx, |editor, cx| {
                    editor.set_text("", cx);
                    editor.set_disabled(false, cx);
                });
                self.has_text = false;
                self.clear_attachments(cx);
                self.restore_revision = None;
                let host = self.target.host().clone();
                if matches!(self.target, ComposerTarget::NewThread { .. }) {
                    self.core.update(cx, |core, cx| {
                        core.navigate(
                            Screen::Thread {
                                host,
                                thread: thread.clone(),
                            },
                            cx,
                        );
                    });
                }
                cx.emit(ComposerEvent::Sent {
                    thread: thread.clone(),
                    item: item.clone(),
                });
            }
            SessionEvent::Reloaded => {
                self.editor.update(cx, |editor, cx| editor.set_text("", cx));
                self.has_text = false;
                self.clear_attachments(cx);
                self.restore_revision = Some(self.edit_revision);
                self.sync_session(cx);
            }
            SessionEvent::Changed => self.sync_session(cx),
        }
        cx.notify();
    }
    fn sync_session(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        self.sync_editor_enabled(cx);
        if !session.read(cx).loaded {
            return;
        }
        if self.restore_revision.take() == Some(self.edit_revision) {
            let body = session.read(cx).body.clone();
            if body != serde_json::Value::Null {
                match serde_json::from_value(body) {
                    Ok(body) => self.restore(&self.target.clone(), body, cx),
                    Err(error) => self.notice = Some(format!("Couldn't read this draft: {error}")),
                }
            }
        }
    }

    fn restore(&mut self, target: &ComposerTarget, body: DraftBody, cx: &mut Context<Self>) {
        // The user started typing before the draft arrived: theirs wins.
        if &self.target != target
            || (!self.writing_answer() && self.has_text)
            || self.has_attachments()
        {
            return;
        }
        if !self.questionnaire.restore_aside_text(&body.text) {
            self.editor
                .update(cx, |editor, cx| editor.set_text(&body.text, cx));
            self.has_text = !body.text.trim().is_empty();
        }
        self.config = body.config;
        self.project = body.project;
        for hash in body.images {
            self.restore_attachment(hash, cx);
        }
        for file in body.files {
            self.push_file(file.path, file.folder);
        }
        cx.notify();
    }

    fn scope(&self) -> String {
        match &self.target {
            ComposerTarget::Thread { thread, .. } => format!("thread:{thread}"),
            ComposerTarget::NewThread { project, .. } => {
                format!("new:{}", project.as_ref().map_or("", ProjectId::as_str))
            }
        }
    }

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "GPUI hands subscribers a reference"
    )]
    fn on_editor_event(
        &mut self,
        _: Entity<TextEditor>,
        event: &TextEditorEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            // Typing an answer is not a draft change.
            TextEditorEvent::Changed if self.writing_answer() => {
                let has_text = !self.editor.read(cx).text().trim().is_empty();
                if has_text != self.has_text {
                    self.has_text = has_text;
                    cx.notify();
                }
            }
            TextEditorEvent::Submitted if self.writing_answer() => self.submit_written_answer(cx),
            TextEditorEvent::Changed => {
                self.edited(cx);
                let has_text = !self.editor.read(cx).text().trim().is_empty();
                if has_text != self.has_text {
                    self.has_text = has_text;
                    cx.notify();
                }
            }
            TextEditorEvent::Submitted => self.send(cx),
        }
    }

    /// Queue every edit immediately; the session owns debounce and lifecycle flush.
    fn edited(&mut self, cx: &mut Context<Self>) {
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.schedule_save(cx);
    }
    fn schedule_save(&self, cx: &mut Context<Self>) {
        let body = DraftBody {
            text: self
                .questionnaire
                .aside_text()
                .unwrap_or_else(|| self.editor.read(cx).text())
                .to_owned(),
            images: self
                .attachments
                .iter()
                .filter_map(|attachment| attachment.state.hash().cloned())
                .collect(),
            files: self
                .files
                .iter()
                .map(|file| DraftFile {
                    path: file.path.clone(),
                    folder: file.folder,
                })
                .collect(),
            config: self.config.clone(),
            project: self.project.clone(),
        };
        #[expect(
            clippy::expect_used,
            reason = "DraftBody holds only strings, ids and string-keyed data; serializing it cannot fail"
        )]
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
        if let Some(session) = &self.session {
            session.update(cx, |session, cx| session.queue(value, cx));
        }
    }

    // Engine settings.

    /// The settings the next message runs with.
    fn effective_config(&self, cx: &App) -> Option<EngineConfig> {
        if let Some(config) = &self.config {
            return Some(config.clone());
        }
        let host = self.host(cx)?;
        let host = host.read(cx);
        match &self.target {
            ComposerTarget::Thread { thread, .. } => {
                host.threads.get(thread).map(|thread| thread.config.clone())
            }
            ComposerTarget::NewThread { .. } => {
                remembered_config(host).or_else(|| default_config(host))
            }
        }
    }

    fn sync_editor_enabled(&self, cx: &mut Context<Self>) {
        let disabled = !self.draft_editable(cx) || self.questionnaire.sending();
        self.editor
            .update(cx, |editor, cx| editor.set_disabled(disabled, cx));
    }

    fn draft_editable(&self, cx: &App) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| !session.read(cx).sending())
    }

    fn metadata_ready(&self, cx: &App) -> bool {
        match &self.target {
            ComposerTarget::NewThread { .. } => true,
            ComposerTarget::Thread { thread, .. } => self
                .host(cx)
                .is_some_and(|host| host.read(cx).threads.contains_key(thread)),
        }
    }
    fn choose_config(&mut self, config: EngineConfig, cx: &mut Context<Self>) {
        if !self.metadata_ready(cx) || !self.draft_editable(cx) {
            return;
        }
        self.apply_permissions(config.permissions, cx);
        self.remember_model_config(&config, cx);
        self.config = Some(config);
        self.edited(cx);
        // Another engine reads other commands.
        self.refresh_mention_catalog(cx);
        cx.notify();
    }

    /// Forge remembers the choice as the one this model comes back with.
    fn remember_model_config(&self, config: &EngineConfig, cx: &mut Context<Self>) {
        if let Some(host) = self.host(cx) {
            host.update(cx, |host, cx| {
                host.remember_model_config(config.clone(), cx);
            });
        }
    }

    /// The project a new thread goes to: the one picked in the tab, else
    /// the one it was opened for, else the one the user last started a
    /// thread in.
    pub(crate) fn project(&self, cx: &App) -> Option<ProjectId> {
        match &self.target {
            ComposerTarget::NewThread { project, .. } => self
                .project
                .clone()
                .or_else(|| project.clone())
                .or_else(|| last_project(self.host(cx)?.read(cx))),
            ComposerTarget::Thread { .. } => self.project.clone(),
        }
    }

    // Attachments.

    /// Pictures attach as images; any other file or folder as a link.
    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            let folder = path.is_dir();
            if !folder && is_image_path(&path) {
                let name = file_name(&path);
                self.add_image(Source::Path(path), name, cx);
                continue;
            }
            if !self.draft_editable(cx) {
                return;
            }
            if let Some(existing) = self.files.iter().find(|file| file.path == path) {
                self.shake = Some((existing.key, Instant::now()));
                cx.notify();
                continue;
            }
            if self.files.len() >= MAX_FILES {
                self.notice = Some(format!("A message can link at most {MAX_FILES} files."));
                cx.notify();
                return;
            }
            self.push_file(path, folder);
            self.edited(cx);
            cx.notify();
        }
    }

    fn push_file(&mut self, path: PathBuf, folder: bool) {
        let key = self.next_key;
        self.next_key += 1;
        self.files.push(FileAttachment {
            key,
            name: file_name(&path),
            path,
            folder,
        });
    }

    fn remove_file(&mut self, key: u64, cx: &mut Context<Self>) {
        if !self.draft_editable(cx) {
            return;
        }
        self.files.retain(|file| file.key != key);
        self.edited(cx);
        cx.notify();
    }

    const fn has_attachments(&self) -> bool {
        !self.attachments.is_empty() || !self.files.is_empty()
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one pipeline: placeholder, conversion and upload of a single image"
    )]
    fn add_image(&mut self, source: Source, name: SharedString, cx: &mut Context<Self>) {
        if !self.draft_editable(cx) {
            return;
        }
        if self.attachments.len() >= 8 {
            self.notice = Some("A message can contain at most 8 images.".into());
            cx.notify();
            return;
        }
        if matches!(&source, Source::Bytes(bytes) if bytes.len() > encode::MAX_INPUT_BYTES) {
            self.notice = Some("Images must be at most 32 MiB.".into());
            cx.notify();
            return;
        }
        let key = self.next_key;
        self.next_key += 1;
        let renderer = cx.svg_renderer();
        let task = cx.spawn(async move |this, cx| {
            static CONVERSION: futures::lock::Mutex<()> = futures::lock::Mutex::new(());
            let background = cx.background_executor().clone();
            // The still decodes in milliseconds, outside the conversion queue.
            let read = background
                .spawn(async move {
                    source.into_bytes().map(|bytes| {
                        let placeholder = encode::placeholder(&bytes);
                        (bytes, placeholder)
                    })
                })
                .await;
            let (bytes, placeholder) = match read {
                Ok(read) => read,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        if let Some(attachment) = this.attachment_mut(key) {
                            attachment.shown = true;
                        }
                        this.drive_attachment(
                            key,
                            &attachment::Input::Failed(format!("{error:#}")),
                            cx,
                        );
                    })
                    .ok();
                    return;
                }
            };
            let source = source_key(&bytes);
            let keep = this
                .update(cx, |this, cx| {
                    if let Some(existing) = this
                        .attachments
                        .iter()
                        .find(|other| other.key != key && other.source == Some(source))
                        .map(|a| a.key)
                    {
                        this.drop_duplicate(key, existing, cx);
                        return false;
                    }
                    let Some(attachment) = this.attachment_mut(key) else {
                        return false;
                    };
                    attachment.source = Some(source);
                    // The tile first appears with its blurred picture in place.
                    attachment.preview = placeholder;
                    attachment.shown = true;
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !keep {
                return;
            }
            let result = background
                .spawn(async move {
                    // Keep the permit in the CPU task even if its view is dropped.
                    let _permit = CONVERSION.lock().await;
                    let webp = encode::to_webp(&bytes)?;
                    let preview = editor_ui::markdown_renderer::decode_image_bytes(&webp, renderer)
                        .map_err(anyhow::Error::msg)?;
                    Ok::<_, anyhow::Error>((webp, preview))
                })
                .await;
            this.update(cx, |this, cx| match result {
                Ok((webp, preview)) => {
                    if let Err(error) = this.admit_preview(key, preview, cx) {
                        this.drive_attachment(key, &attachment::Input::Failed(error), cx);
                        return;
                    }
                    if let Some(attachment) = this.attachment_mut(key) {
                        attachment.encoded = Some(webp);
                    }
                    this.drive_attachment(key, &attachment::Input::Encoded, cx);
                }
                Err(error) => {
                    this.drive_attachment(
                        key,
                        &attachment::Input::Failed(format!("{error:#}")),
                        cx,
                    );
                }
            })
            .ok();
        });
        self.attachments.push(Attachment {
            key,
            name,
            state: attachment::State::Encoding,
            preview: None,
            encoded: None,
            preview_error: None,
            revealed: None,
            shown: false,
            source: None,
            task,
        });
        self.edited(cx);
        cx.notify();
    }
    fn restore_attachment(&mut self, hash: BlobHash, cx: &mut Context<Self>) {
        if self.attachments.len() >= 8 {
            self.notice = Some("This draft exceeds the 8 image limit.".into());
            return;
        }
        let key = self.next_key;
        self.next_key += 1;
        self.attachments.push(Attachment {
            key,
            name: "Image".into(),
            state: attachment::State::Ready(hash.clone()),
            preview: None,
            encoded: None,
            preview_error: None,
            revealed: None,
            shown: true,
            source: None,
            task: Task::ready(()),
        });
        if let Some(host) = self.host(cx) {
            host.update(cx, |host, cx| host.load_image(hash, false, cx));
        }
        self.sync_attachment_previews(cx);
    }
    fn admit_preview(
        &mut self,
        key: u64,
        image: Arc<RenderImage>,
        cx: &mut App,
    ) -> Result<(), String> {
        let retained = self
            .attachments
            .iter()
            .filter(|a| a.key != key)
            .filter_map(|a| a.preview.as_ref())
            .map(|image| editor_ui::markdown_renderer::image_resident_bytes(image))
            .sum();
        if !preview_fits(
            retained,
            editor_ui::markdown_renderer::image_resident_bytes(&image),
        ) {
            return Err("Image previews exceed the message's 128 MiB budget".into());
        }
        if let Some(attachment) = self.attachment_mut(key) {
            let previous = attachment.preview.replace(image);
            release_preview(previous, cx);
            attachment.preview_error = None;
            // Restored previews arrive ready; converted ones reveal on upload.
            if attachment.state.hash().is_some() {
                attachment.revealed = Some(Instant::now());
            }
        }
        Ok(())
    }
    fn sync_attachment_previews(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host(cx) else {
            return;
        };
        let requested: Vec<_> = self
            .attachments
            .iter()
            .filter(|a| a.preview.is_none() && a.preview_error.is_none())
            .filter_map(|a| a.state.hash().cloned())
            .collect();
        for hash in requested {
            host.update(cx, |host, cx| host.load_image(hash, false, cx));
        }
        let pending: Vec<_> = self
            .attachments
            .iter()
            .filter(|a| a.preview.is_none() && a.preview_error.is_none())
            .filter_map(|a| {
                a.state
                    .hash()
                    .and_then(|hash| host.read(cx).image(hash))
                    .map(|image| (a.key, image))
            })
            .collect();
        for (key, image) in pending {
            let error = match image {
                editor_core::BlobImage::Ready(image) => self.admit_preview(key, image, cx).err(),
                editor_core::BlobImage::Failed(error) => Some(error.to_string()),
                editor_core::BlobImage::Loading => None,
            };
            if let Some(error) = error
                && let Some(attachment) = self.attachment_mut(key)
            {
                attachment.preview_error = Some(error);
            }
        }
        cx.notify();
    }
    fn retry_preview(&mut self, key: u64, cx: &mut Context<Self>) {
        let hash = self
            .attachments
            .iter()
            .find(|a| a.key == key)
            .and_then(|a| a.state.hash())
            .cloned();
        if let (Some(hash), Some(host)) = (hash, self.host(cx)) {
            if let Some(attachment) = self.attachment_mut(key) {
                attachment.preview_error = None;
            }
            host.update(cx, |host, cx| host.load_image(hash, true, cx));
            self.sync_attachment_previews(cx);
            cx.notify();
        }
    }

    fn drive_attachment(&mut self, key: u64, input: &attachment::Input, cx: &mut Context<Self>) {
        let client = self.host(cx).map(|host| host.read(cx).client.clone());
        let Some(attachment) = self
            .attachments
            .iter_mut()
            .find(|attachment| attachment.key == key)
        else {
            return;
        };
        if attachment.state.accept(input) {
            if let (Some(client), Some(encoded)) = (client, attachment.encoded.take()) {
                attachment.task = cx.spawn(async move |this, cx| {
                    let input = match client.put_blob(encoded).await {
                        Ok(blob) => attachment::Input::Uploaded(blob.hash),
                        Err(error) => {
                            attachment::Input::Failed(format!("Upload failed: {error:#}"))
                        }
                    };
                    this.update(cx, |this, cx| this.drive_attachment(key, &input, cx))
                        .ok();
                });
            } else {
                attachment.state = attachment::State::Failed(
                    "Upload has no connected Forge or encoded image.".into(),
                );
            }
        }
        if matches!(input, attachment::Input::Uploaded(_)) {
            if attachment.state.hash().is_some() && attachment.preview.is_some() {
                attachment.revealed = Some(Instant::now());
            }
            self.schedule_save(cx);
        }
        cx.notify();
    }

    /// Remove the duplicate attachment and shake the existing tile.
    fn drop_duplicate(&mut self, key: u64, existing: u64, cx: &mut Context<Self>) {
        if let Some(index) = self.attachments.iter().position(|a| a.key == key) {
            // Called from that attachment's own task: let it finish rather
            // than cancel it mid-poll.
            let attachment = self.attachments.remove(index);
            release_preview(attachment.preview, cx);
            attachment.task.detach();
        }
        self.shake = Some((existing, Instant::now()));
        self.edited(cx);
        cx.notify();
    }

    fn attachment_mut(&mut self, key: u64) -> Option<&mut Attachment> {
        self.attachments
            .iter_mut()
            .find(|attachment| attachment.key == key)
    }

    fn remove_attachment(&mut self, key: u64, cx: &mut Context<Self>) {
        if !self.draft_editable(cx) {
            return;
        }
        if let Some(index) = self
            .attachments
            .iter()
            .position(|attachment| attachment.key == key)
        {
            let attachment = self.attachments.remove(index);
            release_preview(attachment.preview, cx);
        }
        self.edited(cx);
        cx.notify();
    }

    fn clear_attachments(&mut self, cx: &mut App) {
        self.files.clear();
        for attachment in self.attachments.drain(..) {
            release_preview(attachment.preview, cx);
        }
    }

    // Sending.

    fn is_working(&self, cx: &App) -> bool {
        match &self.target {
            ComposerTarget::Thread { thread, .. } => self
                .host(cx)
                .is_some_and(|host| host.read(cx).is_working(thread)),
            ComposerTarget::NewThread { .. } => false,
        }
    }

    /// The thread's last turn stopped before it finished, and there is
    /// nothing to send: the primary button carries the turn on.
    fn can_continue(&self, cx: &App) -> bool {
        let ComposerTarget::Thread { thread, .. } = &self.target else {
            return false;
        };
        !self.writing_answer()
            && !self.has_text
            && !self.has_attachments()
            && self
                .host(cx)
                .is_some_and(|host| host.read(cx).can_continue(thread))
    }

    /// The engine's guess at the thread's next message, offered while the
    /// composer would send one: not mid-run, nor while answering a question.
    fn suggestion(&self, cx: &App) -> Option<SharedString> {
        let ComposerTarget::Thread { thread, .. } = &self.target else {
            return None;
        };
        if self.writing_answer() || self.is_working(cx) {
            return None;
        }
        self.host(cx)?.read(cx).suggestion(thread).cloned()
    }

    /// Why sending is not possible right now, if it isn't.
    fn blocked(&self, cx: &App) -> Option<&'static str> {
        if !self.metadata_ready(cx) {
            return Some("Loading thread settings…");
        }
        if self.session.is_none() {
            return Some("Draft unavailable");
        }
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.read(cx).sending())
        {
            return Some("Sending…");
        }
        if !self.has_text && !self.has_attachments() {
            return Some("");
        }
        if self
            .attachments
            .iter()
            .any(|attachment| attachment.state.failure().is_some())
        {
            return Some("Remove the images that failed");
        }
        if self
            .attachments
            .iter()
            .any(|attachment| attachment.state.hash().is_none())
        {
            // The blur on the tile is the only sign; send just waits.
            return Some("");
        }
        if matches!(self.target, ComposerTarget::NewThread { .. }) && self.project(cx).is_none() {
            return Some("Choose a project");
        }
        if self.effective_config(cx).is_none() {
            return Some("Choose a model");
        }
        None
    }

    /// Sends the draft, or retries the failed send with the same id. While
    /// a question is open, records the typed answer instead.
    fn send(&mut self, cx: &mut Context<Self>) {
        if self.writing_answer() {
            self.submit_written_answer(cx);
            return;
        }
        if self.blocked(cx).is_some() {
            return;
        }
        let Some(submission) = self.outgoing_message(cx) else {
            return;
        };
        self.reset_picker();
        self.schedule_save(cx);
        if let Some(session) = &self.session {
            session.update(cx, |session, cx| session.submit(submission, cx));
        }
        cx.notify();
    }

    fn outgoing_message(&self, cx: &App) -> Option<Submission> {
        let config = self.effective_config(cx)?;
        let text = with_file_links(self.editor.read(cx).text().trim(), &self.files);
        let images = self
            .attachments
            .iter()
            .filter_map(|attachment| attachment.state.hash().cloned())
            .collect();
        let host = self.host(cx)?;
        let (create, thread_config) = match &self.target {
            ComposerTarget::Thread { thread, .. } => (
                None,
                host.read(cx)
                    .threads
                    .get(thread)
                    .map(|thread| thread.config.clone()),
            ),
            ComposerTarget::NewThread { .. } => {
                let create = CreateThread {
                    id: ThreadId::new(),
                    project_id: self.project(cx)?,
                    config: config.clone(),
                    title: None,
                    parent_thread_id: None,
                    spawned_by: None,
                };
                (Some(create), Some(config.clone()))
            }
        };
        let config = (thread_config.as_ref() != Some(&config)).then_some(config);
        let thread = create
            .as_ref()
            .map(|create| create.id.clone())
            .or_else(|| match &self.target {
                ComposerTarget::Thread { thread, .. } => Some(thread.clone()),
                ComposerTarget::NewThread { .. } => None,
            })?;
        Some(Submission {
            message: SendMessage {
                id: ItemId::new(),
                text,
                images,
                config,
                draft: None,
            },
            create,
            thread,
        })
    }

    /// Carries on the thread's stopped turn, with no message.
    fn continue_turn(&mut self, cx: &mut Context<Self>) {
        if self.continuing {
            return;
        }
        let (ComposerTarget::Thread { thread, .. }, Some(host)) = (&self.target, self.host(cx))
        else {
            return;
        };
        let client = host.read(cx).client.clone();
        let thread = thread.clone();
        let generation = self.target_generation;
        self.continuing = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.continue_turn(&thread).await;
            this.update(cx, |this, cx| {
                if this.target_generation != generation {
                    return;
                }
                this.continuing = false;
                if let Err(error) = result {
                    this.notice = Some(format!("Couldn't continue: {error:#}"));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Access is the thread's, not the next message's: a change is saved at
    /// once, so a live run switches to it (Full also approves what waits).
    fn apply_permissions(&self, permissions: Permissions, cx: &Context<Self>) {
        let (ComposerTarget::Thread { thread, .. }, Some(host)) = (&self.target, self.host(cx))
        else {
            return;
        };
        let host = host.read(cx);
        let Some(mut config) = host.threads.get(thread).map(|thread| thread.config.clone()) else {
            return;
        };
        if config.permissions == permissions {
            return;
        }
        config.permissions = permissions;
        let client = host.client.clone();
        let thread = thread.clone();
        let generation = self.target_generation;
        cx.spawn(async move |this, cx| {
            let update = UpdateThread {
                config: Some(config),
                ..UpdateThread::default()
            };
            let result = client.update_thread(&thread, &update).await;
            this.update(cx, |this, cx| {
                if this.target_generation != generation {
                    return;
                }
                if let Err(error) = result {
                    this.notice = Some(format!("Couldn't change access: {error:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Clears the thread's goal. The strip leaves when Forge reports it
    /// cleared, so a refusal leaves it standing.
    fn clear_goal(&self, cx: &Context<Self>) {
        let (ComposerTarget::Thread { thread, .. }, Some(host)) = (&self.target, self.host(cx))
        else {
            return;
        };
        let client = host.read(cx).client.clone();
        let thread = thread.clone();
        let generation = self.target_generation;
        cx.spawn(async move |this, cx| {
            let result = client.clear_goal(&thread).await;
            this.update(cx, |this, cx| {
                if this.target_generation != generation {
                    return;
                }
                if let Err(error) = result {
                    this.notice = Some(format!("Couldn't clear the goal: {error:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Cancels the parked turn's pickup; the thread stays failed. Forge's
    /// cancel drops it, and the strip leaves when Forge reports that.
    fn unpark(&self, cx: &Context<Self>) {
        let (ComposerTarget::Thread { thread, .. }, Some(host)) = (&self.target, self.host(cx))
        else {
            return;
        };
        let client = host.read(cx).client.clone();
        let thread = thread.clone();
        let generation = self.target_generation;
        cx.spawn(async move |this, cx| {
            let result = client.cancel(&thread).await;
            this.update(cx, |this, cx| {
                if this.target_generation != generation {
                    return;
                }
                if let Err(error) = result {
                    this.notice = Some(format!("Couldn't cancel the pickup: {error:#}"));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Stops the thread's live run.
    fn stop(&mut self, cx: &mut Context<Self>) {
        if self.cancelling {
            return;
        }
        let (ComposerTarget::Thread { thread, .. }, Some(host)) = (&self.target, self.host(cx))
        else {
            return;
        };
        let client = host.read(cx).client.clone();
        let thread = thread.clone();
        let generation = self.target_generation;
        self.cancelling = true;
        #[cfg(test)]
        let completion_gate = {
            self.cancel_requests.push(thread.clone());
            self.cancel_completion_gate.take()
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.cancel(&thread).await;
            #[cfg(test)]
            let result = if let Some((arrived, release)) = completion_gate {
                arrived.store(true, std::sync::atomic::Ordering::Release);
                if release.await.unwrap_or(false) {
                    Err(anyhow::anyhow!("injected cancellation failure"))
                } else {
                    result
                }
            } else {
                result
            };
            this.update(cx, |this, cx| {
                #[cfg(test)]
                {
                    this.cancel_completions += 1;
                }
                if this.target_generation != generation {
                    return;
                }
                this.cancelling = false;
                if let Err(error) = result {
                    this.notice = Some(format!("Couldn't stop the run: {error:#}"));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens `engine`'s settings page, where it installs and signs in.
    fn sign_in(&mut self, engine: EngineId, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.core.update(cx, |core, cx| {
            core.navigate(
                Screen::Settings(editor_core::SettingsPage::Engine(engine)),
                cx,
            );
        });
    }
}

/// The engine settings last chosen on `host` (used in the composer or set in
/// Settings), while that engine can still run them: installed, not signed
/// out, and the model still listed. Before Forge has listed the engine's
/// models the model is kept as it is.
fn remembered_config(host: &HostData) -> Option<EngineConfig> {
    let value = host.setting(DEFAULT_CONFIG_KEY)?;
    let mut config = match serde_json::from_value::<EngineConfig>(value.clone()) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(host = %host.id, "ignored unreadable remembered engine settings: {error}");
            return None;
        }
    };
    // The model comes back as it was last chosen, also when that was after
    // the last message.
    if let Some(saved) = config
        .model
        .as_deref()
        .and_then(|model| saved_model_config(host, config.engine, model))
    {
        config = saved;
    }
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

/// The config last chosen for `engine`'s `model` on `host`, as saved.
pub(crate) fn saved_model_config(
    host: &HostData,
    engine: EngineId,
    model: &str,
) -> Option<EngineConfig> {
    artisan_api::saved_model_config(host.setting(artisan_api::MODEL_CONFIGS_KEY)?, engine, model)
}

/// The project the user last started a thread in, while it is still there.
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

/// The default for a new thread: the first signed-in engine with models,
/// its default model and that model's default effort; else the first
/// installed engine with its own default model.
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
    config.effort = model.and_then(|model| model.default_effort.clone());
    Some(config)
}

enum Source {
    Bytes(Vec<u8>),
    Path(PathBuf),
}

impl Source {
    fn into_bytes(self) -> anyhow::Result<Vec<u8>> {
        match self {
            Self::Bytes(bytes) => {
                anyhow::ensure!(
                    bytes.len() <= encode::MAX_INPUT_BYTES,
                    "Images must be at most 32 MiB"
                );
                Ok(bytes)
            }
            Self::Path(path) => {
                use std::io::Read as _;
                let file = std::fs::File::open(&path)
                    .with_context(|| format!("read {}", path.display()))?;
                anyhow::ensure!(
                    file.metadata()?.len() <= encode::MAX_INPUT_BYTES as u64,
                    "Images must be at most 32 MiB"
                );
                let mut bytes = Vec::new();
                file.take((encode::MAX_INPUT_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)?;
                anyhow::ensure!(
                    bytes.len() <= encode::MAX_INPUT_BYTES,
                    "Images must be at most 32 MiB"
                );
                Ok(bytes)
            }
        }
    }
}

/// Identifies the picture as given; with its length, collisions don't
/// matter in practice.
fn source_key(bytes: &[u8]) -> (u64, usize) {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::hash::DefaultHasher::new();
    bytes.hash(&mut hasher);
    (hasher.finish(), bytes.len())
}

/// At most this many files and folders link from one message.
const MAX_FILES: usize = 32;

/// Extensions the image path attaches as pictures; anything else links.
fn is_image_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp"
            )
        })
}

fn file_name(path: &std::path::Path) -> SharedString {
    path.file_name()
        .map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
        .into()
}

/// The message text, then one markdown link per file or folder. Angle
/// brackets keep paths with spaces or parentheses one link.
fn with_file_links(text: &str, files: &[FileAttachment]) -> String {
    if files.is_empty() {
        return text.to_owned();
    }
    let links = files
        .iter()
        .map(|file| {
            let slash = if file.folder { "/" } else { "" };
            let name = file.name.replace(['[', ']'], "");
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

// Rendering.

impl Composer {
    /// AttachmentTile owns each preview, loading/error state and remove action.
    fn render_attachments(&self, theme: &ArtisanTheme, cx: &Context<Self>) -> impl IntoElement {
        let mut row = div()
            .id("composer-attachments")
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(4.0))
            .pt(px(4.0))
            .pb(px(8.0))
            .overflow_x_scroll();
        for attachment in self.attachments.iter().filter(|a| a.shown) {
            let key = attachment.key;
            let owner = cx.entity().downgrade();
            let mut tile = editor_ui::attachment_tile::AttachmentTile::new(
                format!("composer-attachment-{key}"),
                *theme,
                attachment.name.clone(),
            )
            .preview(attachment.preview.clone(), attachment.revealed)
            .preparing(matches!(
                attachment.state,
                attachment::State::Encoding | attachment::State::Uploading
            ))
            .error(
                attachment
                    .state
                    .failure()
                    .or(attachment.preview_error.as_deref())
                    .map(str::to_owned),
            )
            .shaking(
                self.shake
                    .filter(|(shaken, _)| *shaken == key)
                    .map(|(_, since)| since),
            )
            .removable(self.draft_editable(cx))
            .on_remove(move |_, cx| {
                owner
                    .update(cx, |this, cx| this.remove_attachment(key, cx))
                    .ok();
            });
            if attachment.preview_error.is_some() {
                let owner = cx.entity().downgrade();
                tile = tile.on_retry(move |_, cx| {
                    owner
                        .update(cx, |this, cx| this.retry_preview(key, cx))
                        .ok();
                });
            }
            row = row.child(tile);
        }
        for file in &self.files {
            let key = file.key;
            let owner = cx.entity().downgrade();
            let glyph = if file.folder {
                AssetId::TABLER_FOLDER
            } else {
                AssetId::TABLER_FILE_TEXT
            };
            row = row.child(
                editor_ui::attachment_tile::AttachmentTile::new(
                    format!("composer-file-{key}"),
                    *theme,
                    file.name.clone(),
                )
                .file(glyph)
                .shaking(
                    self.shake
                        .filter(|(shaken, _)| *shaken == key)
                        .map(|(_, since)| since),
                )
                .removable(self.draft_editable(cx))
                .on_remove(move |_, cx| {
                    owner.update(cx, |this, cx| this.remove_file(key, cx)).ok();
                }),
            );
        }
        row
    }

    /// The primary control (`render_control_row`): a blue icon button that
    /// sends (or steers a live run) with the arrow, stops the run with the
    /// filled square while there is nothing to send, and carries a stopped
    /// turn on with the play glyph while there is nothing to send.
    fn render_primary(&self, theme: &ArtisanTheme, cx: &Context<Self>) -> impl IntoElement {
        let theme = *theme;
        let working = self.is_working(cx);
        let ready = if self.writing_answer() {
            self.has_text
        } else {
            self.blocked(cx).is_none()
        };
        let face = if working && !ready {
            Primary::Stop
        } else if !working && !ready && self.can_continue(cx) {
            Primary::Continue
        } else {
            Primary::Send
        };
        let (icon, label, id) = match face {
            Primary::Stop => (AssetId::TABLER_PLAYER_STOP_FILLED, "Stop", "composer-stop"),
            Primary::Continue => (
                AssetId::TABLER_PLAYER_PLAY_FILLED,
                "Continue where it stopped",
                "composer-continue",
            ),
            Primary::Send => (AssetId::TABLER_ARROW_UP, "Send message", "composer-send"),
        };
        let entity = cx.entity().downgrade();
        let button = Button::icon(id, theme, icon, label)
            .debug_selector(id)
            .variant(ButtonVariant::Accent)
            .shape(ButtonShape::Rounded)
            .disabled(match face {
                Primary::Stop => self.cancelling,
                Primary::Continue => self.continuing,
                Primary::Send => !ready,
            })
            .on_activate(move |_, _, cx| {
                entity
                    .update(cx, |this, cx| match face {
                        Primary::Stop => this.stop(cx),
                        Primary::Continue => this.continue_turn(cx),
                        Primary::Send => this.send(cx),
                    })
                    .ok();
            });
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .child(button)
    }
}

impl Render for Composer {
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder stacks the card, its tab, the error line and the picker tooltip in visual order"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_mention_theme(cx);
        let theme = *cx.theme();
        self.viewport_width = f32::from(window.viewport_size().width);
        self.viewport_height = f32::from(window.viewport_size().height);
        let desktop = theme.colors;
        // Each time the draft empties again, the next phrase shows.
        let placeholder_visible = !self.has_text && !self.has_attachments();
        if let Some(answer) = self.answer_placeholder(cx) {
            self.editor
                .update(cx, |editor, cx| editor.set_placeholder(answer, cx));
        } else if placeholder_visible && !self.placeholder_was_visible {
            self.placeholder_generation = self.placeholder_generation.wrapping_add(1);
            let phrase = placeholder::phrase(self.placeholder_generation);
            self.editor
                .update(cx, |editor, cx| editor.set_placeholder(phrase, cx));
        }
        // An answer's placeholder hands back to the next phrase.
        self.placeholder_was_visible = placeholder_visible && !self.writing_answer();
        let suggestion = self.suggestion(cx);
        self.editor
            .update(cx, |editor, cx| editor.set_suggestion(suggestion, cx));
        // Offline, the connection card says why; these would only repeat it.
        let live = self.host(cx).is_some_and(|host| host.read(cx).is_live());
        let error = self
            .session
            .as_ref()
            .and_then(|session| session.read(cx).error.clone())
            .or_else(|| self.notice.clone())
            .filter(|_| live);

        let metadata_error = match &self.target {
            ComposerTarget::Thread { thread, .. } => self
                .host(cx)
                .and_then(|host| host.read(cx).thread_load_error(thread).map(str::to_owned)),
            ComposerTarget::NewThread { .. } => None,
        };
        let controls = div()
            .id("composer-controls")
            .w_full()
            .h(px(32.0))
            .flex_shrink_0()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .text_color(desktop.text_muted.to_paint())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .min_w(px(0.0))
                    .child(self.render_model_trigger(&theme, cx)),
            )
            .child(self.render_primary(&theme, cx));

        let mut card = div()
            .w_full()
            .flex()
            .flex_col()
            .children(metadata_error.map(|error| {
                div().text_xs().child(error).child(
                    Button::text("composer-thread-retry", theme, "Retry thread settings")
                        .variant(ButtonVariant::Ghost)
                        .on_activate(cx.listener(|this, _, _, cx| {
                            if let ComposerTarget::Thread { thread, .. } = &this.target {
                                let thread = thread.clone();
                                if let Some(host) = this.host(cx) {
                                    host.update(cx, |host, cx| host.ensure_thread(&thread, cx));
                                }
                            }
                        })),
                )
            }));
        // Open agent questions extend the card upward, above the editor.
        if let Some(panel) = self.render_questionnaires(&desktop, cx) {
            card = card.child(panel);
        }
        if !self.writing_answer()
            && (self.attachments.iter().any(|a| a.shown) || !self.files.is_empty())
        {
            card = card.child(self.render_attachments(&theme, cx));
        }
        card = card.child(self.editor.clone());
        // The mention menu hangs above the card at its full width.
        if let Some(menu) = self.render_mention_menu(window, &desktop, cx) {
            card = card.child(menu);
        }
        card = card.child(controls);
        // The tab frames only the card; the goal grows out of it. The shell
        // casts the shadow, so a framed card keeps only its rims.
        let tab = self.render_tab(&theme, &desktop, cx);
        let card = div()
            .id("composer")
            .w_full()
            .on_drop::<ExternalPaths>(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_paths(paths.paths().to_vec(), cx);
            }))
            .on_drop::<DraggedThread>(cx.listener(|this, thread: &DraggedThread, window, cx| {
                this.mention_thread(thread, window, cx);
            }))
            .child(ComposerFrame::new(theme, card).nested(tab.is_some()));

        let card = match tab {
            Some(tab) => {
                let mut moving = false;
                let this = cx.entity().downgrade();
                let actions = goal_strip::StripActions {
                    close_note: std::rc::Rc::new({
                        let this = this.clone();
                        move |_, cx| {
                            this.update(cx, Self::close_note).ok();
                        }
                    }),
                    clear_goal: std::rc::Rc::new({
                        let this = this.clone();
                        move |_, cx| {
                            this.update(cx, |this, cx| this.clear_goal(cx)).ok();
                        }
                    }),
                    unpark: std::rc::Rc::new(move |_, cx| {
                        this.update(cx, |this, cx| this.unpark(cx)).ok();
                    }),
                };
                let strip = self
                    .goal
                    .frame(
                        std::time::Instant::now(),
                        &theme,
                        cx.reduce_motion(),
                        &mut moving,
                        &actions,
                    )
                    .map(|frame| TabCardStrip {
                        reveal: frame.reveal,
                        content: frame.content,
                    });
                if moving {
                    window.request_animation_frame();
                }
                TabCard::new("composer-tab-card", theme, tab, card)
                    .strip(strip)
                    .into_any_element()
            }
            None => card.into_any_element(),
        };
        let mut shell = div().w_full().flex().flex_col().gap(px(8.0));
        if let Some(error) = error {
            shell = shell.child(
                div()
                    .px(px(12.0))
                    .text_size(TypographyTokens::size(TextStep::XsMid))
                    .text_color(theme.colors.destructive_text.to_paint())
                    .child(error),
            );
        }
        if let Some(session) = self
            .session
            .as_ref()
            .filter(|session| live && session.read(cx).error.is_some())
        {
            if session.read(cx).conflict {
                let local = session.clone();
                let remote = session.clone();
                shell = shell.child(
                    div()
                        .flex()
                        .gap(px(8.0))
                        .child(
                            Button::text("draft-keep-local", theme, "Keep my draft").on_activate(
                                move |_, _, cx| {
                                    local.update(cx, |session, cx| session.keep_local(cx));
                                },
                            ),
                        )
                        .child(
                            Button::text("draft-use-remote", theme, "Load other draft")
                                .on_activate(move |_, _, cx| {
                                    remote.update(cx, draft_session::DraftSession::use_remote);
                                }),
                        ),
                );
            } else {
                let session = session.clone();
                shell = shell.child(Button::text("draft-retry", theme, "Retry").on_activate(
                    move |_, _, cx| session.update(cx, |session, cx| session.retry(cx)),
                ));
            }
        }
        shell = shell.child(card);
        if let Some(tooltip) = self.render_tooltip(&theme, window.viewport_size()) {
            shell = shell.child(tooltip);
        }
        shell
    }
}

fn preview_fits(retained: usize, incoming: usize) -> bool {
    retained
        .checked_add(incoming)
        .is_some_and(|total| total <= 128 * 1024 * 1024)
}

/// GPUI's image atlas is independent of the decoded Arc's lifetime.
fn release_preview(image: Option<Arc<RenderImage>>, cx: &mut App) {
    if let Some(image) = image {
        cx.defer(move |cx| {
            cx.drop_image(image, None);
            // Other visible owners can retain these pixels after this owner
            // releases them. Invalidate cached scenes before they reuse a tile.
            cx.refresh_windows();
        });
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod preview_budget_tests {
    #[test]
    fn restored_and_encoded_previews_share_aggregate_admission() {
        let budget = 128 * 1024 * 1024;
        assert!(super::preview_fits(budget - 1, 1));
        assert!(!super::preview_fits(budget - 1, 2));
        assert!(!super::preview_fits(usize::MAX, 1));
    }
}
