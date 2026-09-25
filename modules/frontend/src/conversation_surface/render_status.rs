//! Turn status row rendering for [`ConversationSurface`]: the live thinking
//! line, narration verbs, and settled durations.
//!
//! Extracted from `render_sections.rs`; copy, visibility, and the summary
//! cadence all resolve through the one engine-aware summary policy in
//! `contract.rs`, so paint and scroll identities stay one-to-one.

use super::*;

impl ConversationSurface {
    pub(in crate::conversation_surface) fn render_status(
        &self,
        turn_id: &TurnId,
        block: &crate::conversation_scene::TurnStatusBlock,
        selector: String,
        theme: &ArtisanTheme,
        status_motion: MotionPolicy,
    ) -> Option<AnyElement> {
        // The terminal duration prefers the work-group header when the turn
        // carries the group; the reference settles to the header alone.
        let turn_scene = self.scene.turn_scene(turn_id);
        let turn_has_work_group = turn_scene.is_some_and(|turn| {
            turn.blocks()
                .iter()
                .any(|block| matches!(block, TurnBlock::WorkGroup(_)))
        });
        // The thinking line is the scene summary reduced to one line by the
        // turn's typed engine policy when one rides the block; settled rows
        // never carry it (builder guarantee). A summary that reduces to
        // nothing falls back to the narration, exactly like the reference.
        // Otherwise the narration supplies the verb, with the engine-named
        // wait for a known provider. Copy and cadence share this reduction.
        let has_summary =
            status_summary_copy(block.reasoning_summary.as_deref(), block.engine).is_some();
        let copy = turn_status_block_copy(block, self.active_now_ms)?;
        // A live line identical to the owning group header paints once, in
        // the header; a distinct narration (a summary counts) still paints.
        // Render and scroll identities share this exact decision.
        let owner_header = if matches!(
            block.narration,
            TurnNarration::Thinking | TurnNarration::Working
        ) {
            turn_scene.and_then(|turn| turn_owner_header(turn, self.active_now_ms))
        } else {
            None
        };
        if !turn_status_paints(
            turn_has_work_group,
            block.narration,
            Some(copy.as_str()),
            owner_header.as_deref(),
        ) {
            return None;
        }
        // Base-size muted copy. The effective motion resolves the live
        // window signal at render time (see `effective_status_motion`); the
        // shimmer animates only for live rows under `Full` and stays
        // immediate for settled history and reduced motion. A summary sweeps
        // with the summary cadence and parses inline marks through the
        // frozen text-runs contract (faces survive the band identically
        // under Full and Reduced); verbs keep the verb cadence.
        let live = matches!(
            block.narration,
            TurnNarration::Thinking
                | TurnNarration::Working
                | TurnNarration::ProviderWait
                | TurnNarration::Compacting
                | TurnNarration::BackgroundWait
        );
        let content: AnyElement = if has_summary {
            // Faces ride the shared shimmer through the frozen text-runs
            // contract: the sweep recolors while family and zero tracking
            // compile at layout, identically under Full and Reduced, with
            // selection retained per stable id.
            let runs = inline_runs(&copy, *theme);
            ShimmerText::new(runs.text, *theme, status_motion)
                .text_runs(
                    format!("{selector}-summary"),
                    runs.highlights,
                    runs.overrides,
                )
                .active(live)
                .delay_seconds(0.0)
                .duration_seconds(2.0)
                .text_color(status_color(theme, block.narration))
                .into_element()
        } else {
            ShimmerText::new(copy, *theme, status_motion)
                .active(live)
                .delay_seconds(1.5)
                .duration_seconds(3.0)
                .text_color(status_color(theme, block.narration))
                .into_element()
        };
        let mut status = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            // The turn already supplies 24 px between blocks. Following work
            // history, use 12 px for the live line instead of adding another
            // 8 px on top; the final reply uses the tighter 8 px seam.
            .mt(if turn_has_work_group && live {
                -theme.spacing.steps(3.0)
            } else {
                theme.spacing.steps(2.0)
            })
            .mb(theme.spacing.steps(2.0))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(ProseTypography::BODY_LINE_PX))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .letter_spacing(px(ProseTypography::BODY_TRACKING_PX))
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(content);
        status = status.debug_selector(move || selector.clone());
        Some(status.into_any_element())
    }
}
