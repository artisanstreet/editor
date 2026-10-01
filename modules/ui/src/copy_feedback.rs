//! Copy confirmation shared by every copy action.
//!
//! A successful copy swaps the copy glyph for a check on the shared
//! icon-swap recipe, holds it, then swaps back. The turn footer's "copy
//! response" action and a code fence's copy action both read their progress
//! and paint their icon from here, so the two confirmations cannot drift.

use std::time::Duration;

use artisan_assets::AssetId;
use gpui::{Div, div, prelude::*, px};

use crate::asset_seam::asset_glyph;
use crate::motion::{MotionPlan, MotionPolicy, MotionRecipe};

/// How long the check holds before the copy glyph returns.
const COPY_FEEDBACK_HOLD: Duration = Duration::from_millis(1500);

/// How long after a copy the confirmation keeps animating.
///
/// The hold plus the return swap: a caller painting the confirmation asks
/// for animation frames until this much time has passed since the copy.
pub const COPY_FEEDBACK_WINDOW: Duration = Duration::from_millis(1750);

/// Edge of the confirmation icon box.
const COPY_FEEDBACK_ICON_PX: f32 = 16.0;

/// Copy confirmation uses the shared icon-swap recipe, holds, then returns.
///
/// Returns the unit progress toward the check: 0 shows the copy glyph, 1 the
/// check. Reduced motion steps between the two with no swap animation.
#[expect(
    clippy::cast_possible_truncation,
    reason = "unit opacity is bounded before narrowing to GPUI f32"
)]
#[must_use]
pub fn copy_feedback_progress(elapsed: Duration, motion: MotionPolicy) -> f32 {
    let MotionPlan::Animate(animation) = motion.resolve(MotionRecipe::IconSwap) else {
        return if elapsed < COPY_FEEDBACK_HOLD {
            1.0
        } else {
            0.0
        };
    };
    let duration = animation.duration().as_secs_f64();
    let progress = if elapsed < animation.duration() {
        elapsed.as_secs_f64() / duration
    } else if elapsed < COPY_FEEDBACK_HOLD {
        1.0
    } else {
        1.0 - (elapsed.saturating_sub(COPY_FEEDBACK_HOLD).as_secs_f64() / duration).min(1.0)
    };
    animation.curve().sample(progress) as f32
}

/// Paints the 16 px confirmation icon at `copied` progress toward the check.
///
/// The two glyphs cross-fade in place: the leaving face shrinks, blurs, and
/// fades while the arriving face grows in, per the icon-swap recipe.
#[must_use]
pub fn copy_feedback_icon(copied: f32) -> Div {
    let face = |asset, opacity: f32| {
        let mut face = div()
            .absolute()
            .size(px(COPY_FEEDBACK_ICON_PX))
            .flex()
            .items_center()
            .justify_center()
            .opacity(opacity);
        // The blur is a filter group in the renderer (an offscreen texture
        // cleared and blurred in four passes), so a face at rest carries
        // none: only a face mid-crossfade is blurred.
        if opacity < 1.0 {
            face = face.blur(px(2.0 * (1.0 - opacity)));
        }
        face.child(asset_glyph(asset).size(px(COPY_FEEDBACK_ICON_PX * (0.25 + 0.75 * opacity))))
    };
    let mut icon = div().relative().size(px(COPY_FEEDBACK_ICON_PX));
    // A fully faded face paints nothing; leaving it out spares every idle
    // copy button (one per code fence and settled turn) a filter group per
    // frame.
    if copied < 1.0 {
        icon = icon.child(face(AssetId::TABLER_COPY, 1.0 - copied));
    }
    if copied > 0.0 {
        icon = icon.child(face(AssetId::TABLER_CHECK, copied));
    }
    icon
}
