//! The composer: one draft, its attachments and engine settings, and the
//! send button that starts a run or steers the live one.
//!
//! The draft lives in Forge (`/drafts/{scope}`), saved a moment after each
//! edit. Sending posts a client-made message id; the draft is cleared only
//! once Forge accepted it. While the thread's run is live the same send is a
//! steer: Forge decides, the composer only says so on the button.

mod attachment;
mod encode;
mod goal_strip;
mod mentions;
mod picker;
mod placeholder;
mod questionnaire;
mod send;

pub use goal_strip::{ComposerGoal, ComposerGoalState};
pub use questionnaire::ComposerQuestionnaire;

use editor_ui::text_editor::{PastedFiles, TextEditor, TextEditorEvent};

use anyhow::Context as _;
use artisan_api::{
    BlobHash, CreateThread, EngineConfig, EngineId, ItemId, ProjectId, SendMessage, SignInState,
    ThreadId,
};
use editor_core::{Core, HostData, HostEvent, HostId, Screen};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::button::{AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant};
use editor_ui::glass::{
    GlassStrength, card_shadows, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use editor_ui::motion::{MotionDuration, MotionPolicy};
use editor_ui::picker_motion::smooth_out;
use editor_ui::tab_card::{TabCardStrip, tab_card_with_strip};
use editor_ui::theme::{ArtisanTheme, DesktopTheme, RadiusTokens, ThemeMode};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, ColorExt as _, Context, Entity, EventEmitter, ExternalPaths,
    FocusHandle, Focusable, Image, ImageFormat, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Subscription, Task, Window, canvas, div, img, px,
};
use serde::{Deserialize, Serialize};
use statig::blocking::{IntoStateMachineExt as _, StateMachine};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    fn host(&self) -> &HostId {
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

const DRAFT_SAVE_DELAY: Duration = Duration::from_millis(400);
/// The Editor-local setting holding, per host id, the engine settings last
/// picked or sent with. New threads on that host start from them.
/// Forge's setting for the engine settings new threads start with; Settings
/// → Engines edits the same one, so the latest choice wins either way.
const DEFAULT_CONFIG_KEY: &str = "default_engine_config";
const RADIUS: f32 = 18.0;
const TILE: f32 = 72.0;

/// What `/drafts/{scope}` holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct DraftBody {
    #[serde(default)]
    text: String,
    #[serde(default)]
    images: Vec<BlobHash>,
    #[serde(default)]
    config: Option<EngineConfig>,
}

/// The message in flight; kept so a retry sends exactly the same thing.
struct Outgoing {
    message: SendMessage,
    /// Set until Forge created the thread.
    create: Option<CreateThread>,
    /// The draft to clear once Forge has the message.
    scope: String,
}

pub(crate) struct Attachment {
    key: u64,
    name: SharedString,
    machine: StateMachine<attachment::Machine>,
    /// The WebP, once encoded or fetched back from Forge.
    preview: Option<Arc<Image>>,
    /// The picture as given, shown blurred while the WebP encodes.
    draft: Option<Arc<Image>>,
    /// When the preview arrived: it sharpens out of the blur from then.
    revealed: Option<Instant>,
    /// Hash and length of the picture as given, to spot it pasted twice.
    source: Option<(u64, usize)>,
    task: Task<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Model,
    Project,
}

pub struct Composer {
    core: Entity<Core>,
    target: ComposerTarget,
    editor: Entity<TextEditor>,
    has_text: bool,
    attachments: Vec<Attachment>,
    next_key: u64,
    /// The attachment a duplicate paste or drop shook, and when; every
    /// repeat restarts it.
    shake: Option<(u64, Instant)>,
    send: StateMachine<send::Machine>,
    outgoing: Option<Outgoing>,
    /// Failures outside sending (stop, attachments read from disk).
    notice: Option<String>,
    /// The user's engine choice; `None` follows the thread.
    config: Option<EngineConfig>,
    /// Chosen for a new thread whose target has no project.
    project: Option<ProjectId>,
    menu: Option<Menu>,
    /// The engine whose models the open model menu shows.
    browsing: Option<EngineId>,
    /// The policy dropdown open in the model menu.
    axis: Option<picker::Axis>,
    /// The picker surfaces' motion, hover pills, engine light and tooltip.
    picker: picker::PickerMotion,
    /// The window's width at the last render; the model menu fits it.
    viewport_width: f32,
    /// The window's height at the last render; the project menu fits it.
    viewport_height: f32,
    /// The thread's goal, on the strip along the card's top edge.
    goal: goal_strip::GoalStrip,
    /// Which placeholder phrase shows: the next one each time the draft
    /// empties again.
    placeholder_generation: u64,
    placeholder_was_visible: bool,
    /// Focus for the primary (send or stop) button.
    button_focus: FocusHandle,
    /// Skills and connectors named in the draft, and their menu.
    mentions: mentions::Mentions,
    /// The agent's open questions, answered from the card.
    questionnaire: questionnaire::QuestionnairePanel,
    /// Pending debounced draft save; dropping it cancels.
    save: Option<Task<()>>,
    load_draft: Task<()>,
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
            editor,
            has_text: false,
            attachments: Vec::new(),
            next_key: 0,
            shake: None,
            send: send::Machine.state_machine(),
            outgoing: None,
            notice: None,
            config: None,
            project: None,
            menu: None,
            browsing: None,
            axis: None,
            picker: picker::PickerMotion::default(),
            viewport_width: 1_280.0,
            viewport_height: 800.0,
            goal: goal_strip::GoalStrip::default(),
            placeholder_generation: 0,
            placeholder_was_visible: false,
            button_focus: cx.focus_handle(),
            mentions: mentions::Mentions::default(),
            questionnaire: questionnaire::QuestionnairePanel::default(),
            save: None,
            load_draft: Task::ready(()),
            host_subscription: None,
            _editor_subscriptions: editor_subscriptions,
        };
        composer.open_target(cx);
        composer
    }

    /// Shows the thread's goal on the strip along the card's top edge, or
    /// retracts it.
    pub fn set_goal(&mut self, goal: Option<ComposerGoal>, cx: &mut Context<Self>) {
        if self.goal.set(goal, std::time::Instant::now(), false) {
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
        if self.save.take().is_some() {
            self.save_now(cx);
        }
        self.leave_questionnaires();
        if target.host() != self.target.host() {
            self.mentions.forget_skills();
        }
        self.target = target;
        self.editor.update(cx, |editor, cx| {
            editor.set_text("", cx);
            editor.set_disabled(false, cx);
        });
        self.has_text = false;
        self.attachments.clear();
        self.send = send::Machine.state_machine();
        self.outgoing = None;
        self.notice = None;
        self.config = None;
        self.project = None;
        self.reset_picker();
        self.open_target(cx);
        cx.notify();
    }

    #[must_use]
    pub fn target(&self) -> &ComposerTarget {
        &self.target
    }

    #[must_use]
    pub fn core(&self) -> &Entity<Core> {
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
            cx.subscribe(host, |this, _, event: &HostEvent, cx| {
                if matches!(event, HostEvent::Changed | HostEvent::Reset) {
                    // Settings may have added or removed an MCP server.
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
        let client = host.read(cx).client.clone();
        let scope = self.scope();
        let target = self.target.clone();
        self.load_draft = cx.spawn(async move |this, cx| {
            let body = match client.draft(&scope).await {
                Ok(serde_json::Value::Null) => return,
                Ok(value) => match serde_json::from_value::<DraftBody>(value) {
                    Ok(body) => body,
                    Err(error) => {
                        tracing::warn!(%scope, "ignored an unreadable draft: {error}");
                        return;
                    }
                },
                Err(error) => {
                    tracing::warn!(%scope, "couldn't load the draft: {error:#}");
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| this.restore(&target, body, cx));
        });
    }

    fn restore(&mut self, target: &ComposerTarget, body: DraftBody, cx: &mut Context<Self>) {
        // The user started typing before the draft arrived: theirs wins.
        if &self.target != target || self.has_text || !self.attachments.is_empty() {
            return;
        }
        self.editor
            .update(cx, |editor, cx| editor.set_text(&body.text, cx));
        self.has_text = !body.text.trim().is_empty();
        self.config = body.config;
        for hash in body.images {
            self.restore_attachment(hash, cx);
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

    /// The draft changed: a failed or sent message is now a new draft.
    fn edited(&mut self, cx: &mut Context<Self>) {
        if !self.send.state().is_sending() {
            let mut effects = Vec::new();
            self.send
                .handle_with_context(&send::Input::Edited, &mut effects);
            self.outgoing = None;
        }
        self.schedule_save(cx);
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DRAFT_SAVE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                this.save = None;
                this.save_now(cx);
            });
        }));
    }

    fn save_now(&mut self, cx: &mut Context<Self>) {
        // While an answer is written, the draft is the one put aside.
        let text = self
            .questionnaire
            .aside_text()
            .unwrap_or_else(|| self.editor.read(cx).text())
            .to_owned();
        let attachments = self
            .questionnaire
            .aside_attachments()
            .unwrap_or(&self.attachments);
        let body = DraftBody {
            text,
            images: attachments
                .iter()
                .filter_map(|attachment| attachment.machine.state().hash().cloned())
                .collect(),
            config: self.config.clone(),
        };
        let value = if body.text.is_empty() && body.images.is_empty() && body.config.is_none() {
            serde_json::Value::Null
        } else {
            serde_json::to_value(&body).unwrap_or_default()
        };
        self.put_draft(self.scope(), value, cx);
    }

    fn put_draft(&self, scope: String, value: serde_json::Value, cx: &mut Context<Self>) {
        let Some(host) = self.host(cx) else {
            return;
        };
        let client = host.read(cx).client.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = client.set_draft(&scope, &value).await {
                    tracing::warn!(%scope, "couldn't save the draft: {error:#}");
                }
            })
            .detach();
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

    fn choose_config(&mut self, config: EngineConfig, cx: &mut Context<Self>) {
        self.remember_config(&config, cx);
        self.config = Some(config);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Makes `config` what the next new thread on this host starts with.
    fn remember_config(&self, config: &EngineConfig, cx: &mut Context<Self>) {
        let Some(host) = self.host(cx) else {
            return;
        };
        let host = host.read(cx);
        let value = match serde_json::to_value(config) {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(host = %host.id, "couldn't encode the engine settings to remember: {error}");
                return;
            }
        };
        if host.setting(DEFAULT_CONFIG_KEY) == Some(&value) {
            return;
        }
        let (client, id) = (host.client.clone(), host.id.clone());
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = client.set_setting(DEFAULT_CONFIG_KEY, &value).await {
                    tracing::warn!(host = %id, "couldn't remember the engine settings: {error:#}");
                }
            })
            .detach();
    }

    fn project(&self) -> Option<ProjectId> {
        match &self.target {
            ComposerTarget::NewThread {
                project: Some(project),
                ..
            } => Some(project.clone()),
            _ => self.project.clone(),
        }
    }

    // Attachments.

    fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for path in paths {
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            self.add_image(Source::Path(path), name.into(), cx);
        }
    }

    fn add_image(&mut self, source: Source, name: SharedString, cx: &mut Context<Self>) {
        let key = self.next_key;
        self.next_key += 1;
        let task = cx.spawn(async move |this, cx| {
            let background = cx.background_executor().clone();
            let encoded = match background.spawn(async move { source.into_bytes() }).await {
                Ok(bytes) => {
                    let bytes = Arc::new(bytes);
                    let source = source_key(&bytes);
                    let kept = this
                        .update(cx, |this, cx| {
                            let duplicate = this
                                .attachments
                                .iter()
                                .find(|other| other.key != key && other.source == Some(source));
                            if let Some(existing) = duplicate.map(|other| other.key) {
                                this.drop_duplicate(key, existing, cx);
                                return false;
                            }
                            if let Some(attachment) = this.attachment_mut(key) {
                                attachment.source = Some(source);
                                attachment.draft = draft_image(&bytes);
                                cx.notify();
                            }
                            true
                        })
                        .unwrap_or(false);
                    if !kept {
                        return;
                    }
                    background
                        .spawn(async move { encode::to_webp(&bytes) })
                        .await
                }
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |this, cx| match encoded {
                Ok(webp) => {
                    // Another file or paste that encodes to the same image.
                    let duplicate = this.attachments.iter().find(|other| {
                        other.key != key
                            && other
                                .preview
                                .as_ref()
                                .is_some_and(|preview| preview.bytes == webp)
                    });
                    if let Some(existing) = duplicate.map(|other| other.key) {
                        this.drop_duplicate(key, existing, cx);
                        return;
                    }
                    if let Some(attachment) = this.attachment_mut(key) {
                        attachment.preview =
                            Some(Arc::new(Image::from_bytes(ImageFormat::Webp, webp)));
                        attachment.revealed = Some(Instant::now());
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
            });
        });
        self.attachments.push(Attachment {
            key,
            name,
            machine: attachment::Machine.state_machine(),
            preview: None,
            draft: None,
            revealed: None,
            source: None,
            task,
        });
        self.edited(cx);
        cx.notify();
    }

    fn restore_attachment(&mut self, hash: BlobHash, cx: &mut Context<Self>) {
        let key = self.next_key;
        self.next_key += 1;
        let mut machine = attachment::Machine.state_machine();
        machine.handle_with_context(&attachment::Input::Restored(hash.clone()), &mut Vec::new());
        let task = match self.host(cx) {
            Some(host) => {
                let client = host.read(cx).client.clone();
                cx.spawn(async move |this, cx| match client.blob(&hash).await {
                    Ok(bytes) => {
                        let _ = this.update(cx, |this, cx| {
                            if let Some(attachment) = this
                                .attachments
                                .iter_mut()
                                .find(|attachment| attachment.key == key)
                            {
                                attachment.preview =
                                    Some(Arc::new(Image::from_bytes(ImageFormat::Webp, bytes)));
                                attachment.revealed = Some(Instant::now());
                                cx.notify();
                            }
                        });
                    }
                    Err(error) => tracing::warn!(%hash, "couldn't fetch a draft image: {error:#}"),
                })
            }
            None => Task::ready(()),
        };
        self.attachments.push(Attachment {
            key,
            name: "Image".into(),
            machine,
            preview: None,
            draft: None,
            revealed: None,
            source: None,
            task,
        });
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
        let mut effects = Vec::new();
        attachment.machine.handle_with_context(input, &mut effects);
        if let Some(reason) = attachment.machine.state().failure() {
            tracing::warn!(name = %attachment.name, "attachment failed: {reason}");
        }
        for effect in effects {
            match effect {
                attachment::Effect::Upload => {
                    let (Some(client), Some(preview)) =
                        (client.clone(), attachment.preview.clone())
                    else {
                        continue;
                    };
                    attachment.task = cx.spawn(async move |this, cx| {
                        let input = match client.put_blob(preview.bytes.clone()).await {
                            Ok(blob) => attachment::Input::Uploaded(blob.hash),
                            Err(error) => {
                                attachment::Input::Failed(format!("upload failed: {error:#}"))
                            }
                        };
                        let _ = this.update(cx, |this, cx| this.drive_attachment(key, &input, cx));
                    });
                }
            }
        }
        if matches!(input, attachment::Input::Uploaded(_)) {
            self.schedule_save(cx);
        }
        cx.notify();
    }

    /// The image is already in the tray (master's duplicate check): drop
    /// the new tile and shake the one that holds it.
    fn drop_duplicate(&mut self, key: u64, existing: u64, cx: &mut Context<Self>) {
        if let Some(index) = self.attachments.iter().position(|a| a.key == key) {
            // Called from that attachment's own task: let it finish rather
            // than cancel it mid-poll.
            let attachment = self.attachments.remove(index);
            attachment.task.detach();
        }
        self.shake = Some((existing, Instant::now()));
        self.edited(cx);
        cx.notify();
    }

    /// How far into its duplicate shake the tile `key` is, while it shakes.
    fn shaking(&self, key: u64, reduce_motion: bool) -> Option<Duration> {
        let (shaken, since) = self.shake?;
        let elapsed = since.elapsed();
        (shaken == key && !reduce_motion && elapsed < SHAKE).then_some(elapsed)
    }

    fn attachment_mut(&mut self, key: u64) -> Option<&mut Attachment> {
        self.attachments
            .iter_mut()
            .find(|attachment| attachment.key == key)
    }

    fn remove_attachment(&mut self, key: u64, cx: &mut Context<Self>) {
        self.attachments.retain(|attachment| attachment.key != key);
        self.edited(cx);
        cx.notify();
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

    /// Why sending is not possible right now, if it isn't.
    fn blocked(&self, cx: &App) -> Option<&'static str> {
        if self.send.state().is_sending() {
            return Some("Sending…");
        }
        if !self.has_text && self.attachments.is_empty() {
            return Some("");
        }
        if self
            .attachments
            .iter()
            .any(|attachment| attachment.machine.state().failure().is_some())
        {
            return Some("Remove the images that failed");
        }
        if self
            .attachments
            .iter()
            .any(|attachment| attachment.machine.state().hash().is_none())
        {
            return Some("Preparing images…");
        }
        if matches!(self.target, ComposerTarget::NewThread { .. }) && self.project().is_none() {
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
        if self.send.state().retry_id().is_some() && self.outgoing.is_none() {
            self.send
                .handle_with_context(&send::Input::Edited, &mut Vec::new());
        }
        let id = if let (Some(id), Some(_)) = (self.send.state().retry_id(), &self.outgoing) {
            id.clone()
        } else {
            let Some(outgoing) = self.outgoing_message(cx) else {
                return;
            };
            if let Some(config) = self.effective_config(cx) {
                self.remember_config(&config, cx);
            }
            let id = outgoing.message.id.clone();
            self.outgoing = Some(outgoing);
            id
        };
        let mut effects = Vec::new();
        self.send
            .handle_with_context(&send::Input::Send(id), &mut effects);
        self.perform(effects, cx);
    }

    fn outgoing_message(&self, cx: &App) -> Option<Outgoing> {
        let config = self.effective_config(cx)?;
        let text = self.editor.read(cx).text().trim().to_owned();
        let images = self
            .attachments
            .iter()
            .filter_map(|attachment| attachment.machine.state().hash().cloned())
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
                    project_id: self.project()?,
                    config: config.clone(),
                    title: None,
                    parent_thread_id: None,
                };
                (Some(create), Some(config.clone()))
            }
        };
        let config = (thread_config.as_ref() != Some(&config)).then_some(config);
        Some(Outgoing {
            message: SendMessage {
                id: ItemId::new(),
                text,
                images,
                config,
            },
            create,
            scope: self.scope(),
        })
    }

    fn perform(&mut self, effects: Vec<send::Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                send::Effect::Post(_) => self.post(cx),
                send::Effect::Clear => self.clear_sent(cx),
            }
        }
        cx.notify();
    }

    fn post(&mut self, cx: &mut Context<Self>) {
        let (Some(host), Some(outgoing)) = (self.host(cx), self.outgoing.as_ref()) else {
            return;
        };
        let client = host.read(cx).client.clone();
        let message = outgoing.message.clone();
        let create = outgoing.create.clone();
        let thread = match (&self.target, &create) {
            (_, Some(create)) => create.id.clone(),
            (ComposerTarget::Thread { thread, .. }, None) => thread.clone(),
            (ComposerTarget::NewThread { .. }, None) => return,
        };
        self.editor
            .update(cx, |editor, cx| editor.set_disabled(true, cx));
        self.notice = None;
        cx.spawn(async move |this, cx| {
            if let Some(create) = create {
                if let Err(error) = client.create_thread(&create).await {
                    let error = format!("Couldn't create the thread: {error:#}");
                    let _ = this.update(cx, |this, cx| this.finish(&message.id, Err(error), cx));
                    return;
                }
                let _ = this.update(cx, |this, cx| this.created(&message.id, &thread, cx));
            }
            let sent = client
                .send_message(&thread, &message)
                .await
                .context("Couldn't send the message");
            let _ = this.update(cx, |this, cx| {
                this.finish(
                    &message.id,
                    sent.map(drop).map_err(|error| format!("{error:#}")),
                    cx,
                );
            });
        })
        .detach();
    }

    /// Forge made the thread; the composer now belongs to it, so a retry
    /// sends there instead of creating another.
    fn created(&mut self, id: &ItemId, thread: &ThreadId, cx: &mut Context<Self>) {
        // The user moved on to another target meanwhile.
        if !self.send.state().is_sending_id(id) {
            return;
        }
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.create = None;
        }
        let host = self.target.host().clone();
        self.target = ComposerTarget::Thread {
            host: host.clone(),
            thread: thread.clone(),
        };
        self.project = None;
        self.open_target(cx);
        self.core.update(cx, |core, cx| {
            core.navigate(
                Screen::Thread {
                    host,
                    thread: thread.clone(),
                },
                cx,
            );
        });
        cx.notify();
    }

    fn finish(&mut self, id: &ItemId, result: Result<(), String>, cx: &mut Context<Self>) {
        if !self.send.state().is_sending_id(id) {
            return;
        }
        self.editor
            .update(cx, |editor, cx| editor.set_disabled(false, cx));
        let input = match result {
            Ok(()) => send::Input::Accepted,
            Err(error) => {
                tracing::warn!(target = ?self.target, "send failed: {error}");
                send::Input::Rejected(error)
            }
        };
        let mut effects = Vec::new();
        self.send.handle_with_context(&input, &mut effects);
        self.perform(effects, cx);
    }

    fn clear_sent(&mut self, cx: &mut Context<Self>) {
        let Some(outgoing) = self.outgoing.take() else {
            return;
        };
        self.save = None;
        self.editor.update(cx, |editor, cx| editor.set_text("", cx));
        self.has_text = false;
        self.attachments.clear();
        self.put_draft(outgoing.scope.clone(), serde_json::Value::Null, cx);
        if outgoing.scope != self.scope() && self.config.is_some() {
            // The new thread carries the choice now.
            self.config = None;
        }
        if let ComposerTarget::Thread { thread, .. } = &self.target {
            cx.emit(ComposerEvent::Sent {
                thread: thread.clone(),
                item: outgoing.message.id,
            });
        }
    }

    /// Stops the thread's live run.
    fn stop(&mut self, cx: &mut Context<Self>) {
        let (ComposerTarget::Thread { thread, .. }, Some(host)) = (&self.target, self.host(cx))
        else {
            return;
        };
        let client = host.read(cx).client.clone();
        let thread = thread.clone();
        cx.spawn(async move |this, cx| {
            if let Err(error) = client.cancel(&thread).await {
                let _ = this.update(cx, |this, cx| {
                    this.notice = Some(format!("Couldn't stop the run: {error:#}"));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn sign_in(&mut self, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.core.update(cx, |core, cx| {
            core.navigate(Screen::Settings(editor_core::SettingsPage::Engines), cx);
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
        let model = status.models.iter().find(|model| &model.id == id)?;
        if config
            .effort
            .as_ref()
            .is_some_and(|effort| !model.efforts.contains(effort))
        {
            config.effort.clone_from(&model.default_effort);
        }
        if config
            .speed
            .as_ref()
            .is_some_and(|speed| !model.speeds.iter().any(|offered| &offered.id == speed))
        {
            config.speed = None;
        }
        if config
            .variant
            .as_ref()
            .is_some_and(|variant| !model.variants.iter().any(|offered| &offered.id == variant))
        {
            config.variant = None;
        }
    }
    Some(config)
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
            Self::Bytes(bytes) => Ok(bytes),
            Self::Path(path) => {
                std::fs::read(&path).with_context(|| format!("read {}", path.display()))
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

/// Duplicate-attachment shake (master's, the transitions.dev error shake):
/// two 80 ms swings and two 60 ms recovery legs.
const SHAKE: Duration = Duration::from_millis(280);

/// The tile's horizontal offset `elapsed` into the shake: rest, +6 px,
/// -6 px, +4 px, rest, each leg on its own smooth-out curve.
fn shake_offset(elapsed: Duration) -> f32 {
    const STOPS: [(f32, f32); 5] = [
        (0.0, 0.0),
        (80.0 / 280.0, 6.0),
        (160.0 / 280.0, -6.0),
        (220.0 / 280.0, 4.0),
        (1.0, 0.0),
    ];
    let progress = (elapsed.as_secs_f32() / SHAKE.as_secs_f32()).clamp(0.0, 1.0);
    STOPS
        .windows(2)
        .find(|leg| progress <= leg[1].0)
        .map_or(0.0, |leg| {
            let ((start, from), (end, to)) = (leg[0], leg[1]);
            from + (to - from) * smooth_out((progress - start) / (end - start))
        })
}

/// The picture as given, for the blurred tile while it encodes; formats
/// GPUI can't draw keep the plain tile.
fn draft_image(bytes: &[u8]) -> Option<Arc<Image>> {
    let format = match image::guess_format(bytes).ok()? {
        image::ImageFormat::Png => ImageFormat::Png,
        image::ImageFormat::Jpeg => ImageFormat::Jpeg,
        image::ImageFormat::WebP => ImageFormat::Webp,
        image::ImageFormat::Gif => ImageFormat::Gif,
        image::ImageFormat::Bmp => ImageFormat::Bmp,
        _ => return None,
    };
    Some(Arc::new(Image::from_bytes(format, bytes.to_vec())))
}

impl Attachment {
    /// The best picture to open in the preview: the WebP, else the draft.
    fn viewable(&self) -> Option<Arc<Image>> {
        self.preview.clone().or_else(|| self.draft.clone())
    }

    /// The tile's picture: the draft blurred while it encodes, then the
    /// preview sharpening out of that blur (smooth-out over 350 ms). The
    /// flag says it is still sharpening.
    fn picture(&self, radius: Pixels, reduce_motion: bool) -> Option<(AnyElement, bool)> {
        let Some(preview) = self.preview.clone() else {
            let draft = self.draft.clone()?;
            return Some((
                blurred_tile_image(draft, TILE_BLUR, radius).into_any_element(),
                false,
            ));
        };
        let reveal = self
            .revealed
            .filter(|_| !reduce_motion)
            .map(|since| {
                since.elapsed().as_secs_f32() / MotionDuration::Medium.as_duration().as_secs_f32()
            })
            .filter(|progress| *progress < 1.0);
        Some(match reveal {
            Some(progress) => (
                blurred_tile_image(preview, TILE_BLUR * (1.0 - smooth_out(progress)), radius)
                    .into_any_element(),
                true,
            ),
            None => (
                img(preview)
                    .size_full()
                    .rounded(radius)
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                false,
            ),
        })
    }
}

/// How soft a tile is while its image prepares.
const TILE_BLUR: f32 = 10.0;

/// An image filling a tile with the tile's corner, blurred by `blur`. It
/// rounds itself: the tile's clip is square (GPUI clips to rectangles), so
/// an image overhanging it showed square corners until it sharpened.
fn blurred_tile_image(image: Arc<Image>, blur: f32, corner: Pixels) -> gpui::Div {
    div().absolute().inset_0().blur(px(blur)).child(
        img(image)
            .size_full()
            .rounded(corner)
            .object_fit(ObjectFit::Cover),
    )
}

// Rendering.

/// The composer card's corner (`NATIVE_COMPOSER_RADIUS_PX`).
const CARD_RADIUS: f32 = RADIUS;
/// 64 px editor, 32 px controls and 8 px padding on each edge.
const CARD_MIN_HEIGHT: f32 = 112.0;

impl Composer {
    /// The attachment tray (`attachment_tray`): 72 px card tiles with a
    /// corner concentric to the composer card, each with its remove chip.
    fn render_attachments(
        &self,
        theme: &ArtisanTheme,
        desktop: DesktopTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
        // The 18 px card radius less its 8 px padding and the 4 px inset.
        let tile_radius = RadiusTokens::nested(px(CARD_RADIUS), px(8.0 + 4.0));
        let reduce_motion = cx.reduce_motion();
        let mut animating = false;
        for attachment in &self.attachments {
            let key = attachment.key;
            let state = attachment.machine.state();
            let mut tile = div()
                .id(SharedString::from(format!("composer-attachment-{key}")))
                .relative()
                .size(px(TILE))
                .flex_none()
                .overflow_hidden()
                .rounded(tile_radius)
                .shadow(card_shadows(theme))
                .bg(desktop.field)
                .cursor_pointer()
                .when_some(attachment.viewable(), |tile, image| {
                    tile.on_click(editor_ui::image_preview::open_on_click(image))
                });
            if let Some(elapsed) = self.shaking(key, reduce_motion) {
                animating = true;
                tile = tile.left(px(shake_offset(elapsed)));
            }
            tile = match (&attachment.preview, state.failure()) {
                (_, Some(reason)) => tile.border_1().border_color(destructive()).child(
                    div()
                        .size_full()
                        .p(px(6.0))
                        .text_size(px(11.0))
                        .text_color(destructive())
                        .child(SharedString::from(format!(
                            "{} failed: {reason}",
                            attachment.name
                        ))),
                ),
                (_, None)
                    if let Some((picture, sharpening)) =
                        attachment.picture(tile_radius, reduce_motion) =>
                {
                    animating |= sharpening;
                    tile.child(picture)
                }
                (_, None) => tile.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px(px(5.0))
                        .text_size(px(11.0))
                        .text_color(desktop.secondary)
                        .child("Preparing…"),
                ),
            };
            let remove = div()
                .id(SharedString::from(format!(
                    "composer-attachment-remove-{key}"
                )))
                .absolute()
                .top(px(3.2))
                .right(px(3.2))
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(desktop.chrome.opacity(0.9))
                .text_color(desktop.foreground)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.remove_attachment(key, cx);
                }))
                .child(asset_glyph(AssetId::TABLER_X).size(px(14.0)));
            row = row.child(tile.child(remove));
        }
        if animating {
            // A paint-time probe keeps frames coming while a tile sharpens.
            row = row.child(
                canvas(
                    |_, _, _| {},
                    |_, (), window, _| window.request_animation_frame(),
                )
                .absolute()
                .size_0(),
            );
        }
        row
    }

    /// The primary control (`render_control_row`): a ghost icon button that
    /// sends (or steers a live run) with the arrow, and stops the run with
    /// the filled square while there is nothing to send.
    fn render_primary(&self, theme: &ArtisanTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme;
        let working = self.is_working(cx);
        let ready = if self.writing_answer() {
            self.has_text
        } else {
            self.blocked(cx).is_none()
        };
        let stop_face = working && !ready;
        let (icon, label) = if stop_face {
            (AssetId::TABLER_PLAYER_STOP_FILLED, "Stop")
        } else {
            (AssetId::TABLER_ARROW_UP, "Send message")
        };
        let entity = cx.entity().downgrade();
        let button = AccessibleLabel::new(label).ok().and_then(|label| {
            Button::new(
                if stop_face {
                    "composer-stop"
                } else {
                    "composer-send"
                },
                self.button_focus.clone(),
                theme,
                MotionPolicy::Reduced,
                ButtonVariant::Ghost,
                ButtonSize::IconSmall,
                ButtonContent::icon_only(icon, label),
            )
            .ok()
        });
        let mut right = div().flex().flex_row().items_center().gap(px(4.0));
        if let Some(button) = button {
            right = right.child(
                button
                    .corner_radius(px(10.0))
                    .disabled(!stop_face && !ready)
                    .on_activate(move |_, _, cx| {
                        let _ = entity.update(cx, |this, cx| {
                            if stop_face {
                                this.stop(cx);
                            } else {
                                this.send(cx);
                            }
                        });
                    }),
            );
        }
        right
    }
}

fn destructive() -> gpui::Hsla {
    ArtisanTheme::for_mode(ThemeMode::Dark)
        .colors
        .destructive
        .to_paint()
}

impl Render for Composer {
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder stacks the card, its tab, the error line and the picker tooltip in visual order"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let desktop = DesktopTheme::neutral_dark();
        self.viewport_width = f32::from(window.viewport_size().width);
        self.viewport_height = f32::from(window.viewport_size().height);
        // Each time the draft empties again, the next phrase shows.
        let placeholder_visible = !self.has_text && self.attachments.is_empty();
        if let Some(answer) = self.answer_placeholder() {
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
        let error = self
            .send
            .state()
            .error()
            .map(str::to_owned)
            .or_else(|| self.notice.clone());

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
            .text_color(desktop.secondary)
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
            .id("composer")
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .min_h(px(CARD_MIN_HEIGHT))
            .p(px(8.0))
            .rounded(px(CARD_RADIUS))
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(&theme))
            .shadow(glass_card_shadows())
            .child(glass_material_layer(GlassStrength::Quiet, px(CARD_RADIUS)))
            .child(glass_highlight_layer(GlassStrength::Quiet, px(CARD_RADIUS)))
            .on_drop::<ExternalPaths>(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.add_paths(paths.paths().to_vec(), cx);
            }));
        // Open agent questions extend the card upward, above the editor.
        if let Some(panel) = self.render_questionnaires(desktop, cx) {
            card = card.child(panel);
        }
        if !self.attachments.is_empty() {
            card = card.child(self.render_attachments(&theme, desktop, cx));
        }
        card = card.child(self.editor.clone());
        // The mention menu hangs above the card at its full width.
        if let Some(menu) = self.render_mention_menu(window, desktop, cx) {
            card = card.child(menu);
        }
        card = card.child(controls);

        // The tab frames only the card; the goal grows out of it.
        let card = match self.render_tab(&theme, desktop, cx) {
            Some(tab) => {
                let mut moving = false;
                let strip = self
                    .goal
                    .frame(std::time::Instant::now(), desktop, &mut moving)
                    .map(|frame| TabCardStrip {
                        reveal: frame.reveal,
                        content: frame.content,
                    });
                if moving {
                    window.request_animation_frame();
                }
                tab_card_with_strip(tab, strip, card, px(CARD_RADIUS)).into_any_element()
            }
            None => card.into_any_element(),
        };
        let mut shell = div().w_full().flex().flex_col().gap(px(8.0));
        if let Some(error) = error {
            shell = shell.child(
                div()
                    .px(px(12.0))
                    .text_size(px(13.0))
                    .text_color(destructive())
                    .child(error),
            );
        }
        shell = shell.child(card);
        if let Some(tooltip) = self.render_tooltip(&theme, window.viewport_size()) {
            shell = shell.child(tooltip);
        }
        shell
    }
}

#[cfg(test)]
mod tests;
