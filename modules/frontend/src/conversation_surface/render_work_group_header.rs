//! Work-group header row and disclosure chevron rendering for
//! [`ConversationSurface`].
//!
//! Extracted verbatim from `render_blocks.rs`; visibility widened to the
//! surface module for its parent-owned callers.

use super::*;

impl ConversationSurface {
    /// Builds the single session header row shared by plain and disclosable
    /// renders, mirroring `conversation-work-session.svelte` §469:
    /// `relative flex w-full items-center justify-between gap-3 pb-2` with
    /// the label (or disclosure chevron) at the near end, an engine handoff
    /// at the far end, and the 1 px settled divider pinned to the bottom
    /// edge. Controlled groups carry the label tone on the chevron;
    /// uncontrolled text stays static; a headerless controlled group keeps
    /// the chevron-only affordance with an honest accessible name —
    /// disclosure chrome, never invented content.
    #[expect(
        clippy::too_many_arguments,
        reason = "the header row takes the label, transition, disclosure state, motion, and theme it paints as one unit"
    )]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the header curve samples in f64 and feeds the f32 animation clock; the narrowing is the intended precision"
    )]
    pub(in crate::conversation_surface) fn work_group_header_row(
        label: Option<String>,
        transition: Option<String>,
        fraction: f32,
        controlled: bool,
        selector: &str,
        motion: MotionPolicy,
        mounted_working: bool,
        theme: &ArtisanTheme,
    ) -> AnyElement {
        // Positional tool chains have their own disclosure header. An empty
        // session header must not leave a divider or padding above that chain.
        if !controlled && label.is_none() && transition.is_none() {
            return div().into_any_element();
        }
        let near: AnyElement = match (controlled, label) {
            (true, Some(label)) => div()
                .flex()
                .flex_row()
                .items_center()
                .gap(theme.spacing.steps(1.0))
                .min_w_0()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(label)
                .child(Self::work_group_chevron(
                    fraction,
                    16.0,
                    theme.colors.muted_foreground.to_paint(),
                ))
                .into_any_element(),
            (true, None) => div()
                .id(format!("{selector}-work-trigger"))
                .flex()
                .flex_row()
                .items_center()
                .text_color(theme.colors.muted_foreground.to_paint())
                .aria_label("Toggle work details")
                .gap(theme.spacing.steps(1.0))
                .child("Work history")
                .child(Self::work_group_chevron(
                    fraction,
                    16.0,
                    theme.colors.muted_foreground.to_paint(),
                ))
                .into_any_element(),
            (false, Some(label)) => div()
                .min_w_0()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(label)
                .into_any_element(),
            (false, None) => div().into_any_element(),
        };
        let mut header = div()
            .relative()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(theme.spacing.steps(3.0))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(theme.spacing.steps(6.0))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .letter_spacing(px(ProseTypography::body_tracking_px(
                ProseTypography::BODY_SIZE_PX,
            )))
            .text_color(theme.colors.muted_foreground.to_paint())
            .pb(theme.spacing.steps(2.0))
            .debug_selector({
                let selector = format!("{selector}-header");
                move || selector.clone()
            })
            .child(near);
        if let Some(handoff) = transition {
            header = header.child(
                div()
                    .flex_shrink_0()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(handoff),
            );
        }
        // The settled divider: the reference `t-settle-underline` base rule,
        // always painted at its exact end geometry. Growing it from the
        // measured label width has no text-measurement primitive here, so no
        // width tween is faked; the rule rides inside the entrance below
        // while one plays.
        header = header.child(
            separator(transcript_separator_color(theme), SeparatorAxis::Horizontal)
                .absolute()
                .bottom(px(0.0))
                .left(px(0.0)),
        );
        // Mounted-working entrance only: the reference `status-swap-enter`
        // plays solely for headers mounted while working, and history
        // arriving settled stays static. The frozen flag (retained window
        // state from first mount) means history that later goes live never
        // enters. The selector-keyed chain plays once per group —
        // re-renders never restart it — and settling leaves it at its end
        // state structurally. Hold plus enter mirror the reference 150 ms
        // delay and 150 ms EaseInOut run; opacity, relative 4 px rise (layout
        // neutral, like the reference translate), and 2 px blur all ride the
        // same eased clock. Reduced motion skips the wrapper and rests at
        // the unfiltered state.
        if !mounted_working {
            return header.into_any_element();
        }
        match motion.resolve(MotionRecipe::TextSwap) {
            MotionPlan::Immediate => header.into_any_element(),
            MotionPlan::Animate(animation) => header
                .opacity(0.0)
                .with_animations(
                    ElementId::Name(SharedString::from(format!("{selector}-header-enter"))),
                    vec![
                        Animation::new(MotionDuration::Quick.as_duration()),
                        animation.gpui_clock(),
                    ],
                    |header, index, value| {
                        if index == 0 {
                            header
                                .opacity(0.0)
                                .top(px(-4.0))
                                .filter(vec![Filter::Blur(px(2.0))])
                        } else {
                            let eased = MotionCurve::EaseInOut.sample(f64::from(value)) as f32;
                            header
                                .opacity(eased)
                                .top(px(-4.0 * (1.0 - eased)))
                                .filter(vec![Filter::Blur(px(2.0 * (1.0 - eased)))])
                        }
                    },
                )
                .into_any_element(),
        }
    }

    /// The same catalog chevron rotates with the panel's sampled progress.
    /// Explicit tint preserves the asset seam's monochrome color contract.
    pub(in crate::conversation_surface) fn work_group_chevron(
        fraction: f32,
        size: f32,
        color: gpui::Hsla,
    ) -> AnyElement {
        gpui::svg()
            .path(AssetId::TABLER_CHEVRON_RIGHT.as_str())
            .size(px(size))
            .text_color(color)
            .with_transformation(gpui::Transformation::rotate(gpui::radians(
                fraction * std::f32::consts::FRAC_PI_2,
            )))
            .into_any_element()
    }
}
