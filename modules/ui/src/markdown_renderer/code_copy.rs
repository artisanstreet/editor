//! The copy action of one code fence.
//!
//! The Markdown renderer is synchronous and retains nothing per fence, so the
//! action keeps its own small state in the window, keyed by the fence's
//! selector: the focus handle that makes it keyboard reachable, and the
//! moment of the last copy that drives the shared confirmation.

use std::time::Instant;

use artisan_assets::AssetId;
use gpui::{App, ClipboardItem, ElementId, FocusHandle, Pixels, SharedString, Window, prelude::*};

use crate::button::{
    AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility,
};
use crate::copy_feedback::{COPY_FEEDBACK_WINDOW, copy_feedback_icon, copy_feedback_progress};
use crate::motion::MotionPolicy;
use crate::theme::ArtisanTheme;

/// Accessible name of the fence copy action.
const COPY_CODE_LABEL: &str = "Copy code";

/// Suffix of the copy action's debug selector under its fence selector.
pub(super) const CODE_COPY_SELECTOR_SUFFIX: &str = "copy";

/// Copies one fence's exact displayed source to the clipboard.
#[derive(IntoElement)]
pub(super) struct CodeCopyButton {
    selector: SharedString,
    state_id: SharedString,
    source: SharedString,
    corner_radius: Pixels,
    theme: ArtisanTheme,
}

struct CodeCopyState {
    focus: FocusHandle,
    copied_at: Option<Instant>,
}

impl CodeCopyButton {
    /// `selector` is the action's stable selector (the fence selector plus
    /// [`CODE_COPY_SELECTOR_SUFFIX`]) and `state_id` the key of its window
    /// state (the selector plus `-state`), both formatted once per cached
    /// document; `source` is the fence body exactly as displayed;
    /// `corner_radius` is the action's corner, nested inside the fence's own.
    pub(super) fn new(
        selector: SharedString,
        state_id: SharedString,
        source: SharedString,
        corner_radius: Pixels,
        theme: ArtisanTheme,
    ) -> Self {
        Self {
            selector,
            state_id,
            source,
            corner_radius,
            theme,
        }
    }
}

impl RenderOnce for CodeCopyButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(ElementId::Name(self.state_id.clone()), cx, |_, cx| {
            CodeCopyState {
                focus: cx.focus_handle().tab_stop(true),
                copied_at: None,
            }
        });
        let motion = if cx.reduce_motion() {
            MotionPolicy::Reduced
        } else {
            MotionPolicy::Full
        };
        let copied = state
            .read(cx)
            .copied_at
            .map(|at| at.elapsed())
            .map_or(0.0, |elapsed| {
                if elapsed < COPY_FEEDBACK_WINDOW {
                    window.request_animation_frame();
                }
                copy_feedback_progress(elapsed, motion)
            });
        let focus = state.read(cx).focus.clone();
        let source = self.source;
        Button::new(
            ElementId::Name(self.selector.clone()),
            focus,
            self.theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::IconSmall,
            ButtonContent::icon_only(
                AssetId::TABLER_COPY,
                AccessibleLabel::new(COPY_CODE_LABEL).expect("the copy label is not blank"),
            ),
        )
        .expect("an icon-only copy button has valid content")
        .icon_slot(copy_feedback_icon(copied))
        .corner_radius(self.corner_radius)
        .hover_fill()
        .focus_visibility(FocusVisibility::Visible)
        .tint(
            self.theme.colors.muted_foreground.to_paint(),
            self.theme.colors.foreground.to_paint(),
        )
        .debug_selector(self.selector)
        .on_activate(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(source.to_string()));
            state.update(cx, |state, cx| {
                state.copied_at = Some(Instant::now());
                cx.notify();
            });
        })
    }
}
