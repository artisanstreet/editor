//! Native context-window gauge and details for the composer controls.
//!
//! The parent owns the usage subscription and supplies one immutable snapshot
//! from the run that reported it. This module is deliberately not a catalog
//! fallback: a context window is painted only when the reported run still
//! matches the parent's current usage owner and the wire supplied both token
//! values. Model name and capacity therefore describe the reporting run, never
//! the model selected for a subsequent launch.
//!
//! [`NativeContextUsage`] is the data seam. [`NativeContextUsage::presentation`]
//! is the pure validation/projection boundary and reuses the existing context
//! percentage, tone, auto-compaction, details, description, and gauge policies.
//! The rendering helpers are intentionally focused: callers can put the ring
//! beside a model picker and the details in a controlled [`Popover`].

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_ui::{
    popover::{Popover, PopoverAlign, PopoverChangeReason, PopoverSide, PopoverVariant},
    progress::{ProgressFraction, progress},
    theme::{ArtisanTheme, RadiusStep, RadiusTokens},
};
use gpui::prelude::{InteractiveElement as _, ParentElement as _, Styled as _};
use gpui::{
    App, Bounds, ElementId, FocusHandle, Hsla, Path, PathBuilder, Pixels, Stateful, Window, canvas,
    div, point, px,
};

use crate::context_auto_compaction::{
    ContextUsageAggregate as AutoCompactionUsage, ContextUsageOrigin as AutoCompactionOrigin,
    context_usage_auto_compaction_percent,
};
use crate::context_usage_description::{
    ContextUsageAggregate as DescriptionUsage, context_usage_description,
};
use crate::context_usage_details_policy::ContextUsageDetails;
pub use crate::context_usage_gauge_policy::CONTEXT_USAGE_DESCRIPTION_ID;
use crate::context_usage_gauge_policy::{ContextUsageGaugeInput, project_context_usage_gauge};
use crate::context_usage_tone::{GaugeToneMix, context_gauge_tone_mix, context_usage_percent_opt};

/// Stable selector for the context gauge popover root.
pub const CONTEXT_USAGE_SELECTOR: &str = "artisan-native-context-usage";
/// Stable selector for the ring trigger inside the popover.
pub const CONTEXT_USAGE_RING_SELECTOR: &str = "artisan-native-context-usage-ring";
/// Stable selector for the details card.
pub const CONTEXT_USAGE_DETAILS_SELECTOR: &str = "artisan-native-context-usage-details";
const RING_EDGE_PX: f32 = 16.0;
const RING_STROKE_PX: f32 = 2.5;
const RING_RADIUS_PX: f32 = 5.75;
const RING_START_DEGREES: f32 = -90.0;
const FULL_RING_EPSILON_DEGREES: f32 = 0.01;

/// One parent-owned context usage observation.
///
/// Every field is copied exactly from the reporting run or its authoritative
/// run subscription. `reporting_run_id` is the fence that prevents an older
/// stream update from being painted after the parent moved to another run.
/// The model name and window are required reporting facts at projection time;
/// this module never derives them from a next-launch policy or catalog.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativeContextUsage {
    /// Run scope that produced the observation.
    pub reporting_run_id: String,
    /// Engine/harness that produced the observation.
    pub reporting_engine_id: String,
    /// Provider-native model id that produced the observation.
    pub reporting_model_id: String,
    /// User-facing name of the model that produced the observation.
    pub reporting_model_name: String,
    /// Latest context usage from the provider, in tokens.
    pub context_tokens: Option<u64>,
    /// Context capacity from the same provider observation, in tokens.
    pub context_window_tokens: Option<u64>,
    /// Optional input-token breakdown from that observation.
    pub input_tokens: Option<u64>,
    /// Optional cached-input breakdown from that observation.
    pub cached_input_tokens: Option<u64>,
    /// Optional output-token breakdown from that observation.
    pub output_tokens: Option<u64>,
}

impl NativeContextUsage {
    /// Creates a report with the minimum fields needed for a gauge.
    #[must_use]
    pub fn new(
        reporting_run_id: impl Into<String>,
        reporting_engine_id: impl Into<String>,
        reporting_model_id: impl Into<String>,
        reporting_model_name: impl Into<String>,
        context_tokens: Option<u64>,
        context_window_tokens: Option<u64>,
    ) -> Self {
        Self {
            reporting_run_id: reporting_run_id.into(),
            reporting_engine_id: reporting_engine_id.into(),
            reporting_model_id: reporting_model_id.into(),
            reporting_model_name: reporting_model_name.into(),
            context_tokens,
            context_window_tokens,
            input_tokens: None,
            cached_input_tokens: None,
            output_tokens: None,
        }
    }

    /// Adds the optional token breakdown without changing the report identity.
    #[must_use]
    pub const fn with_breakdown(
        mut self,
        input_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    ) -> Self {
        self.input_tokens = input_tokens;
        self.cached_input_tokens = cached_input_tokens;
        self.output_tokens = output_tokens;
        self
    }

    /// Returns whether the report belongs to the parent's current run owner.
    #[must_use]
    pub fn belongs_to_run(&self, current_run_id: Option<&str>) -> bool {
        let Some(current_run_id) = current_run_id else {
            return false;
        };
        !current_run_id.is_empty()
            && !self.reporting_run_id.is_empty()
            && current_run_id == self.reporting_run_id
    }

    /// Projects a renderable reading, or hides it when the report is not
    /// attributable to the current run or lacks a valid denominator.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "token counters are converted to f64 for the shared percentage and auto-compaction policy; u64 counters stay far below 2^53 in practice"
    )]
    pub fn presentation(
        &self,
        current_run_id: Option<&str>,
    ) -> Option<NativeContextUsagePresentation> {
        if !self.belongs_to_run(current_run_id)
            || self.reporting_engine_id.trim().is_empty()
            || self.reporting_model_id.trim().is_empty()
            || self.reporting_model_name.trim().is_empty()
        {
            return None;
        }

        let context_tokens = self.context_tokens?;
        let context_window_tokens = self.context_window_tokens?;
        if context_window_tokens == 0 {
            return None;
        }

        let context_tokens_f64 = context_tokens as f64;
        let context_window_tokens_f64 = context_window_tokens as f64;
        let percent =
            context_usage_percent_opt(Some(context_tokens_f64), Some(context_window_tokens_f64))?;

        let auto_compaction_usage = AutoCompactionUsage::new(
            Some(AutoCompactionOrigin::new(
                Some(&self.reporting_engine_id),
                Some(&self.reporting_model_id),
            )),
            Some(context_tokens_f64),
        );
        let auto_compaction_percent = context_usage_auto_compaction_percent(
            Some(&auto_compaction_usage),
            context_window_tokens_f64,
        );
        if !auto_compaction_percent.is_finite() {
            return None;
        }

        let description_usage = DescriptionUsage {
            context_tokens: Some(context_tokens),
            input_tokens: self.input_tokens,
            cached_input_tokens: self.cached_input_tokens,
            output_tokens: self.output_tokens,
        };
        let description = context_usage_description(&description_usage, context_window_tokens);
        let gauge = project_context_usage_gauge(ContextUsageGaugeInput::new(
            &description,
            percent,
            auto_compaction_percent,
            Some(&self.reporting_model_name),
            context_window_tokens_f64,
        ));
        let details = ContextUsageDetails::new(
            Some(&self.reporting_model_name),
            percent,
            context_window_tokens_f64,
        )
        .ok()?;

        Some(NativeContextUsagePresentation {
            reporting_run_id: self.reporting_run_id.clone(),
            reporting_engine_id: self.reporting_engine_id.clone(),
            reporting_model_id: self.reporting_model_id.clone(),
            model_name: self.reporting_model_name.clone(),
            context_tokens,
            window_tokens: context_window_tokens,
            percent,
            compaction_percent: Some(auto_compaction_percent),
            aria_label: gauge.trigger.aria_label,
            description,
            details,
        })
    }

    /// Renders the ring/details popover when this report is valid for the
    /// parent's current run.
    ///
    /// The `open` value and open-change callback remain controlled by the
    /// parent. The callback is called for pointer, keyboard, outside, and
    /// Escape changes by the shared native [`Popover`].
    #[must_use]
    pub fn render_popover(
        &self,
        current_run_id: Option<&str>,
        theme: ArtisanTheme,
        focus: FocusHandle,
        open: bool,
        disabled: bool,
        on_open_change: impl Fn(bool, PopoverChangeReason, &mut Window, &mut App) + 'static,
    ) -> Option<Popover> {
        let presentation = self.presentation(current_run_id)?;
        let trigger = render_ring_trigger(&presentation, &theme);
        let details = render_details(&presentation, &theme);

        Some(
            Popover::new(CONTEXT_USAGE_SELECTOR, focus, theme, open, trigger, details)
                .variant(PopoverVariant::Default)
                .side(PopoverSide::Top)
                .align(PopoverAlign::Start)
                .side_offset(px(8.0))
                .disabled(disabled)
                .debug_selector(CONTEXT_USAGE_SELECTOR)
                .on_open_change(on_open_change),
        )
    }
}

/// Owned render facts for a valid reporting observation.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeContextUsagePresentation {
    /// Reporting run identity retained for parent event fencing.
    pub reporting_run_id: String,
    /// Reporting engine identity.
    pub reporting_engine_id: String,
    /// Reporting model identity.
    pub reporting_model_id: String,
    /// Model name from the reporting run.
    pub model_name: String,
    /// Numerator from the reporting run.
    pub context_tokens: u64,
    /// Denominator from the reporting run.
    pub window_tokens: u64,
    /// Finite clamped context fullness percentage.
    pub percent: f64,
    /// Documented auto-compaction boundary, when the policy supplies one.
    pub compaction_percent: Option<f64>,
    /// Accessible trigger label from the shared gauge policy.
    pub aria_label: String,
    /// Persistent screen-reader description from the shared policy.
    pub description: String,
    /// Details-card prose and progress facts from the shared policy.
    pub details: ContextUsageDetails,
}

impl NativeContextUsagePresentation {
    /// Returns the shared context gauge tone legs for the finite percentage.
    #[must_use]
    pub fn tone_mix(&self) -> GaugeToneMix {
        context_gauge_tone_mix(self.percent, self.compaction_percent.unwrap_or(100.0))
    }

    /// Returns the normalized progress share used by the details bar.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the details fill ratio is f64 policy math narrowed to the f32 value the GPUI progress bar paints"
    )]
    pub fn progress_fraction(&self) -> ProgressFraction {
        ProgressFraction::new((self.details.fill().value() / self.details.fill().max()) as f32)
    }
}

/// Returns the context details card for a previously validated presentation.
#[must_use]
pub fn render_details(
    presentation: &NativeContextUsagePresentation,
    theme: &ArtisanTheme,
) -> Stateful<gpui::Div> {
    let details = &presentation.details;
    div()
        .id(ElementId::Name(CONTEXT_USAGE_DETAILS_SELECTOR.into()))
        .debug_selector(|| CONTEXT_USAGE_DETAILS_SELECTOR.to_owned())
        .flex()
        .flex_col()
        .gap(px(12.0))
        .text_color(theme.colors.popover_foreground.to_paint())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(theme.typography.control_text)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Context Window"),
                )
                .child(
                    div()
                        .text_size(theme.typography.label_text)
                        .line_height(px(19.0))
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(details.percent().sentence().to_owned()),
                )
                .child(
                    div()
                        .text_size(theme.typography.label_text)
                        .line_height(px(19.0))
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(details.capacity().sentence().to_owned()),
                ),
        )
        .child(
            progress(
                artisan_ui::progress::ProgressStyle::resolve(*theme),
                presentation.progress_fraction(),
            )
            .debug_selector(|| format!("{CONTEXT_USAGE_DETAILS_SELECTOR}-progress")),
        )
}

fn render_ring_trigger(
    presentation: &NativeContextUsagePresentation,
    theme: &ArtisanTheme,
) -> Stateful<gpui::Div> {
    let tone = presentation.tone_mix();
    let ring_color = ring_color(theme, tone);
    let track_color = theme.colors.foreground.with_alpha(0.16).to_paint();
    let percent = finite_percent(presentation.percent);
    let ring_selector = CONTEXT_USAGE_RING_SELECTOR.to_owned();

    let arc = canvas(
        move |bounds, _, _| build_ring_path(bounds, percent),
        move |_, path: Option<Path<Pixels>>, window, _| {
            if let Some(path) = path {
                window.paint_path(path, ring_color);
            }
        },
    )
    .absolute()
    .top(px(0.0))
    .right(px(0.0))
    .bottom(px(0.0))
    .left(px(0.0));

    let ring = div()
        .relative()
        .size(px(RING_EDGE_PX))
        .flex()
        .items_center()
        .justify_center()
        .rounded(RadiusTokens::value(RadiusStep::Sm))
        .border_2()
        .border_color(track_color)
        .child(arc);

    div()
        .id(ElementId::Name(CONTEXT_USAGE_RING_SELECTOR.into()))
        .debug_selector(move || ring_selector.clone())
        .size(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(RadiusTokens::value(RadiusStep::Sm))
        .child(ring)
}

fn build_ring_path(bounds: Bounds<Pixels>, percent: f32) -> Option<Path<Pixels>> {
    if percent <= 0.0 {
        return None;
    }

    let center = point(
        bounds.origin.x + bounds.size.width / 2.0,
        bounds.origin.y + bounds.size.height / 2.0,
    );
    let end_degrees = RING_START_DEGREES + (percent * 360.0).min(360.0 - FULL_RING_EPSILON_DEGREES);
    let mut builder = PathBuilder::stroke(px(RING_STROKE_PX));
    builder.move_to(point_on_circle(
        center,
        px(RING_RADIUS_PX),
        RING_START_DEGREES,
    ));
    builder.arc_to(
        point(px(RING_RADIUS_PX), px(RING_RADIUS_PX)),
        px(0.0),
        percent >= 0.5,
        true,
        point_on_circle(center, px(RING_RADIUS_PX), end_degrees),
    );
    builder.build().ok()
}

fn point_on_circle(
    center: gpui::Point<Pixels>,
    radius: Pixels,
    degrees: f32,
) -> gpui::Point<Pixels> {
    let radians = degrees.to_radians();
    let radius = f32::from(radius);
    point(
        center.x + px(radius * radians.cos()),
        center.y + px(radius * radians.sin()),
    )
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the clamped percentage is narrowed from f64 policy math to the f32 arc geometry GPUI paints"
)]
fn finite_percent(percent: f64) -> f32 {
    if percent.is_finite() {
        percent.clamp(0.0, 100.0) as f32 / 100.0
    } else {
        0.0
    }
}

fn ring_color(theme: &ArtisanTheme, tone: GaugeToneMix) -> Hsla {
    if tone.danger > 0 {
        theme.colors.banner_error.to_paint()
    } else if tone.warn > 0 {
        theme.colors.banner_warning.to_paint()
    } else {
        theme.colors.banner_info.to_paint()
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::float_cmp, reason = "test assertions compare the exact pixel arithmetic the UI performs; an epsilon would weaken the regression coverage")]
    use super::{CONTEXT_USAGE_DESCRIPTION_ID, NativeContextUsage, NativeContextUsagePresentation};
    use crate::context_usage_details_policy::ContextUsageDetailsError;

    fn usage() -> NativeContextUsage {
        NativeContextUsage::new(
            "run-1",
            "codex",
            "gpt-5.6-luna",
            "Luna",
            Some(90_000),
            Some(200_000),
        )
        .with_breakdown(Some(60_000), Some(10_000), Some(20_000))
    }

    #[test]
    fn stale_or_missing_run_identity_hides_the_gauge() {
        let usage = usage();
        assert!(usage.presentation(None).is_none());
        assert!(usage.presentation(Some("run-2")).is_none());
        assert!(usage.presentation(Some("")).is_none());
        assert!(usage.presentation(Some("run-1")).is_some());
    }

    #[test]
    fn missing_reporting_identity_and_zero_window_hide_without_fallback() {
        let mut missing_model = usage();
        missing_model.reporting_model_name.clear();
        assert!(missing_model.presentation(Some("run-1")).is_none());

        let mut zero_window = usage();
        zero_window.context_window_tokens = Some(0);
        assert!(zero_window.presentation(Some("run-1")).is_none());

        let mut missing_tokens = usage();
        missing_tokens.context_tokens = None;
        assert!(missing_tokens.presentation(Some("run-1")).is_none());
    }

    #[test]
    fn presentation_clamps_percent_and_keeps_reporting_model_and_window() {
        let mut usage = usage();
        usage.context_tokens = Some(300_000);
        usage.context_window_tokens = Some(200_000);
        let presentation = usage.presentation(Some("run-1")).expect("valid report");
        assert_eq!(presentation.model_name, "Luna");
        assert_eq!(presentation.window_tokens, 200_000);
        assert_eq!(presentation.percent, 100.0);
        assert_eq!(presentation.aria_label, "Context window 100% full");
        assert_eq!(
            presentation.description,
            "Context window contains 300,000 of 200,000 tokens. Input: 60,000 tokens. Cached input: 10,000 tokens. Output: 20,000 tokens."
        );
        assert_eq!(CONTEXT_USAGE_DESCRIPTION_ID, "context-usage-details");
    }

    #[test]
    fn compaction_marker_is_policy_derived_and_optional_in_the_view() {
        let presentation = usage().presentation(Some("run-1")).expect("valid report");
        assert_eq!(presentation.compaction_percent, Some(90.0));
        assert_eq!(presentation.tone_mix().danger, 0);
        assert!(presentation.progress_fraction().value() > 0.0);

        let cloned: NativeContextUsagePresentation = presentation.clone();
        assert_eq!(cloned.reporting_run_id, "run-1");
        assert_eq!(cloned.reporting_engine_id, "codex");
        assert_eq!(cloned.reporting_model_id, "gpt-5.6-luna");
    }

    #[test]
    fn invalid_details_are_not_recovered_with_fabricated_text() {
        let error = super::ContextUsageDetails::new(Some("Luna"), f64::NAN, 200_000.0)
            .expect_err("NaN is not a displayable report");
        assert!(matches!(error, ContextUsageDetailsError::NonFinitePercent));
    }
}
