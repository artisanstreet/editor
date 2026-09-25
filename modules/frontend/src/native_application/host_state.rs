//! Connection-scoped state the Forge pushes over the delivery stream.
//!
//! The Forge pushes each engine's usage with its readiness verdict, the
//! user's preferences, and the subscribed thread's display title and live
//! run usage whenever one changes. The Editor schedules no usage reads of
//! its own: it renders what arrives, and only an explicit refresh asks the
//! Forge to re-read.

use artisan_domain::{ThreadListing, ThreadRetitled};

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
        }
        cx.notify();
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
