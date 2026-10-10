//! The reader's changes to a thread: settling it into History, archiving
//! it, and restoring it from Settings.
//!
//! A change shows at once: the sidebar row moves or leaves before
//! the Forge answers, and the Forge's next recent-threads push confirms it.
//! If the Forge refuses, a notice says so and the listing is read again, so
//! the sidebar never keeps a change that did not happen. Each answer carries
//! the archived threads, which Settings shows with when each is erased.

use super::*;
use crate::native_transport_service::{ThreadLifecycleCommand, ThreadLifecycleEvent};
use crate::window_notice::NoticeSeverity;
use artisan_domain::{
    ArchivedThreadListing, RecentThreadListing, ThreadChange, ThreadPlacement, UpdateThread,
};

#[derive(Default)]
pub(super) struct ThreadLifecycleState {
    /// The archived threads as the Forge last answered them.
    archived: Option<ArchivedThreadListing>,
    /// Whether the open Settings page asked for the archive.
    archived_requested: bool,
    archive_notice: Option<String>,
    /// Changes sent and not yet answered, with the thread's title as it
    /// read when sent, for a notice after the row has left.
    pending: HashMap<RequestId, (UpdateThread, String)>,
}

impl NativeApplication {
    /// Sends one change and shows it in the sidebar at once.
    pub(super) fn change_thread(
        &mut self,
        thread: ThreadId,
        change: ThreadChange,
        cx: &mut Context<Self>,
    ) {
        let Ok(request_id) = mint_request_id("thread-change") else {
            return;
        };
        let update = UpdateThread {
            request_id: request_id.clone(),
            thread_id: thread,
            change,
        };
        let command = ThreadLifecycleCommand::Update(update.clone());
        if self
            .submit_command(NativeTransportCommand::ThreadLifecycle(command))
            .is_err()
        {
            self.show_notice(
                NoticeSeverity::Warning,
                "The Editor is not connected to a Forge.".to_owned(),
                cx,
            );
            return;
        }
        let title = self.thread_title(&update.thread_id);
        self.show_thread_change(&update, cx);
        self.thread_lifecycle
            .pending
            .insert(request_id, (update, title));
        self.refresh_settings_archive(cx);
    }

    /// Reflects a sent change before the Forge confirms it. Archiving the
    /// open thread leaves it for its project's new thread.
    fn show_thread_change(&mut self, update: &UpdateThread, cx: &mut Context<Self>) {
        let thread = &update.thread_id;
        if update.change == ThreadChange::Archive
            && let NativeRoute::Thread {
                project,
                thread: open,
            }
            | NativeRoute::Editor {
                project,
                thread: open,
                ..
            } = self.route()
            && open == thread
        {
            let project = Some(project.clone());
            self.navigate(NativeRoute::NewThread { project }, cx);
        }
        if update.change == ThreadChange::Unarchive
            && let Some(archived) = &self.thread_lifecycle.archived
        {
            let rows = archived
                .threads()
                .iter()
                .filter(|row| &row.thread.thread.thread_id != thread)
                .cloned()
                .collect();
            self.thread_lifecycle.archived = ArchivedThreadListing::new(rows).ok();
        }
        let Some(listing) = &self.sidebar_threads.recent else {
            return;
        };
        let rows = listing
            .threads()
            .iter()
            .filter(|row| update.change != ThreadChange::Archive || &row.thread.thread_id != thread)
            .cloned()
            .map(|mut row| {
                if &row.thread.thread_id == thread {
                    match &update.change {
                        ThreadChange::Rename(title) => row.thread.title = title.clone(),
                        ThreadChange::Settle => row.placement = ThreadPlacement::History,
                        ThreadChange::Archive | ThreadChange::Unarchive => {}
                    }
                }
                row
            })
            .collect();
        if let Ok(listing) = RecentThreadListing::new(rows) {
            self.sidebar_threads.recent = Some(listing);
            self.sidebar_threads.recent_revision += 1;
        }
        cx.notify();
    }

    /// Serves the Forge's answers.
    pub(super) fn receive_thread_lifecycle(
        &mut self,
        event: ThreadLifecycleEvent,
        cx: &mut Context<Self>,
    ) {
        let state = &mut self.thread_lifecycle;
        match event {
            ThreadLifecycleEvent::Archived(Ok(listing)) => {
                state.archived = Some(*listing);
                state.archive_notice = None;
            }
            ThreadLifecycleEvent::Archived(Err(_)) => {
                state.archived_requested = false;
                state.archive_notice = Some(
                    "The Forge could not list your archived threads. Reopen this page to retry."
                        .to_owned(),
                );
            }
            ThreadLifecycleEvent::Updated { update, result } => {
                let sent_title = state
                    .pending
                    .remove(&update.request_id)
                    .map(|(_, title)| title);
                if let Ok(listing) = result {
                    state.archived = Some(*listing);
                } else {
                    let title = sent_title.unwrap_or_else(|| self.thread_title(&update.thread_id));
                    let verb = match update.change {
                        ThreadChange::Rename(_) => "rename",
                        ThreadChange::Settle => "settle",
                        ThreadChange::Archive => "archive",
                        ThreadChange::Unarchive => "restore",
                    };
                    self.show_notice(
                        NoticeSeverity::Warning,
                        format!("Couldn't {verb} {title}. Try again."),
                        cx,
                    );
                    // The listing shown may hold the refused change.
                    let _ = self.submit_command(NativeTransportCommand::ReadRecentThreads);
                    self.thread_lifecycle.archived_requested = false;
                }
            }
        }
        self.refresh_settings_archive(cx);
        cx.notify();
    }

    /// Reads the archive again for a freshly mounted Settings page.
    pub(super) fn reload_settings_archive(&mut self, cx: &mut Context<Self>) {
        self.thread_lifecycle.archived_requested = false;
        self.refresh_settings_archive(cx);
    }

    /// Pushes the archive into a mounted Threads settings page, reading it
    /// first if the page has not asked yet.
    pub(super) fn refresh_settings_archive(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.settings_screen_key, Some((SettingsRoute::Threads, _))) {
            return;
        }
        let Some(screen) = self.settings_screen.clone() else {
            return;
        };
        let state = &mut self.thread_lifecycle;
        if !state.archived_requested {
            state.archived_requested = true;
            let command = ThreadLifecycleCommand::ReadArchived;
            if self
                .submit_command(NativeTransportCommand::ThreadLifecycle(command))
                .is_err()
            {
                self.thread_lifecycle.archive_notice =
                    Some("The Editor is not connected to a Forge.".to_owned());
            }
        }
        let state = &self.thread_lifecycle;
        let listing = state.archived.clone();
        let restoring = state
            .pending
            .values()
            .filter(|(update, _)| update.change == ThreadChange::Unarchive)
            .map(|(update, _)| update.thread_id.clone())
            .collect();
        let notice = state.archive_notice.clone();
        screen.update(cx, |screen, screen_cx| {
            screen.set_archived_threads(listing, restoring, notice, screen_cx);
        });
    }

    /// The thread's title in quotes, as notices name it.
    fn thread_title(&self, thread: &ThreadId) -> String {
        let listed = self
            .sidebar_threads
            .recent
            .iter()
            .flat_map(RecentThreadListing::threads)
            .map(|row| &row.thread)
            .chain(
                self.thread_lifecycle
                    .archived
                    .iter()
                    .flat_map(ArchivedThreadListing::threads)
                    .map(|row| &row.thread.thread),
            )
            .find(|summary| &summary.thread_id == thread);
        listed.map_or_else(
            || "this thread".to_owned(),
            |summary| format!("\u{201c}{}\u{201d}", summary.title.as_str()),
        )
    }
}
