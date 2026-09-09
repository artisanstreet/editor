//! Small, bounded motion state used by the native model picker.
//!
//! The picker keeps geometry and lifecycle state separate from its GPUI paint
//! closures.  That matters for two details inherited from the Electron
//! implementation: a hover pill must start an interrupted flight at its
//! currently displayed position, and a closing popover must remain mounted
//! until its finite exit has completed.

#![forbid(unsafe_code)]

/// A measured hover-pill rectangle in the coordinate space of its surface.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct HoverRect {
    /// Horizontal offset from the surface's content origin.
    pub left: f32,
    /// Vertical offset from the surface's content origin.
    pub top: f32,
    /// Measured row width.
    pub width: f32,
    /// Measured row height.
    pub height: f32,
}

impl HoverRect {
    /// Interpolates every geometric component with the same progress value.
    #[must_use]
    pub(crate) fn lerp(self, target: Self, progress: f32) -> Self {
        let progress = progress.clamp(0.0, 1.0);
        Self {
            left: self.left + (target.left - self.left) * progress,
            top: self.top + (target.top - self.top) * progress,
            width: self.width + (target.width - self.width) * progress,
            height: self.height + (target.height - self.height) * progress,
        }
    }
}

/// A single active hover-pill transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HoverTransition {
    /// The rectangle at the instant the current flight began.
    pub from: HoverRect,
    /// The rectangle the flight is approaching.
    pub to: HoverRect,
    /// Monotonic identity used to invalidate an old GPUI animation closure.
    pub generation: u64,
}

/// State for one shared hover pill.
#[derive(Clone, Debug, Default)]
pub(crate) struct SlidingHoverState {
    active_id: Option<String>,
    target_id: Option<String>,
    current: HoverRect,
    from: HoverRect,
    target: HoverRect,
    visible: bool,
    has_geometry: bool,
    animating: bool,
    generation: u64,
}

impl SlidingHoverState {
    /// Returns the row whose highlight should currently be painted.
    #[must_use]
    pub(crate) fn active_id(&self) -> Option<&str> {
        self.active_id.as_deref()
    }

    /// Returns the rectangle used for an immediate/static paint.
    #[must_use]
    pub(crate) fn visual_rect(&self) -> HoverRect {
        self.current
    }

    /// Returns whether the shared pill should be visible.
    #[must_use]
    pub(crate) const fn visible(&self) -> bool {
        self.visible
    }

    /// Marks a row as the active target without inventing geometry.
    pub(crate) fn set_active(&mut self, id: String) {
        if self.active_id.as_deref() != Some(id.as_str()) {
            self.active_id = Some(id);
        }
        self.visible = true;
    }

    /// Clears the target immediately.  The next target is a first placement,
    /// matching the source surface's `visible = false` behavior.
    pub(crate) fn clear(&mut self) {
        self.active_id = None;
        self.target_id = None;
        self.visible = false;
        self.has_geometry = false;
        self.animating = false;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Hides the pill while retaining its target and geometry, so a later
    /// `set_active` plus `measure` resumes sliding from the last displayed
    /// rectangle instead of placing instantly. Used when the pointer drifts
    /// onto blank areas inside a shared surface; a full `clear` would snap
    /// the next row-to-row flight.
    pub(crate) fn hide(&mut self) {
        self.active_id = None;
        self.visible = false;
    }

    /// Clears a target that no longer exists after a filtered/list mutation.
    pub(crate) fn clear_if_missing(&mut self, visible_ids: &[String]) {
        if self
            .active_id
            .as_ref()
            .is_some_and(|active| !visible_ids.iter().any(|id| id == active))
        {
            self.clear();
        }
    }

    /// Records a measured row.  A new row ID flies from the last displayed
    /// rectangle; remeasurement of the same row (resize/scroll) synchronizes
    /// immediately so layout changes cannot leave a stale pill behind.
    pub(crate) fn measure(&mut self, id: &str, rect: HoverRect) -> bool {
        if self.active_id.as_deref() != Some(id) {
            return false;
        }
        if self.target_id.as_deref() == Some(id) && self.target == rect && self.visible {
            return false;
        }

        let switch_target = self.target_id.as_deref() != Some(id);
        let animate = self.visible && self.has_geometry && switch_target;
        self.target_id = Some(id.to_owned());
        self.visible = true;
        self.has_geometry = true;
        self.from = self.current;
        self.target = rect;
        self.generation = self.generation.wrapping_add(1);
        if animate {
            self.animating = true;
        } else {
            self.current = rect;
            self.animating = false;
        }
        true
    }

    /// Returns the finite flight to render, if this pill is moving.
    #[must_use]
    pub(crate) fn transition(&self) -> Option<HoverTransition> {
        self.animating.then_some(HoverTransition {
            from: self.from,
            to: self.target,
            generation: self.generation,
        })
    }

    /// Applies one animation sample, ignoring stale closures from an
    /// interrupted or cleared transition.
    pub(crate) fn apply_progress(&mut self, generation: u64, progress: f32) {
        if !self.animating || generation != self.generation {
            return;
        }
        self.current = self.from.lerp(self.target, progress);
        if progress >= 1.0 {
            self.current = self.target;
            self.animating = false;
        }
    }
}

/// The finite popover lifecycle.  `Closing` deliberately remains visible so
/// the view can paint its retained exit presentation until the owner settles
/// it after the source 100 ms duration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PickerMenuPhase {
    /// The panel is not mounted.
    #[default]
    Hidden,
    /// The panel is mounting into its open presentation.
    Opening,
    /// The panel is open and interactive.
    Open,
    /// The panel is retained for its non-interactive exit.
    Closing,
}

/// Retained opacity/offset state for the picker popover.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PickerMenuMotion {
    phase: PickerMenuPhase,
    current_opacity: f32,
    current_offset: f32,
    from_opacity: f32,
    from_offset: f32,
    to_opacity: f32,
    to_offset: f32,
    generation: u64,
}

impl Default for PickerMenuMotion {
    fn default() -> Self {
        Self {
            phase: PickerMenuPhase::Hidden,
            current_opacity: 0.0,
            current_offset: 8.0,
            from_opacity: 0.0,
            from_offset: 8.0,
            to_opacity: 0.0,
            to_offset: 8.0,
            generation: 0,
        }
    }
}

impl PickerMenuMotion {
    /// Returns the lifecycle phase.
    #[must_use]
    pub(crate) const fn phase(self) -> PickerMenuPhase {
        self.phase
    }

    /// Returns the retained visual state at the latest animation sample.
    #[must_use]
    pub(crate) const fn current(self) -> (f32, f32) {
        (self.current_opacity, self.current_offset)
    }

    /// Returns the animation endpoints and generation, if a flight is active.
    #[must_use]
    pub(crate) const fn transition(self) -> Option<(f32, f32, f32, f32, u64)> {
        match self.phase {
            PickerMenuPhase::Opening | PickerMenuPhase::Closing => Some((
                self.from_opacity,
                self.from_offset,
                self.to_opacity,
                self.to_offset,
                self.generation,
            )),
            PickerMenuPhase::Hidden | PickerMenuPhase::Open => None,
        }
    }

    /// Begins or reverses the open presentation from its current displayed
    /// opacity/offset, never from a hard-coded starting frame.
    pub(crate) fn begin_open(&mut self) -> u64 {
        self.from_opacity = self.current_opacity;
        self.from_offset = self.current_offset;
        self.to_opacity = 1.0;
        self.to_offset = 0.0;
        self.phase = PickerMenuPhase::Opening;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    /// Begins or reverses the close presentation from the current frame.
    pub(crate) fn begin_close(&mut self) -> u64 {
        self.from_opacity = self.current_opacity;
        self.from_offset = self.current_offset;
        self.to_opacity = 0.0;
        self.to_offset = 8.0;
        self.phase = PickerMenuPhase::Closing;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    /// Applies one eased animation sample for the current generation.
    pub(crate) fn apply_progress(&mut self, generation: u64, progress: f32) {
        if generation != self.generation {
            return;
        }
        let progress = progress.clamp(0.0, 1.0);
        self.current_opacity = self.from_opacity + (self.to_opacity - self.from_opacity) * progress;
        self.current_offset = self.from_offset + (self.to_offset - self.from_offset) * progress;
    }

    /// Settles a completed open only if no newer transition superseded it.
    pub(crate) fn finish_open(&mut self, generation: u64) -> bool {
        if self.phase != PickerMenuPhase::Opening || generation != self.generation {
            return false;
        }
        self.current_opacity = 1.0;
        self.current_offset = 0.0;
        self.phase = PickerMenuPhase::Open;
        true
    }

    /// Drops a completed close only if no newer transition superseded it.
    pub(crate) fn finish_close(&mut self, generation: u64) -> bool {
        if self.phase != PickerMenuPhase::Closing || generation != self.generation {
            return false;
        }
        self.current_opacity = 0.0;
        self.current_offset = 8.0;
        self.phase = PickerMenuPhase::Hidden;
        true
    }

    /// Invalidates a retained surface when its parent is reopened after the
    /// surface has already been removed from the visual tree.
    pub(crate) fn hide(&mut self) {
        self.phase = PickerMenuPhase::Hidden;
        self.current_opacity = 0.0;
        self.current_offset = 8.0;
        self.from_opacity = 0.0;
        self.from_offset = 8.0;
        self.to_opacity = 0.0;
        self.to_offset = 8.0;
        self.generation = self.generation.wrapping_add(1);
    }
}

/// Bounded target state for picker wheel smoothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PickerScrollState {
    target: f32,
    active: bool,
}

impl PickerScrollState {
    /// Returns the currently requested content offset.
    #[must_use]
    pub(crate) const fn target(self) -> f32 {
        self.target
    }

    /// Adds one wheel delta, clamping to the GPUI offset range `[-max, 0]`.
    pub(crate) fn push(&mut self, current: f32, delta: f32, max: f32) {
        let base = if self.active { self.target } else { current };
        self.target = (base + delta).clamp(-max.max(0.0), 0.0);
        self.active = (self.target - current).abs() > f32::EPSILON;
    }

    /// Cancels smoothing and adopts a directly supplied native/trackpad
    /// position.
    pub(crate) fn cancel_to(&mut self, current: f32, max: f32) {
        self.target = current.clamp(-max.max(0.0), 0.0);
        self.active = false;
    }

    /// Clamps a target after content or viewport geometry changes.
    pub(crate) fn clamp_to_max(&mut self, max: f32) {
        self.target = self.target.clamp(-max.max(0.0), 0.0);
    }

    /// Takes one bounded interpolation step toward the target.
    #[must_use]
    pub(crate) fn step(&mut self, current: f32, max: f32) -> Option<f32> {
        self.clamp_to_max(max);
        let current = current.clamp(-max.max(0.0), 0.0);
        let distance = self.target - current;
        if distance.abs() <= 0.25 {
            self.active = false;
            return (self.target != current).then_some(self.target);
        }
        self.active = true;
        Some(current + distance * 0.28)
    }

    /// Returns whether an interpolation frame is still required.
    #[must_use]
    pub(crate) const fn active(self) -> bool {
        self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_first_placement_is_instant_and_interrupts_from_current() {
        let mut hover = SlidingHoverState::default();
        hover.set_active("first".to_owned());
        assert!(hover.measure(
            "first",
            HoverRect {
                left: 0.0,
                top: 0.0,
                width: 100.0,
                height: 48.0,
            },
        ));
        assert!(hover.transition().is_none());

        hover.set_active("second".to_owned());
        assert!(hover.measure(
            "second",
            HoverRect {
                left: 12.0,
                top: 52.0,
                width: 120.0,
                height: 48.0,
            },
        ));
        let transition = hover.transition().expect("second target animates");
        hover.apply_progress(transition.generation, 0.5);
        let mid = hover.visual_rect();
        assert_eq!(mid.left, 6.0);

        hover.set_active("third".to_owned());
        assert!(hover.measure(
            "third",
            HoverRect {
                left: 24.0,
                top: 104.0,
                width: 80.0,
                height: 48.0,
            },
        ));
        let interrupted = hover.transition().expect("third target animates");
        assert_eq!(interrupted.from, mid);
        hover.apply_progress(interrupted.generation, 0.25);
        hover.apply_progress(interrupted.generation, 0.5);
        assert_eq!(hover.visual_rect(), mid.lerp(interrupted.to, 0.5));
        hover.clear();
        hover.set_active("first".to_owned());
        hover.measure("first", HoverRect::default());
        assert!(
            hover.transition().is_none(),
            "reentry must place the pill instantly"
        );
    }

    #[test]
    fn menu_reopen_reverses_from_retained_frame_and_stale_finish_is_ignored() {
        let mut menu = PickerMenuMotion::default();
        let opening = menu.begin_open();
        menu.apply_progress(opening, 0.4);
        let (opacity, offset) = menu.current();
        let closing = menu.begin_close();
        let close_transition = menu.transition().expect("close transition");
        assert_eq!(close_transition.0, opacity);
        assert_eq!(close_transition.1, offset);
        assert!(!menu.finish_open(opening));
        menu.apply_progress(closing, 1.0);
        assert!(menu.finish_close(closing));
        assert_eq!(menu.phase(), PickerMenuPhase::Hidden);
    }

    #[test]
    fn scroll_targets_are_bounded_and_settle_without_an_idle_loop() {
        let mut scroll = PickerScrollState::default();
        scroll.push(0.0, -500.0, 220.0);
        assert_eq!(scroll.target(), -220.0);
        let mut current = 0.0;
        for _ in 0..64 {
            let Some(next) = scroll.step(current, 220.0) else {
                break;
            };
            current = next;
        }
        assert!(!scroll.active());
        assert_eq!(current, -220.0);

        scroll.push(current, 80.0, 220.0);
        assert_eq!(scroll.target(), -140.0);
        scroll.cancel_to(0.0, 40.0);
        assert_eq!(scroll.target(), 0.0);
        assert!(!scroll.active());
    }
}
