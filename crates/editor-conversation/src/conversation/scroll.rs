use super::adapter::ScrollDirective;
use super::controller::Conversation;
use super::fold::Row;
use super::viewport::Input;
use aqua_ui::collection::{CollectionRevealAlignment, FollowMode, FollowState};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct SmoothScroll {
    pub destination: Option<f32>,
    pub frame: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Idle,
    FollowTail,
    Scroll {
        offset: f32,
        continue_animation: bool,
    },
}

impl SmoothScroll {
    pub fn cancel(&mut self) {
        self.destination = None;
        self.frame = None;
    }

    pub fn step(
        &mut self,
        following: bool,
        reduced_motion: bool,
        landing: bool,
        disclosing: bool,
        native_following: bool,
        current: f32,
        maximum: f32,
        now: Instant,
    ) -> Step {
        if reduced_motion || landing || disclosing {
            self.cancel();
            return if following && !native_following {
                Step::FollowTail
            } else {
                Step::Idle
            };
        }
        if self.destination.is_none() && (!following || self.frame.is_none()) {
            self.frame = None;
            return if following && !native_following {
                Step::FollowTail
            } else {
                Step::Idle
            };
        }
        let max = maximum.max(0.0);
        let current = current.clamp(-max, 0.0);
        let target = self.destination.unwrap_or(-max).clamp(-max, 0.0);
        let elapsed = self
            .frame
            .replace(now)
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        let distance = target - current;
        if distance.abs() <= 0.25 {
            self.frame = None;
            self.destination = None;
            return Step::Scroll {
                offset: target,
                continue_animation: false,
            };
        }
        let fraction = 1.0 - (-elapsed.as_secs_f32() / 0.09).exp();
        Step::Scroll {
            offset: current + distance * fraction,
            continue_animation: true,
        }
    }
}

pub fn wants_older(viewport_height: f32, scroll_offset: f32) -> bool {
    viewport_height > 0.0 && -scroll_offset < 1200.0
}

pub fn scrolled(
    conversation: &Conversation,
    top: usize,
    viewport_height: f32,
    scroll_offset: f32,
    maximum: f32,
    source_prefix: bool,
    gap_shown: bool,
    mut below_viewport: impl FnMut(usize) -> bool,
) -> Vec<Input> {
    let Some(view) = conversation.view.as_ref() else {
        return Vec::new();
    };
    let mut inputs = Vec::new();
    if wants_older(viewport_height, scroll_offset) && !source_prefix {
        inputs.push(Input::TopReached);
    }
    let at_end = maximum > 0.0 && -scroll_offset >= maximum - 1.0;
    if at_end || gap_shown {
        inputs.push(Input::BottomReached);
    } else {
        let anchor = conversation
            .rows
            .iter()
            .skip(top)
            .find_map(|(row, _)| row.anchor().and_then(|id| view.position_of(id)))
            .or(view.viewport.oldest)
            .unwrap_or_default();
        let through = conversation
            .rows
            .iter()
            .enumerate()
            .skip(top)
            .take_while(|(index, _)| *index == top || !below_viewport(*index))
            .filter_map(|(_, (row, _))| row.last_anchor())
            .filter_map(|id| view.position_of(id))
            .max()
            .map_or(anchor, |through| through.max(anchor));
        inputs.push(Input::ScrolledAway { anchor, through });
    }
    inputs
}

pub fn gap_visible(rows: &[(Row, u64)], revealed_count: usize, last_below_viewport: bool) -> bool {
    revealed_count > 0
        && matches!(rows.get(revealed_count - 1), Some((Row::Gap, _)))
        && !last_below_viewport
}

pub fn row_destination(
    collection: &aqua_ui::collection_runtime::CollectionState,
    index: usize,
) -> Option<f32> {
    (index < collection.len()).then(|| -collection.offset_of(index))
}

pub const TRANSCRIPT: &str = "conversation-transcript";

pub fn following_tail(ui: &aqua::UiApp) -> bool {
    ui.is_collection_following_tail(TRANSCRIPT) == Some(true)
}

pub fn enter_tail(ui: &mut aqua::UiApp) -> bool {
    ui.collection_follow_state(TRANSCRIPT).is_some()
        && !following_tail(ui)
        && ui.set_collection_follow_mode(TRANSCRIPT, FollowMode::Tail)
}

pub fn leave_tail(ui: &mut aqua::UiApp) -> bool {
    ui.collection_follow_state(TRANSCRIPT)
        .is_some_and(|state| state != FollowState::Normal)
        && ui.set_collection_follow_mode(TRANSCRIPT, FollowMode::Normal)
}

pub fn pause_tail(ui: &mut aqua::UiApp) -> bool {
    following_tail(ui) && ui.pause_collection_following_tail(TRANSCRIPT)
}

pub fn apply_step(ui: &mut aqua::UiApp, step: Step, x: f32) -> bool {
    match step {
        Step::Idle => false,
        Step::FollowTail => enter_tail(ui),
        Step::Scroll { offset, .. } => {
            let left = leave_tail(ui);
            ui.scroll_collection_to(TRANSCRIPT, aqua_core::Point::new(x, -offset)) || left
        }
    }
}

pub fn apply_directive(
    ui: &mut aqua::UiApp,
    smooth: &mut SmoothScroll,
    directive: &ScrollDirective,
    now: Instant,
    x: f32,
) -> Option<bool> {
    match directive {
        ScrollDirective::FollowEnd => {
            smooth.cancel();
            Some(enter_tail(ui))
        }
        ScrollDirective::EaseToEnd => {
            let left = leave_tail(ui);
            smooth.destination = None;
            smooth.frame = Some(now);
            Some(left)
        }
        ScrollDirective::ToRow { index, animate } => {
            let target = ui.collection_item_target_aligned(
                TRANSCRIPT,
                *index,
                CollectionRevealAlignment::Nearest,
            )?;
            let left = leave_tail(ui);
            if *animate {
                smooth.destination = Some(-target.y);
                smooth.frame = Some(now);
                Some(left)
            } else {
                smooth.cancel();
                Some(
                    ui.scroll_collection_to(TRANSCRIPT, aqua_core::Point::new(x, target.y)) || left,
                )
            }
        }
        ScrollDirective::Offset(offset) => {
            smooth.cancel();
            let left = leave_tail(ui);
            Some(ui.scroll_collection_to(TRANSCRIPT, aqua_core::Point::new(x, *offset)) || left)
        }
    }
}
