use std::time::Duration;

pub const DROPDOWN_OPEN: Duration = Duration::from_millis(250);
pub const DROPDOWN_CLOSE: Duration = Duration::from_millis(200);
pub(crate) const DROPDOWN_PRE_SCALE: f32 = 0.97;
const HIDDEN_OFFSET: f32 = 8.0;
#[must_use]
pub fn smooth_out(progress: f32) -> f32 {
    super::motion::MotionCurve::SmoothOut.sample(f64::from(progress)) as f32
}

#[must_use]
pub(crate) fn eased(phase: MenuPhase, progress: f32) -> f32 {
    let curve = match phase {
        MenuPhase::Closing => super::motion::MotionCurve::EaseInOut,
        MenuPhase::Hidden | MenuPhase::Opening | MenuPhase::Open => {
            super::motion::MotionCurve::SmoothOut
        }
    };
    curve.sample_f32(f64::from(progress))
}

#[must_use]
pub(crate) const fn travel_scale(phase: MenuPhase) -> f32 {
    match phase {
        MenuPhase::Closing => DROPDOWN_PRE_SCALE,
        MenuPhase::Hidden | MenuPhase::Opening | MenuPhase::Open => 1.0,
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuPhase {
    #[default]
    Hidden,
    Opening,
    Open,
    Closing,
}
#[derive(Clone, Copy, Debug)]
pub struct MenuMotion {
    phase: MenuPhase,
    current_opacity: f32,
    current_offset: f32,
    from_opacity: f32,
    from_offset: f32,
    to_opacity: f32,
    to_offset: f32,
    generation: u64,
}

impl Default for MenuMotion {
    fn default() -> Self {
        Self {
            phase: MenuPhase::Hidden,
            current_opacity: 0.0,
            current_offset: HIDDEN_OFFSET,
            from_opacity: 0.0,
            from_offset: HIDDEN_OFFSET,
            to_opacity: 0.0,
            to_offset: HIDDEN_OFFSET,
            generation: 0,
        }
    }
}

impl MenuMotion {
    pub(crate) const fn frame(self) -> (f32, f32) {
        (self.current_opacity, self.current_offset)
    }
    #[must_use]
    pub(crate) const fn phase(self) -> MenuPhase {
        self.phase
    }
    #[must_use]
    pub(crate) const fn transition(self) -> Option<(f32, f32, f32, f32, u64)> {
        match self.phase {
            MenuPhase::Opening | MenuPhase::Closing => Some((
                self.from_opacity,
                self.from_offset,
                self.to_opacity,
                self.to_offset,
                self.generation,
            )),
            MenuPhase::Hidden | MenuPhase::Open => None,
        }
    }
    pub(crate) const fn begin_open(&mut self) -> u64 {
        self.from_opacity = self.current_opacity;
        self.from_offset = self.current_offset;
        self.to_opacity = 1.0;
        self.to_offset = 0.0;
        self.phase = MenuPhase::Opening;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }
    pub(crate) const fn begin_close(&mut self) -> u64 {
        self.from_opacity = self.current_opacity;
        self.from_offset = self.current_offset;
        self.to_opacity = 0.0;
        self.to_offset = HIDDEN_OFFSET;
        self.phase = MenuPhase::Closing;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub(crate) fn apply_progress(&mut self, generation: u64, progress: f32) {
        if generation != self.generation {
            return;
        }
        let progress = progress.clamp(0.0, 1.0);
        self.current_opacity =
            (self.to_opacity - self.from_opacity).mul_add(progress, self.from_opacity);
        self.current_offset =
            (self.to_offset - self.from_offset).mul_add(progress, self.from_offset);
    }
    pub(crate) fn finish_open(&mut self, generation: u64) -> bool {
        if self.phase != MenuPhase::Opening || generation != self.generation {
            return false;
        }
        self.current_opacity = 1.0;
        self.current_offset = 0.0;
        self.phase = MenuPhase::Open;
        true
    }
    pub(crate) fn finish_close(&mut self, generation: u64) -> bool {
        if self.phase != MenuPhase::Closing || generation != self.generation {
            return false;
        }
        self.current_opacity = 0.0;
        self.current_offset = HIDDEN_OFFSET;
        self.phase = MenuPhase::Hidden;
        true
    }
    pub(crate) fn hide(&mut self) {
        *self = Self {
            generation: self.generation.wrapping_add(1),
            ..Self::default()
        };
    }
}
