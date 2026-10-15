//! The copy action of one code fence.
//!
//! The Markdown renderer is synchronous and retains nothing per fence, so the
//! action keeps its own small state in the window, keyed by the fence's
//! selector: the focus handle that makes it keyboard reachable, and the
//! moment of the last copy that drives the shared confirmation.

use std::time::{Duration, Instant};

use editor_assets::AssetId;
use gpui::{App, ClipboardItem, ElementId, SharedString, Task, Window, prelude::*};

use crate::button::{Button, ButtonVariant};
use crate::copy_feedback::{COPY_FEEDBACK_WINDOW, copy_feedback_icon, copy_feedback_progress};
use crate::motion::{MotionPlan, MotionPolicy, MotionRecipe};
use crate::theme::ArtisanTheme;

/// Accessible name of the fence copy action.
const COPY_CODE_LABEL: &str = "Copy code";

/// Copies one fence's exact displayed source to the clipboard.
#[derive(IntoElement)]
pub(super) struct CodeCopyButton {
    selector: SharedString,
    state_id: ElementId,
    source: SharedString,
    theme: ArtisanTheme,
}

struct CodeCopyState {
    copied_at: Option<Instant>,
    wake: Option<Task<()>>,
}

impl CodeCopyButton {
    /// Stable action and state IDs are prepared once per Item. `source`
    /// contains the exact text copied by this action.
    pub(super) fn new(
        selector: SharedString,
        state_id: ElementId,
        source: SharedString,
        theme: &ArtisanTheme,
    ) -> Self {
        Self {
            selector,
            state_id,
            source,
            theme: *theme,
        }
    }
}

impl RenderOnce for CodeCopyButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.state_id.clone(), cx, |_, _| CodeCopyState {
            copied_at: None,
            wake: None,
        });
        let motion = if cx.reduce_motion() {
            MotionPolicy::Reduced
        } else {
            MotionPolicy::Full
        };
        let transition = match motion.resolve(MotionRecipe::IconSwap) {
            MotionPlan::Animate(plan) => plan.duration(),
            MotionPlan::Immediate => Duration::ZERO,
        };
        let copied = state
            .read(cx)
            .copied_at
            .map(|at| at.elapsed())
            .map_or(0.0, |elapsed| {
                let progress = copy_feedback_progress(elapsed, motion);
                if elapsed < transition
                    || (elapsed < COPY_FEEDBACK_WINDOW && progress > 0.0 && progress < 1.0)
                {
                    window.request_animation_frame();
                }
                progress
            });
        let source = self.source;
        Button::icon(
            ElementId::Name(self.selector.clone()),
            self.theme,
            AssetId::TABLER_COPY,
            COPY_CODE_LABEL,
        )
        .variant(ButtonVariant::Ghost)
        .icon_slot(copy_feedback_icon(copied))
        .debug_selector(self.selector)
        .on_activate(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(source.to_string()));
            let weak = state.downgrade();
            let wake = cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                let _ = weak.update(cx, |state, cx| {
                    state.wake = None;
                    cx.notify();
                });
            });
            state.update(cx, |state, cx| {
                state.copied_at = Some(Instant::now());
                state.wake = Some(wake);
                cx.notify();
            });
        })
    }
}
