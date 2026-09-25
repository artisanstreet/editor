//! This host connection's side of a machine switch and of quitting the Editor.
//!
//! The workspace owns the switch transaction. The view seals its one
//! connection, keeps the composer read-only, shows what is still saving, and
//! is then dropped wholesale with every host-scoped value it owns.
//!
//! Holds taken here cover application-level operations that outlive their
//! command. A message flight holds until its correlated reply arrives or the
//! flight ends; a send still waiting for account readiness has not been
//! admitted, so sealing cancels it and the draft stays in the composer.

use super::*;
use crate::native_transport_service::{ConnectionHolds, Hold};

/// A switch away from this host that is waiting for its in-flight work.
pub(super) struct HostSwitchNotice {
    target_label: String,
    _repaint: Option<Task<()>>,
}

impl NativeApplication {
    /// The holds of this view's connection, when it has one.
    pub(super) fn connection_holds(&self) -> Option<Arc<ConnectionHolds>> {
        self.service
            .as_ref()
            .map(|service| Arc::clone(service.holds()))
    }

    /// A hold for an application-level operation that outlives its command.
    pub(super) fn connection_hold(&self, kind: HoldKind) -> Option<Hold> {
        self.service.as_ref()?.holds().try_hold(kind)
    }

    /// Admits one command on this view's connection.
    pub(super) fn submit_command(
        &self,
        command: NativeTransportCommand,
    ) -> Result<(), CommandSendError> {
        #[cfg(test)]
        if let Some(sink) = &self.test_command_sink {
            sink.commands.borrow_mut().push(command);
            return sink.outcomes.borrow_mut().pop_front().unwrap_or(Ok(()));
        }
        let Some(service) = self.service.as_ref() else {
            return Err(CommandSendError::Stopped);
        };
        service.submit(command)
    }

    /// Releases the message-flight hold once the flight has ended, its
    /// correlated reply has arrived, or the connection is gone.
    pub(super) fn settle_message_flight_hold(&mut self, replied: Option<&RequestId>) {
        let settled = self.message_flight.as_ref().is_none_or(|flight| {
            replied == Some(&flight.request_id) || self.service_stopped || self.shutdown_prepared
        });
        if settled {
            self.message_flight_hold = None;
        }
    }

    /// Seals this connection for a switch to `target_label` and returns the
    /// holds the switch drains.
    pub(super) fn begin_host_switch(
        &mut self,
        target_label: String,
        cx: &mut Context<Self>,
    ) -> Option<Arc<ConnectionHolds>> {
        let holds = self.connection_holds();
        if let Some(holds) = &holds {
            holds.seal();
        }
        self.host_switch = Some(HostSwitchNotice {
            target_label,
            _repaint: holds
                .as_ref()
                .map(|holds| repaint_on_hold_changes(holds, cx)),
        });
        self.composer.update(cx, |composer, composer_cx| {
            composer.set_disabled(true, composer_cx);
        });
        self.sync_composer_availability(cx);
        cx.notify();
        holds
    }

    /// Points a pending switch at another host.
    pub(super) fn retarget_host_switch(&mut self, target_label: String, cx: &mut Context<Self>) {
        if let Some(notice) = self.host_switch.as_mut() {
            notice.target_label = target_label;
            cx.notify();
        }
    }

    /// Cancels a pending switch: the connection accepts work again.
    pub(super) fn cancel_host_switch(&mut self, cx: &mut Context<Self>) {
        if self.host_switch.take().is_none() {
            return;
        }
        if let Some(holds) = self.connection_holds() {
            holds.unseal();
        }
        if !self.shutdown_prepared && !self.composer_queue.state.edit_pending() {
            self.composer.update(cx, |composer, composer_cx| {
                composer.set_disabled(false, composer_cx);
            });
        }
        self.sync_composer_availability(cx);
        cx.notify();
    }

    /// Whether a switch away from this host is in progress.
    pub(super) const fn host_switch_pending(&self) -> bool {
        self.host_switch.is_some()
    }

    /// Progress copy for a pending switch, such as
    /// `Saving 2 messages to Ubuntu…`, then `Switching to This computer…`.
    pub(super) fn host_switch_status(&self) -> Option<String> {
        let notice = self.host_switch.as_ref()?;
        let saving = self
            .service
            .as_ref()
            .and_then(|service| service.holds().status().summary());
        Some(saving.map_or_else(
            || format!("Switching to {}…", notice.target_label),
            |summary| format!("Saving {summary} to {}…", self.machine_label),
        ))
    }

    /// The window-level switch progress notice, beside the machine error.
    pub(super) fn host_switch_banner(&self) -> Option<Stateful<Div>> {
        let status = self.host_switch_status()?;
        Some(
            div()
                .id("host-switch-status")
                .debug_selector(|| "host-switch-status".to_owned())
                .role(gpui::Role::Status)
                .absolute()
                .top_8()
                .right_4()
                .p_3()
                .rounded(px(10.0))
                .bg(self.desktop_theme.chrome)
                .text_size(px(12.0))
                .text_color(self.desktop_theme.foreground)
                .child(status),
        )
    }

    /// Retains any admitted message and seals the connection before the
    /// application starts service shutdown. This runs on the GPUI
    /// application thread.
    pub(super) fn prepare_shutdown(&mut self, cx: &mut Context<Self>) {
        if let Some(holds) = self.connection_holds() {
            holds.seal();
        }
        self.flush_composer_drafts(cx);
        self.shutdown_prepared = true;
        self.thread_switch_flight = None;
        self.ordinary_unsubscribe_thread = None;
        self.pending_thread = None;
        self.set_picker_disabled(true, cx);
        self.set_thread_picker_disabled(true, cx);
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.composer.update(cx, |composer, composer_cx| {
            composer.set_disabled(true, composer_cx);
        });
        cx.notify();
    }
}

/// Repaints the view whenever the connection's holds change, waiting for each
/// change off the UI thread.
fn repaint_on_hold_changes(
    holds: &ConnectionHolds,
    cx: &mut Context<NativeApplication>,
) -> Task<()> {
    let mut changes = holds.subscribe();
    cx.spawn(async move |view, cx| {
        loop {
            let (next, open) = cx
                .background_executor()
                .spawn(async move {
                    let open = changes.changed().await.is_ok();
                    (changes, open)
                })
                .await;
            changes = next;
            if !open || view.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        }
    })
}
