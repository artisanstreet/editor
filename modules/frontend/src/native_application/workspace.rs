//! One editor window owns exactly one Forge connection.
//!
//! The machine selector chooses which Forge that is. A switch is one
//! transaction: seal the current connection so it refuses new mutations,
//! wait off the UI thread until its holds drain, shut its service down, drop
//! the whole host-scoped [`NativeApplication`], and connect a fresh one to the
//! chosen host. Re-selecting the current host while it drains cancels the
//! switch. Quitting seals and drains the same connection with a bounded wait.
//!
//! The view entity is the host-scoped state: every value it owns belongs to
//! its one connection and is discarded with it. Only window presentation that
//! is not about a host (the sidebar and profile-menu disclosure) carries over.
use super::impl_machines::{HostResolved, SelectMachine};
use super::*;
use std::path::PathBuf;
use std::time::Instant;

/// How long a stopping service may take to release its custody during a switch.
const SERVICE_STOP_LIMIT: Duration = Duration::from_secs(45);

/// How long quitting waits for in-flight mutations before shutting down anyway.
pub(super) const QUIT_DRAIN_LIMIT: Duration = Duration::from_secs(10);

type Connector = Box<dyn FnMut(Option<PathBuf>) -> Option<Arc<NativeTransportService>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SwitchPhase {
    /// Sealed and waiting for holds; re-selecting the current host cancels.
    Draining,
    /// Drained and shutting the old service down; no longer cancellable.
    Disconnecting,
}

struct HostSwitch {
    target: Option<PathBuf>,
    phase: SwitchPhase,
    task: Task<()>,
}

/// The one connected host: its home and the view that owns all of its state.
struct ConnectedHost {
    home: Option<PathBuf>,
    view: Entity<NativeApplication>,
    _selection: Subscription,
    _resolution: Subscription,
}

pub(super) struct NativeWorkspace {
    host: ConnectedHost,
    switch: Option<HostSwitch>,
    connector: Connector,
}

impl NativeWorkspace {
    pub(super) fn new(
        home: Option<PathBuf>,
        service: Option<Arc<NativeTransportService>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            host: Self::open_host(home, service, window, cx),
            switch: None,
            connector: Box::new(spawn_connection),
        }
    }

    fn open_host(
        home: Option<PathBuf>,
        service: Option<Arc<NativeTransportService>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ConnectedHost {
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
        let resolution = cx.subscribe(&view, |workspace, _, event: &HostResolved, cx| {
            workspace.adopt_resolved_home(event.0.clone(), cx);
        });
        ConnectedHost {
            home,
            view,
            _selection: selection,
            _resolution: resolution,
        }
    }

    fn select(&mut self, home: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let current = crate::native_hosts::same_host(home.as_deref(), self.host.home.as_deref());
        match self.switch.as_mut() {
            Some(switch) if current && switch.phase == SwitchPhase::Draining => {
                self.cancel_switch(cx);
            }
            Some(switch) => {
                let label = crate::native_hosts::label(home.as_deref());
                switch.target = home;
                self.host
                    .view
                    .update(cx, |view, cx| view.retarget_host_switch(label, cx));
            }
            None if current => self.retry_current_host(home, window, cx),
            None => self.begin_switch(home, window, cx),
        }
        cx.notify();
    }

    /// Seals the current connection and runs the switch off the UI thread.
    fn begin_switch(
        &mut self,
        target: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = crate::native_hosts::label(target.as_deref());
        let holds = self
            .host
            .view
            .update(cx, |view, cx| view.begin_host_switch(label, cx));
        let task = cx.spawn_in(window, async move |workspace, cx| {
            if let Some(holds) = holds {
                cx.background_executor()
                    .spawn(async move { holds.idle().await })
                    .await;
            }
            let Ok(service) = workspace.update(cx, Self::disconnect) else {
                return;
            };
            if let Some(service) = service {
                let executor = cx.background_executor().clone();
                if !close_connection(service, executor, Duration::ZERO, Some(SERVICE_STOP_LIMIT))
                    .await
                {
                    eprintln!("Forge host switch: the previous connection did not stop in time");
                }
            }
            let _ = workspace.update_in(cx, |workspace, window, cx| {
                workspace.finish_switch(window, cx);
            });
        });
        self.switch = Some(HostSwitch {
            target,
            phase: SwitchPhase::Draining,
            task,
        });
    }

    fn cancel_switch(&mut self, cx: &mut Context<Self>) {
        if self.switch.take().is_some() {
            self.host
                .view
                .update(cx, NativeApplication::cancel_host_switch);
        }
    }

    /// Drained: stops the old view and requests its service shutdown.
    fn disconnect(&mut self, cx: &mut Context<Self>) -> Option<Arc<NativeTransportService>> {
        if let Some(switch) = self.switch.as_mut() {
            switch.phase = SwitchPhase::Disconnecting;
        }
        self.host.view.update(cx, |view, cx| {
            view.prepare_shutdown(cx);
            view.service.clone()
        })
    }

    /// Reset and reconnect: drops every host-scoped value with the old view
    /// and opens a fresh view on the chosen host.
    fn finish_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(switch) = self.switch.take() else {
            return;
        };
        // This runs inside the switch task; let it finish rather than cancel it.
        switch.task.detach();
        let previous = self.host.view.read(cx);
        let profile_open = previous.profile_menu.is_open();
        let sidebar_collapsed = previous.sidebar_collapsed;
        let service = (self.connector)(switch.target.clone());
        // Replacing the host drops the previous view and every value it owns.
        self.host = Self::open_host(switch.target, service, window, cx);
        self.host.view.update(cx, |view, cx| {
            view.sidebar_collapsed = sidebar_collapsed;
            view.focus_handle.focus(window, cx);
        });
        if profile_open {
            self.reopen_profile_menu(window, cx);
        }
        // A fresh launch reopens the machine the user last chose. Best-effort:
        // a hint that cannot be saved must not disturb the switch.
        let home = self.host.home.clone();
        let _ = crate::editor_settings::update(cx, |settings| settings.with_reopen_host(home));
        cx.notify();
    }

    /// The connection resolved its host to a newer registration: the
    /// session's home and the reopen hint follow it, so a relaunch opens the
    /// registration that exists rather than the one it replaced.
    fn adopt_resolved_home(&mut self, home: PathBuf, cx: &mut Context<Self>) {
        let home = Some(home);
        if self.host.home == home {
            return;
        }
        self.host.home.clone_from(&home);
        let _ = crate::editor_settings::update(cx, |settings| settings.with_reopen_host(home));
        cx.notify();
    }

    fn reopen_profile_menu(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.host.view.update(cx, |view, cx| {
            view.profile_menu.set_open(true);
            view.begin_profile_menu_open(cx);
            view.profile_focus.focus(window, cx);
        });
    }

    /// Selecting the connected host again retries a failed or stopped
    /// connection in place.
    fn retry_current_host(
        &mut self,
        home: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = self.host.view.clone();
        let (retry_pending, failed_live, stopped) = {
            let app = view.read(cx);
            let live = app
                .service
                .as_ref()
                .filter(|service| !service.is_finished())
                .cloned();
            (
                app.connection_retry_pending,
                live.filter(|_| matches!(app.state, NativeViewState::Failure(_))),
                app.service
                    .as_ref()
                    .is_none_or(|service| service.is_finished()),
            )
        };
        if retry_pending {
            return;
        }
        if let Some(service) = failed_live {
            view.update(cx, |view, cx| {
                view.close_failed_connection(service, home, cx);
            });
            return;
        }
        // A stopped service has already released its reconnect custody.
        // Reconnect in place and refresh the invitation.
        let replacement = stopped.then(|| (self.connector)(home));
        view.update(cx, |view, cx| {
            if let Some(service) = replacement {
                view.reconnect_stopped_service(service, cx);
            }
            view.focus_handle.focus(window, cx);
            cx.notify();
        });
    }

    pub(super) fn selected_view(&self) -> Entity<NativeApplication> {
        self.host.view.clone()
    }

    pub(super) fn open_machines(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.host
            .view
            .update(cx, |view, cx| view.open_machines(window, cx));
    }

    /// Quitting: a pending switch stops where it is, and the one connection
    /// is sealed and returned for a bounded drain and shutdown.
    pub(super) fn prepare_shutdown(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Arc<NativeTransportService>> {
        self.switch = None;
        self.host.view.update(cx, |view, cx| {
            view.prepare_shutdown(cx);
            view.service.clone()
        })
    }
}

impl NativeApplication {
    /// Shuts a failed live worker down off the UI thread, then asks the
    /// workspace to reconnect once its custody is released.
    fn close_failed_connection(
        &mut self,
        service: Arc<NativeTransportService>,
        home: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if service.request_shutdown().is_err() {
            self.set_failure(command_failure(CommandSendError::Busy), cx);
            return;
        }
        self.connection_retry_pending = true;
        self.state = NativeViewState::Loading;
        cx.spawn(async move |view, cx| {
            let deadline = Instant::now() + SERVICE_STOP_LIMIT;
            while !service.is_finished() && Instant::now() < deadline {
                cx.background_executor().timer(POLL_INTERVAL).await;
            }
            let _ = view.update(cx, |view, cx| {
                view.connection_retry_pending = false;
                if view.shutdown_prepared {
                    return;
                }
                if service.is_finished() {
                    cx.emit(SelectMachine(home));
                } else {
                    view.set_failure(command_failure(CommandSendError::Busy), cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn reconnect_stopped_service(
        &mut self,
        service: Option<Arc<NativeTransportService>>,
        cx: &mut Context<Self>,
    ) {
        if let Some(stopped) = self.service.take() {
            let _ = stopped.join();
            self.handle_service_stopped(ServiceStopStatus::Clean, cx);
            self.retire_host_after_switch_stop(cx);
            self.active_subscription_request_id = None;
            self.retained_switch_request_ids.clear();
            self.retained_switch_patch_ids.clear();
            self.retained_switch_snapshot_threads.clear();
            self.retained_switch_listings.clear();
        }
        self.service = service;
        self.service_stopped = false;
        self.poll_task = None;
        self.state = if self.service.is_some() {
            NativeViewState::Loading
        } else {
            NativeViewState::Failure(command_failure(CommandSendError::Stopped))
        };
        self.start_polling(cx);
    }
}

/// Seals `service`, waits up to `drain_limit` for its holds, then shuts it
/// down and waits up to `stop_limit` (unbounded when `None`) for it to
/// release custody. Returns whether the service finished.
pub(super) async fn close_connection(
    service: Arc<NativeTransportService>,
    executor: gpui::BackgroundExecutor,
    drain_limit: Duration,
    stop_limit: Option<Duration>,
) -> bool {
    service.holds().seal();
    let drain_deadline = Instant::now() + drain_limit;
    while !service.holds().status().is_idle()
        && !service.is_finished()
        && Instant::now() < drain_deadline
    {
        // Keep the bounded event bridge moving so in-flight handlers can reply.
        while matches!(service.try_recv(), Ok(Some(_))) {}
        executor.timer(POLL_INTERVAL).await;
    }
    let stop_deadline = stop_limit.and_then(|limit| Instant::now().checked_add(limit));
    loop {
        if service.is_finished() {
            let _ = service.join();
            return true;
        }
        if stop_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return false;
        }
        // Shutdown admission is nonblocking; retry while the queue is full.
        let _ = service.request_shutdown();
        while matches!(service.try_recv(), Ok(Some(_))) {}
        executor.timer(POLL_INTERVAL).await;
    }
}

fn spawn_connection(home: Option<PathBuf>) -> Option<Arc<NativeTransportService>> {
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

impl Render for NativeWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_window_title(&super::selectors::window_title(
            &self.host.view.read(cx).machine_label,
        ));
        div().size_full().child(self.host.view.clone())
    }
}

#[cfg(test)]
mod tests;
