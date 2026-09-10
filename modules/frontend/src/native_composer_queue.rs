//! GPUI projection for the authoritative queued-message lip and usage state.
//!
//! `NativeComposerControls` already owns the keyboard/pointer action buttons
//! for pending steering. This module supplies that component's exact rows,
//! renders the bounded count/status companion, and resolves controls events
//! back to a generation-fenced queue identity. It never withdraws, reads, or
//! restores anything itself; the application owns those transport and draft
//! operations.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_ui::{
    button::{AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility},
    motion::MotionPolicy,
    theme::{ArtisanTheme, DesktopTheme},
};
use gpui::prelude::{InteractiveElement as _, ParentElement as _, Styled as _};
use gpui::{App, Div, ElementId, FocusHandle, Stateful, Window, div, px};

use crate::composer_queue_state::{
    ComposerQueueIdentity, ComposerQueueState, QueueLipRow, QueueStatus, ReportingUsage,
};
use crate::native_composer_controls::{
    NativeComposerControlsEvent, NativeComposerControlsSnapshot, PendingSteeringRow,
};
use crate::native_context_usage::NativeContextUsage;

/// Stable selector for the queue count/status companion surface.
pub(crate) const NATIVE_COMPOSER_QUEUE_SELECTOR: &str = "artisan-native-composer-queue";
/// Stable selector for the explicit restore retry action.
pub(crate) const NATIVE_COMPOSER_QUEUE_RESTORE_SELECTOR: &str =
    "artisan-native-composer-queue-restore";

/// A controls action resolved to one exact byte-free queue identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum QueueControlIntent {
    /// The caller must capture the empty composer target before beginning it.
    Edit(ComposerQueueIdentity),
    /// The caller can begin withdrawal without reading image bytes.
    Discard(ComposerQueueIdentity),
}

/// Copies the queue/usage projection into the existing controls snapshot.
///
/// Rows remain in the authoritative page order and leave the lip only after a
/// later parent projection omits them. Context usage is made from the last
/// reporting run, if any; the existing `NativeContextUsage::presentation`
/// hides a gauge when the report omitted context or its window.
pub(crate) fn project_controls_snapshot(
    state: &ComposerQueueState,
    snapshot: &mut NativeComposerControlsSnapshot,
) {
    snapshot.pending_steering = state
        .pending_lip_rows()
        .into_iter()
        .map(pending_steering_row)
        .collect();
    snapshot.context_usage = native_context_usage_for(state, snapshot.run_id.as_deref());
    if snapshot.run_id.is_none() {
        snapshot.run_id = snapshot
            .context_usage
            .as_ref()
            .map(|usage| usage.reporting_run_id.clone());
    }
    if snapshot.context_usage.is_none() {
        snapshot.context_usage_open = false;
    }
}

fn native_context_usage_for(
    state: &ComposerQueueState,
    current_run_id: Option<&str>,
) -> Option<NativeContextUsage> {
    let usage = state.reporting_usage_for(current_run_id)?;
    Some(native_context_usage(usage))
}

fn native_context_usage(usage: ReportingUsage) -> NativeContextUsage {
    NativeContextUsage::new(
        usage.run_id,
        usage.engine_id,
        usage.model_id,
        usage.model_name,
        usage.context_tokens,
        usage.context_window_tokens,
    )
    .with_breakdown(
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.output_tokens,
    )
}

fn pending_steering_row(row: QueueLipRow) -> PendingSteeringRow {
    PendingSteeringRow::new(row.command_id, row.generation, row.text, row.editable)
}

/// Resolves an existing controls event without performing its side effect.
///
/// The generation and original queue command id must still be present in the
/// current page and editable projection. The parent then obtains the entry's
/// exact thread/message/original-request ids from `ComposerQueueState` before
/// creating a transport command.
#[must_use]
pub(crate) fn resolve_controls_event(
    state: &ComposerQueueState,
    event: &NativeComposerControlsEvent,
) -> Option<QueueControlIntent> {
    let (command_id, generation, intent) = match event {
        NativeComposerControlsEvent::EditQueuedSteer {
            command_id,
            generation,
        } => (command_id.as_str(), *generation, QueueIntentKind::Edit),
        NativeComposerControlsEvent::DiscardQueuedSteer {
            command_id,
            generation,
        } => (command_id.as_str(), *generation, QueueIntentKind::Discard),
        _ => return None,
    };
    let identity = ComposerQueueIdentity::new(command_id, generation);
    let editable = state
        .pending_lip_rows()
        .into_iter()
        .any(|row| row.command_id == command_id && row.generation == generation && row.editable);
    editable.then(|| match intent {
        QueueIntentKind::Edit => QueueControlIntent::Edit(identity),
        QueueIntentKind::Discard => QueueControlIntent::Discard(identity),
    })
}

#[derive(Clone, Copy)]
enum QueueIntentKind {
    Edit,
    Discard,
}

/// Returns the exact queue count copy, or no copy when no rows/count are
/// authoritative yet. A count remains exact even when the server says the
/// bounded page has more rows than it can display.
#[must_use]
pub(crate) fn queue_count_label(state: &ComposerQueueState) -> Option<String> {
    if state.total_count() == 0 && state.entries().is_empty() {
        return None;
    }
    if state.has_more() {
        Some(format!(
            "{} queued · showing {} here",
            state.total_count(),
            state.entries().len()
        ))
    } else {
        Some(format!("{} queued", state.total_count()))
    }
}

/// Renders the bounded queue count and any truthful operation status.
///
/// The existing controls entity renders the ordered rows and edit/discard
/// buttons. Mount this small sibling above that lip when the parent wants the
/// exact count/status visible without turning the composer into a settings
/// page.
#[must_use]
pub(crate) fn render_queue_summary(
    state: &ComposerQueueState,
    theme: ArtisanTheme,
) -> Option<Stateful<Div>> {
    render_queue_summary_inner(state, theme, None)
}

/// Renders the same summary with an explicit restore retry button.
///
/// The callback is invoked only by pointer/keyboard activation of the button;
/// it must capture a fresh empty-composer target and use
/// `take_restore_candidate_with_target` before attempting restoration.
#[must_use]
pub(crate) fn render_queue_summary_with_retry<F>(
    state: &ComposerQueueState,
    theme: ArtisanTheme,
    focus: FocusHandle,
    on_retry: F,
) -> Option<Stateful<Div>>
where
    F: Fn(&mut Window, &mut App) + 'static,
{
    render_queue_summary_inner(state, theme, Some((focus, Box::new(on_retry))))
}

type RetryCallback = Box<dyn Fn(&mut Window, &mut App)>;

fn render_queue_summary_inner(
    state: &ComposerQueueState,
    theme: ArtisanTheme,
    retry: Option<(FocusHandle, RetryCallback)>,
) -> Option<Stateful<Div>> {
    let count = queue_count_label(state);
    let status = state.status();
    if count.is_none() && status == QueueStatus::Idle {
        return None;
    }

    let desktop_theme = DesktopTheme::neutral_dark();
    let mut body = div()
        .id(ElementId::Name(NATIVE_COMPOSER_QUEUE_SELECTOR.into()))
        .debug_selector(|| NATIVE_COMPOSER_QUEUE_SELECTOR.to_owned())
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(20.0))
        .py(px(6.0))
        .text_size(theme.typography.control_text)
        .text_color(desktop_theme.secondary);

    if let Some(count) = count {
        body = body.child(div().min_w(px(0.0)).flex_shrink_0().child(count));
    }
    if !status.label().is_empty() {
        body = body.child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .truncate()
                .child(status.label()),
        );
    }

    if status.restore_retry_available()
        && state.can_retry_restore()
        && let Some((focus, on_retry)) = retry
    {
        let retry_button = Button::new(
            ElementId::Name(NATIVE_COMPOSER_QUEUE_RESTORE_SELECTOR.into()),
            focus,
            theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::Small,
            ButtonContent::text("Restore"),
        )
        .expect("the queued-message restore button is valid")
        .focus_visibility(FocusVisibility::Visible)
        .disabled(false)
        .debug_selector(NATIVE_COMPOSER_QUEUE_RESTORE_SELECTOR)
        .on_activate(move |_, window, app| on_retry(window, app));
        body = body.child(retry_button);
    }

    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::{
        AuthoredText, MessageId, QueuedMessageListOrder, QueuedMessageListing,
        QueuedMessageSummary, RequestId, ThreadId, UnixMillis,
    };

    fn thread(value: &str) -> ThreadId {
        ThreadId::parse(value).expect("thread")
    }

    fn request(value: &str) -> RequestId {
        RequestId::parse(value).expect("request")
    }

    fn page(thread_id: &ThreadId) -> QueuedMessageListing {
        QueuedMessageListing::new(
            thread_id.clone(),
            QueuedMessageListOrder::OldestFirst,
            2,
            2,
            vec![
                QueuedMessageSummary {
                    message_id: MessageId::parse("message-a").expect("message"),
                    thread_id: thread_id.clone(),
                    original_request_id: request("command-a"),
                    text: Some(AuthoredText::parse("first").expect("text")),
                    attachments: Vec::new(),
                    accepted_at: UnixMillis::EPOCH,
                    last_error: Some(
                        artisan_domain::DispatchError::parse("engine unconfigured".to_owned())
                            .expect("dispatcher diagnostic"),
                    ),
                },
                QueuedMessageSummary {
                    message_id: MessageId::parse("message-b").expect("message"),
                    thread_id: thread_id.clone(),
                    original_request_id: request("command-b"),
                    text: Some(AuthoredText::parse("second").expect("text")),
                    attachments: Vec::new(),
                    accepted_at: UnixMillis::EPOCH,
                    last_error: None,
                },
            ],
        )
        .expect("page")
    }

    #[test]
    fn controls_projection_preserves_order_and_exact_count() {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 5);
        let token = state
            .begin_queue_refresh(true, true, false, false, true)
            .expect("refresh");
        state
            .apply_queue_listing(&token, page(&thread_id))
            .expect("page");

        let rows = state.pending_lip_rows();
        assert_eq!(
            rows.iter()
                .map(|row| row.command_id.as_str())
                .collect::<Vec<_>>(),
            ["command-a", "command-b"]
        );
        assert_eq!(queue_count_label(&state).as_deref(), Some("2 queued"));

        let mut snapshot = NativeComposerControlsSnapshot::default();
        snapshot.run_id = None;
        project_controls_snapshot(&state, &mut snapshot);
        assert_eq!(snapshot.pending_steering.len(), 2);
        assert_eq!(snapshot.pending_steering[1].text, "second");
        assert_eq!(
            state.entries()[0].dispatch_error(),
            Some("engine unconfigured")
        );
        assert_eq!(state.entries()[1].dispatch_error(), None);
    }

    #[test]
    fn stale_controls_generation_cannot_resolve_a_reused_command() {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 7);
        let token = state
            .begin_queue_refresh(true, true, false, false, true)
            .expect("refresh");
        state
            .apply_queue_listing(&token, page(&thread_id))
            .expect("page");
        let stale = NativeComposerControlsEvent::EditQueuedSteer {
            command_id: "command-a".to_owned(),
            generation: 6,
        };
        assert!(resolve_controls_event(&state, &stale).is_none());
        let current = NativeComposerControlsEvent::DiscardQueuedSteer {
            command_id: "command-a".to_owned(),
            generation: 7,
        };
        assert!(matches!(
            resolve_controls_event(&state, &current),
            Some(QueueControlIntent::Discard(_))
        ));
    }
}
