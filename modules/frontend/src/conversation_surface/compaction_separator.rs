//! The compaction separator: a full-width rule with a centred label that
//! marks where the engine compacted the conversation, a checkpoint the turn
//! continues past.
//!
//! While the engine compacts, the label reads "Compacting" under the shared
//! shimmer; when it finishes, the word swaps to "Compacted" and settles. A
//! compaction the turn outlived reads "Compaction stopped". Motion follows
//! the transitions-dev tokens and plays only for a separator that appears
//! live (see [`super::work_motion`]); history paints at rest:
//!
//! - Entering, the row opens its height first (250 ms smooth-out) so nothing
//!   below jumps; then both rules draw outward from the label (400 ms
//!   smooth-out), and the label rises in 80 ms after them (250 ms, 4 px,
//!   3 px blur).
//! - Settling, the word swaps with the text-swap recipe (150 ms out and
//!   150 ms in, ease-in-out, 4 px, 2 px blur), then one soft highlight runs
//!   from the label to both ends of the rule (500 ms): the checkpoint.
//!
//! The label sits in a slot as wide as the widest word it can show, so the
//! rules keep their length when the word changes.

use std::time::{Duration, Instant};

use gpui::{ColorExt as _, prelude::FluentBuilder as _, relative};

use super::*;

/// Row height: one line of body text.
const ROW_HEIGHT_PX: f32 = 24.0;
/// Space between the label and each rule.
const LABEL_GAP_PX: f32 = 12.0;
/// The checkpoint highlight's length along a rule.
const PULSE_LENGTH_PX: f32 = 56.0;
/// The highlight's peak opacity over the rule.
const PULSE_PEAK_ALPHA: f32 = 0.55;

const OPEN: Duration = Duration::from_millis(250);
const DRAW_DELAY: Duration = Duration::from_millis(150);
const DRAW: Duration = Duration::from_millis(400);
const LABEL_DELAY: Duration = Duration::from_millis(230);
const LABEL_RISE: Duration = Duration::from_millis(250);
const SWAP_HALF: Duration = Duration::from_millis(150);
const PULSE: Duration = Duration::from_millis(500);

/// One separator's motion, retained by its turn row.
#[derive(Debug)]
pub(super) struct CompactionMotion {
    phase: CompactionPhase,
    /// When the separator appeared live, while its entrance plays.
    entered: Option<Instant>,
    /// When the compaction settled live, and the label it settled from.
    settled: Option<(Instant, CompactionPhase)>,
}

/// One frame of a separator's motion. Every progress runs 0 to 1 and rests
/// at 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CompactionFrame {
    /// Row height.
    pub open: f32,
    /// Rule length, from the label outward.
    pub draw: f32,
    /// Label rise-in.
    pub label: f32,
    /// The label being swapped out and the swap's progress over both halves.
    pub swap: Option<(CompactionPhase, f32)>,
    /// The checkpoint highlight's run along the rules.
    pub pulse: Option<f32>,
}

impl CompactionFrame {
    /// The separator at rest.
    pub(super) const REST: Self = Self {
        open: 1.0,
        draw: 1.0,
        label: 1.0,
        swap: None,
        pulse: None,
    };

    fn moving(self) -> bool {
        self != Self::REST
    }
}

impl CompactionMotion {
    /// Starts tracking a separator first painted at `phase`. It enters only
    /// when it appears live and running, under full motion.
    pub(super) fn first_seen(phase: CompactionPhase, full_motion: bool, now: Instant) -> Self {
        Self {
            phase,
            entered: (full_motion && phase == CompactionPhase::Running).then_some(now),
            settled: None,
        }
    }

    /// Advances to this frame's phase; returns what to paint and whether
    /// anything still moves.
    pub(super) fn advance(
        &mut self,
        phase: CompactionPhase,
        full_motion: bool,
        now: Instant,
    ) -> (CompactionFrame, bool) {
        if phase != self.phase {
            // Only a compaction watched while it ran settles with motion; a
            // stop has no checkpoint and swaps at once.
            if full_motion
                && self.phase == CompactionPhase::Running
                && phase == CompactionPhase::Done
            {
                self.settled = Some((now, self.phase));
            } else {
                self.settled = None;
            }
            self.phase = phase;
        }
        if !full_motion {
            self.entered = None;
            self.settled = None;
        }
        let mut frame = CompactionFrame::REST;
        if let Some(entered) = self.entered {
            let elapsed = now.saturating_duration_since(entered);
            frame.open = eased(
                MotionCurve::SmoothOut,
                progress(elapsed, Duration::ZERO, OPEN),
            );
            frame.draw = eased(MotionCurve::SmoothOut, progress(elapsed, DRAW_DELAY, DRAW));
            frame.label = eased(
                MotionCurve::SmoothOut,
                progress(elapsed, LABEL_DELAY, LABEL_RISE),
            );
            if elapsed >= (DRAW_DELAY + DRAW).max(LABEL_DELAY + LABEL_RISE) {
                self.entered = None;
            }
        }
        if let Some((settled, from)) = self.settled {
            let elapsed = now.saturating_duration_since(settled);
            let swap = progress(elapsed, Duration::ZERO, SWAP_HALF * 2);
            if swap < 1.0 {
                frame.swap = Some((from, swap));
            }
            let pulse = progress(elapsed, SWAP_HALF * 2, PULSE);
            if pulse > 0.0 && pulse < 1.0 {
                frame.pulse = Some(pulse);
            }
            if elapsed >= SWAP_HALF * 2 + PULSE {
                self.settled = None;
            }
        }
        (
            frame,
            frame.moving() || self.entered.is_some() || self.settled.is_some(),
        )
    }
}

/// Linear progress of a clock that starts `delay` in and runs `duration`.
fn progress(elapsed: Duration, delay: Duration, duration: Duration) -> f32 {
    let Some(run) = elapsed.checked_sub(delay) else {
        return 0.0;
    };
    (run.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

#[expect(clippy::cast_possible_truncation, reason = "progress stays in 0..=1")]
fn eased(curve: MotionCurve, value: f32) -> f32 {
    curve.sample(f64::from(value)) as f32
}

impl TurnRowView {
    /// Renders one compaction separator with its scroll anchor.
    pub(super) fn render_compaction_separator(
        &self,
        id: &SceneId,
        phase: CompactionPhase,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let frame = self.motion.borrow_mut().compaction(id, phase);
        let muted = theme.colors.muted_foreground.to_paint();
        let rule_color = transcript_separator_color(theme);

        // The slot keeps the widest word it can show in flow, invisibly; the
        // visible words sit centred over it.
        let widest = match phase {
            CompactionPhase::Stopped => CompactionPhase::Stopped.label(),
            CompactionPhase::Running | CompactionPhase::Done => CompactionPhase::Running.label(),
        };
        let word = |phase: CompactionPhase| {
            div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .flex()
                .justify_center()
                .child(
                    ShimmerText::new(phase.label(), *theme, self.status_motion)
                        .active(phase == CompactionPhase::Running)
                        .delay_seconds(0.0)
                        .duration_seconds(2.0)
                        .text_color(muted)
                        .into_element(),
                )
        };
        let mut slot = div()
            .relative()
            .flex_shrink_0()
            .child(div().opacity(0.0).child(widest));
        slot = match frame.swap {
            Some((from, swap)) if swap < 0.5 => {
                let out = eased(MotionCurve::EaseInOut, swap * 2.0);
                slot.child(
                    word(from)
                        .opacity(1.0 - out)
                        .top(px(-4.0 * out))
                        .filter(vec![Filter::Blur(px(2.0 * out))]),
                )
            }
            Some((_, swap)) => {
                let enter = eased(MotionCurve::EaseInOut, swap * 2.0 - 1.0);
                slot.child(
                    word(phase)
                        .opacity(enter)
                        .top(px(4.0 * (1.0 - enter)))
                        .filter(vec![Filter::Blur(px(2.0 * (1.0 - enter)))]),
                )
            }
            None if frame.label < 1.0 => slot.child(
                word(phase)
                    .opacity(frame.label)
                    .top(px(4.0 * (1.0 - frame.label)))
                    .filter(vec![Filter::Blur(px(3.0 * (1.0 - frame.label)))]),
            ),
            None => slot.child(word(phase)),
        };

        // Each rule grows from the label outward; the checkpoint highlight
        // runs the same way and fades as it goes.
        let rule = |outward_left: bool| {
            let mut track = div()
                .flex_1()
                .min_w_0()
                .h(px(1.0))
                .relative()
                .flex()
                .when(outward_left, |track| track.justify_end())
                .child(div().h_full().w(relative(frame.draw)).bg(rule_color));
            if let Some(pulse) = frame.pulse {
                let travel = eased(MotionCurve::SmoothOut, pulse);
                let alpha = PULSE_PEAK_ALPHA * (std::f32::consts::PI * pulse).sin();
                let highlight = div()
                    .absolute()
                    .top(px(0.0))
                    .h_full()
                    .w(px(PULSE_LENGTH_PX))
                    .bg(theme.colors.foreground.to_paint().opacity(alpha));
                // The highlight travels a lane one highlight shorter than the
                // rule, so it starts at the label and ends flush with the end.
                let lane = div().absolute().top(px(0.0)).h_full();
                track = track.child(if outward_left {
                    lane.left(px(PULSE_LENGTH_PX))
                        .right(px(0.0))
                        .child(highlight.right(relative(travel)))
                } else {
                    lane.left(px(0.0))
                        .right(px(PULSE_LENGTH_PX))
                        .child(highlight.left(relative(travel)))
                });
            }
            track
        };

        let row = div()
            .w_full()
            .min_w_0()
            .h(px(ROW_HEIGHT_PX * frame.open))
            .overflow_hidden()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(LABEL_GAP_PX))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(ROW_HEIGHT_PX))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .tracking(ProseTypography::TRACKING_EM)
            .text_color(muted)
            .child(rule(true))
            .child(slot)
            .child(rule(false))
            .debug_selector({
                let selector = selector.clone();
                move || selector
            });
        anchors
            .attach(
                div().w_full().min_w_0(),
                Some(id),
                item_id_for_scene_id(id).as_ref(),
            )
            .debug_selector(move || format!("{selector}-compaction"))
            .child(row)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    #[test]
    fn history_paints_at_rest() {
        let start = Instant::now();
        for phase in [
            CompactionPhase::Running,
            CompactionPhase::Done,
            CompactionPhase::Stopped,
        ] {
            let mut motion = CompactionMotion::first_seen(phase, false, start);
            assert_eq!(
                motion.advance(phase, false, start),
                (CompactionFrame::REST, false)
            );
        }
        // A separator first seen settled never enters, even live.
        let mut motion = CompactionMotion::first_seen(CompactionPhase::Done, true, start);
        assert_eq!(
            motion.advance(CompactionPhase::Done, true, start),
            (CompactionFrame::REST, false)
        );
    }

    #[test]
    fn a_live_compaction_opens_draws_then_labels() {
        let start = Instant::now();
        let mut motion = CompactionMotion::first_seen(CompactionPhase::Running, true, start);
        let (first, moving) = motion.advance(CompactionPhase::Running, true, start);
        assert!(moving);
        assert_eq!((first.open, first.draw, first.label), (0.0, 0.0, 0.0));
        // The space opens before the rules draw, and they lead the label.
        let (early, _) = motion.advance(CompactionPhase::Running, true, at(start, 140));
        assert!(early.open > 0.5 && early.draw == 0.0 && early.label == 0.0);
        let (mid, _) = motion.advance(CompactionPhase::Running, true, at(start, 300));
        assert!(mid.open == 1.0 && mid.draw > 0.0 && mid.label > 0.0 && mid.draw > mid.label);
        let (rest, moving) = motion.advance(CompactionPhase::Running, true, at(start, 600));
        assert_eq!(rest, CompactionFrame::REST);
        assert!(!moving);
    }

    #[test]
    fn settling_swaps_the_word_then_pulses_once() {
        let start = Instant::now();
        let mut motion = CompactionMotion::first_seen(CompactionPhase::Running, true, start);
        motion.advance(CompactionPhase::Running, true, at(start, 1_000));
        let done = at(start, 2_000);
        let (swap_out, _) = motion.advance(CompactionPhase::Done, true, done);
        assert_eq!(swap_out.swap, Some((CompactionPhase::Running, 0.0)));
        assert_eq!(swap_out.pulse, None);
        let (pulse, moving) = motion.advance(CompactionPhase::Done, true, at(done, 550));
        assert_eq!(pulse.swap, None);
        assert!(pulse.pulse.is_some_and(|value| value > 0.0 && value < 1.0));
        assert!(moving);
        let (rest, moving) = motion.advance(CompactionPhase::Done, true, at(done, 800));
        assert_eq!(rest, CompactionFrame::REST);
        assert!(!moving);
    }

    #[test]
    fn a_stop_swaps_at_once_without_a_checkpoint() {
        let start = Instant::now();
        let mut motion = CompactionMotion::first_seen(CompactionPhase::Running, true, start);
        motion.advance(CompactionPhase::Running, true, at(start, 1_000));
        // The turn ended, so the row is no longer under full live motion.
        let (frame, moving) = motion.advance(CompactionPhase::Stopped, false, at(start, 2_000));
        assert_eq!(frame, CompactionFrame::REST);
        assert!(!moving);
    }
}
