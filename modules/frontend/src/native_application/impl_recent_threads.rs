//! The recent threads the sidebar shows, and the selected project's thread
//! listing kept current from them.
//!
//! The Forge serves the recent threads across every project when the
//! connection starts (and again after a reconnect) and pushes every change;
//! the Editor keeps the latest list and renders it. Nothing here polls. When
//! a pushed list shows a thread of the selected project that the project's
//! listing does not show as it is (a new thread, a new title, a newer
//! message), that listing is read again once, so the thread picker, the
//! command menu, and opening a thread stay on the Forge's data.

use artisan_domain::{RecentThread, RecentThreadListing};

use super::*;

impl NativeApplication {
    /// Applies the recent threads the Forge served.
    pub(super) fn receive_recent_threads(
        &mut self,
        result: Result<RecentThreadListing, ServiceFailure>,
        cx: &mut Context<Self>,
    ) {
        // A failed read keeps the last list; the next push replaces it.
        if let Ok(listing) = result {
            self.apply_recent_threads(listing, cx);
        }
    }

    /// Applies the recent threads the Forge served or pushed.
    pub(super) fn apply_recent_threads(
        &mut self,
        listing: RecentThreadListing,
        cx: &mut Context<Self>,
    ) {
        let threads = &mut self.sidebar_threads;
        if threads.recent.as_ref() != Some(&listing) {
            threads.recent = Some(listing);
            threads.recent_revision = threads.recent_revision.wrapping_add(1);
            self.refresh_project_threads_if_stale();
        }
        cx.notify();
    }

    /// A reconnected connection asks for the recent threads again, which
    /// also resumes their pushes, and sends the drafts it parked.
    pub(super) fn resume_after_reconnect(&mut self) {
        self.resume_composer_drafts();
        let _ = self.submit_command(NativeTransportCommand::ReadRecentThreads);
    }

    /// Opens a recent thread in its own project: at once when the selected
    /// project lists it, after one read of the listing when the thread is
    /// newer than the listing, and by entering its project otherwise.
    pub(super) fn open_recent_thread(
        &mut self,
        project: ProjectId,
        thread: ThreadId,
        cx: &mut Context<Self>,
    ) {
        if self.selected_project.as_ref() != Some(&project) {
            self.enter_project(project, true, Some(thread), cx);
        } else if self.thread_is_listed(&thread) {
            self.open_listed_thread(thread, cx);
        } else {
            self.sidebar_threads.open_after_refresh = Some(thread);
            self.refresh_project_threads();
        }
    }

    /// Reads the selected project's listing again when the latest recent
    /// threads show one of its threads differently than the listing does,
    /// once per pushed list.
    pub(super) fn refresh_project_threads_if_stale(&mut self) {
        let threads = &self.sidebar_threads;
        if threads.refreshed_revision == threads.recent_revision || threads.pending.is_some() {
            return;
        }
        let stale = match (
            threads.recent.as_ref(),
            self.selected_project.as_ref(),
            self.thread_listing.as_ref(),
        ) {
            (Some(recent), Some(project), Some(listing)) => {
                listing_is_stale(recent.threads(), project, listing)
            }
            _ => false,
        };
        if !stale {
            self.sidebar_threads.refreshed_revision = self.sidebar_threads.recent_revision;
            return;
        }
        if self.refresh_project_threads() {
            self.sidebar_threads.refreshed_revision = self.sidebar_threads.recent_revision;
        }
    }

    /// Reads the selected project's listing without selecting it or
    /// mounting a conversation; true once the read is on its way.
    pub(super) fn refresh_project_threads(&mut self) -> bool {
        if self.service_stopped
            || self.shutdown_prepared
            || self.thread_switch_flight.is_some()
            || self.intake_stage.is_some()
            || self.thread_listing.is_none()
            || self.sidebar_threads.pending.is_some()
        {
            return false;
        }
        let Some(project_id) = self.selected_project.clone() else {
            return false;
        };
        let Some(generation) = self.sidebar_threads.generation.checked_add(1) else {
            return false;
        };
        self.sidebar_threads.generation = generation;
        let submitted = self
            .submit_command(NativeTransportCommand::RefreshThreads {
                project_id: project_id.clone(),
                generation,
            })
            .is_ok();
        if submitted {
            self.sidebar_threads.pending = Some((project_id, generation));
        }
        submitted
    }

    pub(super) fn receive_refreshed_threads(
        &mut self,
        project_id: &ProjectId,
        generation: u64,
        result: Result<ThreadListing, ServiceFailure>,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_threads.pending.as_ref() != Some(&(project_id.clone(), generation)) {
            return;
        }
        self.sidebar_threads.pending = None;
        let open_after = self.sidebar_threads.open_after_refresh.take();
        if self.selected_project.as_ref() != Some(project_id)
            || self.thread_switch_flight.is_some()
            || self.intake_stage.is_some()
        {
            return;
        }
        let Ok(listing) = result else {
            return;
        };
        if listing
            .threads()
            .iter()
            .any(|thread| &thread.project_id != project_id)
        {
            return;
        }
        if self.selected_thread.as_ref().is_some_and(|selected| {
            !listing
                .threads()
                .iter()
                .any(|thread| &thread.thread_id == selected)
        }) {
            self.handle_threads(project_id, &listing, cx);
            return;
        }
        if self.thread_listing.as_ref() != Some(&listing) {
            // Background reads must not auto-open a thread or disturb an
            // in-flight snapshot, draft, route, or selected conversation.
            self.thread_listing = Some(listing.clone());
            self.update_thread_picker(listing, self.selected_thread.clone(), cx);
            self.sync_command_menu_groups(cx);
            cx.notify();
        }
        if let Some(thread) = open_after.filter(|thread| self.thread_is_listed(thread)) {
            self.open_listed_thread(thread, cx);
        }
    }
}

/// Whether the recent threads show a thread of `project` that `listing`
/// lacks or shows differently.
fn listing_is_stale(recent: &[RecentThread], project: &ProjectId, listing: &ThreadListing) -> bool {
    recent
        .iter()
        .filter(|row| &row.thread.project_id == project)
        .any(|row| !listing.threads().iter().any(|listed| listed == &row.thread))
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::{DisplayName, ThreadSummary, ThreadTitle, UnixMillis};

    fn summary(id: &str, project: &str, title: &str) -> ThreadSummary {
        ThreadSummary {
            has_started_response: true,
            has_active_work: false,
            last_message_at: Some(UnixMillis::from_millis(10)),
            thread_id: ThreadId::parse(id).unwrap(),
            project_id: ProjectId::parse(project).unwrap(),
            title: ThreadTitle::parse(title).unwrap(),
            created_at: UnixMillis::EPOCH,
            updated_at: UnixMillis::EPOCH,
        }
    }

    fn recent(thread: ThreadSummary) -> RecentThread {
        RecentThread {
            thread,
            subtitle: DisplayName::parse("owner/repo").unwrap(),
        }
    }

    #[test]
    fn only_the_selected_projects_changed_rows_make_its_listing_stale() {
        let project = ProjectId::parse("selected").unwrap();
        let listed = summary("listed", "selected", "Listed");
        let listing = ThreadListing::new(vec![listed.clone()]).unwrap();
        let other = recent(summary("elsewhere", "other", "Elsewhere"));
        assert!(!listing_is_stale(
            &[recent(listed.clone()), other.clone()],
            &project,
            &listing
        ));
        let retitled = recent(summary("listed", "selected", "Refined"));
        assert!(listing_is_stale(&[retitled], &project, &listing));
        let new_thread = recent(summary("new", "selected", "New"));
        assert!(listing_is_stale(&[new_thread, other], &project, &listing));
    }
}
