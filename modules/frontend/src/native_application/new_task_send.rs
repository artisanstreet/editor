//! Sending from the new-thread screen: the project's new-task draft.
//!
//! No thread exists yet on the new-thread screen; the composer shows the
//! selected project's Forge draft (`ComposerDraftScope::Project`). Send
//! submits that draft through the same draft-send flow as a thread's (save
//! first, per-scope holds, a repeat after a lost answer names the same
//! revision): the Forge creates the thread and queues its first message in
//! one transaction and answers the thread it created, even for a repeated
//! revision. The Editor then opens that thread, whose subscription brings
//! the message's outbox row.
//!
//! Send is never a silent no-op: without a thread or project to send to, the
//! composer says why and keeps the draft.

use artisan_domain::ComposerDraftScope;

use super::*;

impl NativeApplication {
    /// The draft Send submits: the open thread's, or on the new-thread screen
    /// the selected draft thread's (a task created before its first message)
    /// or else the selected project's new-task draft. `None` when nothing can
    /// be sent right now; a thread with messages is never sent to from the
    /// new-thread screen.
    pub(super) fn submission_scope(&self, cx: &App) -> Option<ComposerDraftScope> {
        if self.host_switch_pending() {
            return None;
        }
        match self.route() {
            NativeRoute::Thread { project, thread } => (self.selected_project.as_ref()
                == Some(project)
                && self.selected_thread.as_ref() == Some(thread)
                && self.message_composer_visible(cx))
            .then(|| ComposerDraftScope::Thread(thread.clone())),
            NativeRoute::NewThread { .. } => match &self.selected_thread {
                Some(thread) => (self.selected_thread_is_draft()
                    && self.message_composer_visible(cx))
                .then(|| ComposerDraftScope::Thread(thread.clone())),
                None if self.new_task_composer_ready() => self
                    .selected_project
                    .clone()
                    .map(ComposerDraftScope::Project),
                None => None,
            },
            _ => None,
        }
    }

    /// Whether the new-thread screen's composer can send its project draft:
    /// the project is listed without an open or opening thread, and the
    /// connection takes commands.
    fn new_task_composer_ready(&self) -> bool {
        matches!(
            self.state,
            NativeViewState::EmptyThreads | NativeViewState::Ready
        ) && self.selected_thread.is_none()
            && self.pending_thread.is_none()
            && self.intake_stage.is_none()
            && self.thread_switch_flight.is_none()
            && self.ordinary_unsubscribe_thread.is_none()
            && self.command_submission_is_available()
            && !self.service_stopped
    }

    /// The new-thread screen with no project to send to: Send stays
    /// available so pressing it says why instead of doing nothing.
    pub(super) fn new_task_lacks_project(&self) -> bool {
        matches!(self.route(), NativeRoute::NewThread { .. })
            && self.selected_project.is_none()
            && self.selected_thread.is_none()
            && matches!(
                self.state,
                NativeViewState::EmptyProjects | NativeViewState::EmptyThreads
            )
            && !self.service_stopped
            && !self.host_switch_pending()
    }

    /// Binds the composer to the project's new-task draft before its send,
    /// keeping what it shows: a view that never opened the project's draft
    /// (or still showed another scope) carries its text into it.
    pub(super) fn bind_new_task_composer(&mut self, project: &ProjectId, cx: &mut Context<Self>) {
        let scope = ComposerDraftScope::Project(project.clone());
        if self.composer.read(cx).draft_scope().as_ref() == Some(&scope) {
            return;
        }
        let key = format!("project:{}", project.as_str());
        self.composer
            .update(cx, |composer, cx| composer.switch_thread(&key, true, cx));
    }

    /// Says why Send had nothing to send to; the draft stays.
    pub(super) fn refuse_unscoped_send(&mut self, cx: &mut Context<Self>) {
        let note = if self.selected_project.is_none() && self.selected_thread.is_none() {
            "Choose a project for this task before sending. Your draft is preserved."
        } else {
            "This task is still opening. Your draft is preserved; send again in a moment."
        };
        self.message_failure = Some(NativeMessageFailure::new(ServiceFailure {
            stage: ServiceFailureStage::Request,
            category: ServiceFailureCategory::InvalidConfiguration,
        }));
        self.message_failure_note = Some(note.to_owned());
        self.sync_composer_controls(cx);
        cx.notify();
    }

    /// Opens the thread an accepted send was queued in when the view is not
    /// on it yet: the thread a new-task send created (entering it like a
    /// chosen recent thread, which lists the project again first), or a
    /// draft thread sent from the new-thread screen. Anything typed after
    /// Send moves along with the composer.
    pub(super) fn open_sent_thread(
        &mut self,
        scope: &ComposerDraftScope,
        thread: ThreadId,
        cx: &mut Context<Self>,
    ) {
        match scope {
            ComposerDraftScope::Project(project) => {
                self.project_navigation.restore_draft = false;
                self.open_recent_thread(project.clone(), thread, cx);
            }
            ComposerDraftScope::Thread(_) => {
                if let (NativeRoute::NewThread { .. }, Some(project)) =
                    (self.route(), self.selected_project.clone())
                {
                    self.navigate(NativeRoute::Thread { project, thread }, cx);
                }
            }
        }
    }
}
