//! Connection-scoped state the Forge pushes over the delivery stream.
//!
//! The Forge pushes each engine's usage with its readiness verdict, the
//! user's preferences, the subscribed thread's display title, live run
//! usage and live thinking, the recent threads across every project, and the project catalog
//! whenever one changes. The Editor schedules no usage reads of
//! its own: it renders what arrives, and only an explicit refresh asks the
//! Forge to re-read.

use artisan_domain::{LiveThinking, ThreadListing, ThreadRetitled};

use crate::native_transport_service::HostStateEvent;

use super::*;

impl NativeApplication {
    /// Applies one pushed host-state value.
    pub(super) fn apply_host_state(&mut self, state: HostStateEvent, cx: &mut Context<Self>) {
        match state {
            HostStateEvent::AccountUsage(snapshot) => self.apply_pushed_usage(&snapshot, cx),
            HostStateEvent::Preferences(preferences) => {
                self.apply_forge_preferences(&preferences, cx);
            }
            HostStateEvent::ThreadRetitled(retitled) => self.apply_thread_title(&retitled, cx),
            HostStateEvent::RunUsage(usage) => self.apply_pushed_run_usage(usage, cx),
            HostStateEvent::LiveThinking(thinking) => self.apply_live_thinking(&thinking, cx),
            HostStateEvent::HeldBackWork(work) => self.apply_held_back_work(&work, cx),
            HostStateEvent::EarlierTurnMarkers(markers) => {
                self.apply_earlier_turn_markers(markers, cx);
            }
            HostStateEvent::RecentThreads(listing) => self.apply_recent_threads(listing, cx),
            HostStateEvent::ProjectCatalog(listing) => self.apply_project_catalog(&listing, cx),
            HostStateEvent::EngineInstalls(snapshot) => self.apply_engine_installs(snapshot, cx),
        }
        cx.notify();
    }

    /// Shows what the open thread's live run is thinking, and drops it once
    /// the run moved on. A push for any other thread is stale and ignored.
    fn apply_live_thinking(&mut self, thinking: &LiveThinking, cx: &mut Context<Self>) {
        if self.selected_thread.as_ref() != Some(&thinking.thread_id) {
            return;
        }
        let same_thread = self
            .engine_observations
            .as_ref()
            .is_some_and(|retained| retained.thread_id() == &thinking.thread_id);
        if !same_thread {
            self.engine_observations =
                Some(EngineObservationState::new(thinking.thread_id.clone()));
        }
        let changed = self
            .engine_observations
            .as_mut()
            .is_some_and(|state| state.set_live_thinking(thinking));
        if changed {
            self.replay_observation_activity(cx);
        }
    }

    /// Records which settled turns of the open thread keep their work rows
    /// on the Forge. The push precedes the thread's history-current marker,
    /// so the turns' sections appear with the rest of the thread.
    fn apply_held_back_work(
        &mut self,
        work: &artisan_domain::HeldBackWork,
        cx: &mut Context<Self>,
    ) {
        if self.selected_thread.as_ref() != Some(&work.thread_id) {
            return;
        }
        let same_thread = self
            .engine_observations
            .as_ref()
            .is_some_and(|retained| retained.thread_id() == &work.thread_id);
        if !same_thread {
            self.engine_observations = Some(EngineObservationState::new(work.thread_id.clone()));
        }
        let changed = self
            .engine_observations
            .as_mut()
            .is_some_and(|state| state.set_held_back(work));
        if changed {
            self.replay_observation_activity(cx);
        }
    }

    /// Shows a subscribed thread's refined title at once, wherever the
    /// listing names it.
    fn apply_thread_title(&mut self, retitled: &ThreadRetitled, cx: &mut Context<Self>) {
        let Some(listing) = self.thread_listing.as_ref() else {
            return;
        };
        if !listing
            .threads()
            .iter()
            .any(|thread| thread.thread_id == retitled.thread_id && thread.title != retitled.title)
        {
            return;
        }
        let threads = listing
            .threads()
            .iter()
            .cloned()
            .map(|mut thread| {
                if thread.thread_id == retitled.thread_id {
                    thread.title = retitled.title.clone();
                }
                thread
            })
            .collect();
        let Ok(listing) = ThreadListing::new(threads) else {
            return;
        };
        self.thread_listing = Some(listing.clone());
        self.update_thread_picker(listing, self.selected_thread.clone(), cx);
        self.sync_command_menu_groups(cx);
    }
}
