//! One editor window owns the available Forge connections and their scoped presentation state.
//!
//! Changing the selected connection does not restart the editor or tear down another host's
//! subscription. Inactive views continue receiving their own events, so identical project or
//! thread ids from different Forges cannot mix, and drafts survive switching away and back.
use super::impl_machines::SelectMachine;
use super::*;
use std::path::PathBuf;

struct MachineSession {
    home: Option<PathBuf>,
    view: Entity<NativeApplication>,
    _selection: Subscription,
}

pub(super) struct NativeWorkspace {
    sessions: Vec<MachineSession>,
    selected: usize,
}

impl NativeWorkspace {
    pub(super) fn new(
        home: Option<PathBuf>,
        service: Option<Arc<NativeTransportService>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut workspace = Self {
            sessions: Vec::new(),
            selected: 0,
        };
        workspace.add_session(home, service, window, cx);
        workspace
    }

    fn add_session(
        &mut self,
        home: Option<PathBuf>,
        service: Option<Arc<NativeTransportService>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = crate::native_hosts::label(home.as_deref());
        let view = cx.new(|cx| {
            let mut view = NativeApplication::new(service, window, cx);
            view.machine_home.clone_from(&home);
            view.machine_label = label;
            #[cfg(not(test))]
            view.refresh_machines(cx);
            view.start_polling(cx);
            view
        });
        let selection = cx.subscribe_in(
            &view,
            window,
            |workspace, _, event: &SelectMachine, window, cx| {
                workspace.select(event.0.clone(), window, cx);
            },
        );
        self.selected = self.sessions.len();
        self.sessions.push(MachineSession {
            home,
            view,
            _selection: selection,
        });
    }

    fn select(&mut self, home: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let profile_open = self.sessions[self.selected]
            .view
            .read(cx)
            .profile_menu
            .is_open();
        let existing = self.sessions.iter().position(|session| {
            crate::native_hosts::same_host(home.as_deref(), session.home.as_deref())
        });
        if let Some(index) = existing {
            self.selected = index;
            let view = self.sessions[index].view.clone();
            view.update(cx, |view, cx| {
                // A stopped service has already released its reconnect custody. Keep the
                // editor state, and refresh the invitation when explicitly selecting it again.
                if view
                    .service
                    .as_ref()
                    .is_none_or(|service| service.is_finished())
                {
                    if let Some(service) = view.service.take() {
                        let _ = service.join();
                        view.handle_service_stopped(ServiceStopStatus::Clean, cx);
                        view.retire_host_after_switch_stop(cx);
                        view.active_subscription_request_id = None;
                        view.retained_switch_request_ids.clear();
                        view.retained_switch_patch_ids.clear();
                        view.retained_switch_snapshot_threads.clear();
                        view.retained_switch_listings.clear();
                    }
                    view.service = Self::connect(home);
                    view.service_stopped = false;
                    view.poll_task = None;
                    view.state = if view.service.is_some() {
                        NativeViewState::Loading
                    } else {
                        NativeViewState::Failure(command_failure(CommandSendError::Stopped))
                    };
                    view.start_polling(cx);
                }
                view.focus_handle.focus(window, cx);
                cx.notify();
            });
        } else {
            let service = Self::connect(home.clone());
            self.add_session(home, service, window, cx);
        }
        if profile_open {
            self.sessions[self.selected].view.update(cx, |view, cx| {
                view.profile_menu.set_open(true);
                view.begin_profile_menu_open(cx);
                view.ensure_profile_usage(false, None, cx);
                view.profile_focus.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn connect(home: Option<PathBuf>) -> Option<Arc<NativeTransportService>> {
        #[cfg(not(test))]
        {
            NativeTransportService::spawn_for_host(home)
                .ok()
                .map(Arc::new)
        }
        #[cfg(test)]
        {
            drop(home);
            None
        }
    }

    pub(super) fn selected_view(&self) -> Entity<NativeApplication> {
        self.sessions[self.selected].view.clone()
    }

    pub(super) fn open_machines(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.sessions[self.selected]
            .view
            .update(cx, |view, cx| view.open_machines(window, cx));
    }

    pub(super) fn prepare_shutdown(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<Arc<NativeTransportService>> {
        self.sessions
            .iter()
            .filter_map(|session| {
                session.view.update(cx, |view, cx| {
                    view.prepare_shutdown(cx);
                    view.service.clone()
                })
            })
            .collect()
    }
}

impl Render for NativeWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.sessions[self.selected].view.clone();
        window.set_window_title(&format!("{WINDOW_TITLE} — {}", view.read(cx).machine_label));
        div().size_full().child(view)
    }
}

#[cfg(test)]
mod tests;
