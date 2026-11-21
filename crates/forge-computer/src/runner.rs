use crate::{
    desktop::{
        Button, CaptureError, CursorError, Desktop, DisplayError, Input, InputError, Point, Rect,
        WindowChange, WindowError,
    },
    frame,
    steps::{At, Key, Step, Wait},
};
use std::num::NonZeroU32;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub(crate) rect: Rect,
    pub(crate) width: NonZeroU32,
    pub(crate) height: NonZeroU32,
}

#[derive(Debug, thiserror::Error)]
#[error("{at:?} lies outside the {width}x{height} screenshot")]
pub struct OutsideScreenshot {
    at: At,
    width: NonZeroU32,
    height: NonZeroU32,
}

impl View {
    pub(crate) fn new(rect: Rect) -> Self {
        let scale = frame::scale_for(rect.width, rect.height);
        Self {
            rect,
            width: frame::scaled(rect.width, scale),
            height: frame::scaled(rect.height, scale),
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "floored screenshot coordinates within the view map inside the display"
    )]
    pub(crate) fn point(self, at: At) -> Result<Point, OutsideScreenshot> {
        let (width, height) = (f64::from(self.width.get()), f64::from(self.height.get()));
        if at.x() >= width || at.y() >= height {
            return Err(OutsideScreenshot {
                at,
                width: self.width,
                height: self.height,
            });
        }
        Ok(Point {
            x: self.rect.x + (at.x() * f64::from(self.rect.width) / width).floor() as i32,
            y: self.rect.y + (at.y() * f64::from(self.rect.height) / height).floor() as i32,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Halted {
    #[error("the run was stopped")]
    RunStopped,
    #[error("computer use was turned off")]
    ComputerUseOff,
    #[error("the tool call was withdrawn")]
    Withdrawn,
}

#[derive(Debug, Default)]
pub struct Halt(OnceLock<Halted>);

impl Halt {
    pub(crate) fn halt(&self, why: Halted) {
        self.0.get_or_init(|| why);
    }

    fn halted(&self) -> Option<Halted> {
        self.0.get().copied()
    }

    pub(crate) fn check(&self) -> Result<(), Halted> {
        self.halted().map_or(Ok(()), Err)
    }
}

#[derive(Debug)]
pub enum Screen {
    Changed,
    Unchanged,
    Unknown(CaptureError),
}

#[derive(Debug)]
pub struct Outcome {
    pub(crate) completed: usize,
    pub(crate) interrupted: bool,
    pub(crate) screen: Screen,
    pub(crate) settled: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error(transparent)]
    Halted(Halted),
    #[error("computer input paused")]
    Paused,
    #[error("interrupted by user input")]
    Interrupted,
    #[error("batch exceeded 60 seconds")]
    TooLong,
    #[error("display changed; take a new screenshot")]
    DisplayChanged,
    #[error("check the agent display")]
    Display(#[source] DisplayError),
    #[error("focus is outside the agent display; focus an agent window first")]
    FocusOutside,
    #[error("find the focused window")]
    Focus(#[source] WindowError),
    #[error("cursor is outside the agent display")]
    CursorOutside,
    #[error("read the cursor")]
    Cursor(#[source] CursorError),
    #[error("send input")]
    Input(#[source] InputError),
    #[error("watch the screen")]
    Capture(#[source] CaptureError),
    #[error("screen wait timed out")]
    WaitTimedOut,
    #[error("map a point to the display")]
    Point(#[source] OutsideScreenshot),
    #[error("release held input")]
    Release(#[source] InputError),
    #[error("put the cursor back")]
    RestoreCursor(#[source] InputError),
    #[error("give focus back to window {window}")]
    RestoreFocus {
        window: crate::desktop::WindowId,
        #[source]
        source: WindowError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("step {step} reaches outside the screenshot; no input was sent")]
    Outside {
        step: usize,
        #[source]
        source: OutsideScreenshot,
    },
    #[error("user is active; no input was sent")]
    UserActive,
    #[error("capture the screen before input; no input was sent")]
    Before(#[source] CaptureError),
    #[error("read the cursor before input; no input was sent")]
    Cursor(#[source] CursorError),
    #[error("batch stopped after {completed} completed steps; do not blindly retry")]
    Stopped {
        completed: usize,
        #[source]
        source: StepError,
    },
}

struct Batch<'a> {
    desktop: &'a dyn Desktop,
    view: View,
    generation: u64,
    keys: Vec<Key>,
    buttons: Vec<Button>,
    began: Instant,
    paused: &'a AtomicBool,
    halt: &'a Halt,
}
impl Drop for Batch<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.release() {
            tracing::error!("releasing computer input: {}", diagnostics::chain(&error));
        }
    }
}
impl Batch<'_> {
    fn release(&mut self) -> Result<(), StepError> {
        let mut failure = None;
        while let Some(key) = self.keys.pop() {
            if let Err(error) = self.desktop.send(&Input::Key { key, down: false }) {
                failure.get_or_insert(error);
            }
        }
        while let Some(button) = self.buttons.pop() {
            if let Err(error) = self.desktop.send(&Input::Button {
                button,
                down: false,
            }) {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), |error| Err(StepError::Release(error)))
    }
    fn check(&self) -> Result<(), StepError> {
        self.halt.check().map_err(StepError::Halted)?;
        if self.paused.load(Ordering::SeqCst) {
            return Err(StepError::Paused);
        }
        if self.desktop.user_activity() != self.generation {
            return Err(StepError::Interrupted);
        }
        if self.began.elapsed() > Duration::from_secs(60) {
            return Err(StepError::TooLong);
        }
        if self.desktop.agent_display().map_err(StepError::Display)? != self.view.rect {
            return Err(StepError::DisplayChanged);
        }
        Ok(())
    }
    fn event(&mut self, input: Input) -> Result<(), StepError> {
        self.check()?;
        if matches!(
            input,
            Input::Key { down: true, .. } | Input::Text(_) | Input::Wheel { .. }
        ) {
            let focus = self.desktop.foreground();
            let focus_inside = match focus.map(|id| self.desktop.window_rect(id)) {
                Some(Ok(rect)) => self.view.rect.contains(rect.center()),
                Some(Err(WindowError::Gone { .. })) | None => false,
                Some(Err(error)) => return Err(StepError::Focus(error)),
            };
            if !focus_inside {
                return Err(StepError::FocusOutside);
            }
        }
        if matches!(input, Input::Button { down: true, .. })
            && !self
                .view
                .rect
                .contains(self.desktop.cursor().map_err(StepError::Cursor)?)
        {
            return Err(StepError::CursorOutside);
        }
        self.desktop.send(&input).map_err(StepError::Input)?;
        match input {
            Input::Key { key, down } => {
                self.keys.retain(|k| *k != key);
                if down {
                    self.keys.push(key);
                }
            }
            Input::Button { button, down } => {
                self.buttons.retain(|b| *b != button);
                if down {
                    self.buttons.push(button);
                }
            }
            Input::MoveTo(_) | Input::Wheel { .. } | Input::Text(_) => {}
        }
        Ok(())
    }
    fn move_to(&mut self, at: At) -> Result<(), StepError> {
        self.event(Input::MoveTo(
            self.view.point(at).map_err(StepError::Point)?,
        ))
    }
    fn button(&mut self, button: Button, down: bool) -> Result<(), StepError> {
        self.event(Input::Button { button, down })
    }
    fn delay(&self, duration: Duration) -> Result<(), StepError> {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.check()?;
            std::thread::sleep(
                until
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(10)),
            );
        }
        self.check()
    }
    fn capture(&self) -> Result<frame::Frame, StepError> {
        self.desktop
            .capture(self.view.rect)
            .map_err(StepError::Capture)
    }
    fn wait(&self, wait: Wait) -> Result<bool, StepError> {
        let (timeout, settled) = match wait {
            Wait::For(d) => return self.delay(d).map(|()| true),
            Wait::Changed(d) => (d, false),
            Wait::Settled(d) => (d, true),
        };
        let start = Instant::now();
        let mut previous = self.capture()?;
        let mut stable = Instant::now();
        while start.elapsed() < timeout {
            self.delay(Duration::from_millis(50))?;
            let current = self.capture()?;
            if current != previous {
                if !settled {
                    return Ok(true);
                }
                stable = Instant::now();
            }
            if settled && stable.elapsed() >= Duration::from_millis(200) {
                return Ok(true);
            }
            previous = current;
        }
        Ok(false)
    }
    fn step(&mut self, step: &Step) -> Result<(), StepError> {
        match step {
            Step::Move(at) => self.move_to(*at)?,
            Step::Down { at, button } | Step::Up { at, button } => {
                if let Some(at) = at {
                    self.move_to(*at)?;
                }
                self.button(*button, matches!(step, Step::Down { .. }))?;
            }
            Step::Click { at, button, count } => {
                self.move_to(*at)?;
                for _ in 0..*count {
                    self.button(*button, true)?;
                    self.delay(Duration::from_millis(20))?;
                    self.button(*button, false)?;
                    self.delay(Duration::from_millis(40))?;
                }
            }
            Step::Drag {
                from,
                through,
                to,
                button,
            } => {
                self.move_to(*from)?;
                self.button(*button, true)?;
                for at in through.iter().chain([to]) {
                    self.move_to(*at)?;
                    self.delay(Duration::from_millis(16))?;
                }
                self.button(*button, false)?;
            }
            Step::Scroll { at, dx, dy } => {
                self.move_to(*at)?;
                self.event(Input::Wheel { dx: *dx, dy: *dy })?;
            }
            Step::Key(keys) => {
                for key in keys {
                    self.event(Input::Key {
                        key: *key,
                        down: true,
                    })?;
                }
                for key in keys.iter().rev() {
                    self.event(Input::Key {
                        key: *key,
                        down: false,
                    })?;
                }
            }
            Step::KeyDown(key) | Step::KeyUp(key) => self.event(Input::Key {
                key: *key,
                down: matches!(step, Step::KeyDown(_)),
            })?,
            Step::Type(text) => {
                for c in text.chars() {
                    self.event(Input::Text(c))?;
                }
            }
            Step::Wait(wait) => {
                if !self.wait(*wait)? {
                    return Err(StepError::WaitTimedOut);
                }
            }
        }
        Ok(())
    }
}

fn points(step: &Step) -> Vec<At> {
    match step {
        Step::Move(at) | Step::Click { at, .. } | Step::Scroll { at, .. } => vec![*at],
        Step::Down { at, .. } | Step::Up { at, .. } => at.iter().copied().collect(),
        Step::Drag {
            from, through, to, ..
        } => std::iter::once(*from)
            .chain(through.iter().copied())
            .chain([*to])
            .collect(),
        Step::Key(_) | Step::KeyDown(_) | Step::KeyUp(_) | Step::Type(_) | Step::Wait(_) => {
            Vec::new()
        }
    }
}

pub(crate) fn run(
    desktop: &dyn Desktop,
    view: View,
    steps: &[Step],
    paused: &AtomicBool,
    halt: &Halt,
) -> Result<Outcome, RunError> {
    for (index, step) in steps.iter().enumerate() {
        for point in points(step) {
            view.point(point).map_err(|source| RunError::Outside {
                step: index + 1,
                source,
            })?;
        }
    }
    if desktop.user_idle() < Duration::from_millis(500) {
        return Err(RunError::UserActive);
    }
    let generation = desktop.user_activity();
    let before = desktop.capture(view.rect).map_err(RunError::Before)?;
    let cursor = desktop.cursor().map_err(RunError::Cursor)?;
    let focus = desktop.foreground();
    let mut batch = Batch {
        desktop,
        view,
        generation,
        keys: Vec::new(),
        buttons: Vec::new(),
        began: Instant::now(),
        paused,
        halt,
    };
    let mut completed = 0;
    let mut interrupted = false;
    let mut settled = false;
    let mut failure = None;
    for step in steps {
        if let Err(error) = batch.step(step) {
            interrupted = interruption(desktop, generation, paused, &error);
            if !interrupted {
                failure = Some(error);
            }
            break;
        }
        completed += 1;
    }
    if let Err(error) = batch.release() {
        failure.get_or_insert(error);
    }
    if failure.is_none() && !interrupted {
        match batch.wait(Wait::Settled(Duration::from_secs(1))) {
            Ok(calm) => settled = calm,
            Err(error) => {
                interrupted = interruption(desktop, generation, paused, &error);
                if !interrupted {
                    failure = Some(error);
                }
            }
        }
    }
    let screen = match desktop.capture(view.rect) {
        Ok(after) if after == before => Screen::Unchanged,
        Ok(_) => Screen::Changed,
        Err(error) => Screen::Unknown(error),
    };
    drop(batch);
    if desktop.user_activity() == generation && halt.halted().is_none() {
        if let Err(error) = desktop.send(&Input::MoveTo(cursor)) {
            failure.get_or_insert(StepError::RestoreCursor(error));
        }
        if let Some(window) = focus.filter(|focus| desktop.foreground() != Some(*focus))
            && let Err(source) = desktop.change_window(window, WindowChange::Focus)
        {
            failure.get_or_insert(StepError::RestoreFocus { window, source });
        }
    }
    match failure {
        Some(source) => Err(RunError::Stopped { completed, source }),
        None => Ok(Outcome {
            completed,
            interrupted,
            screen,
            settled,
        }),
    }
}

fn interruption(
    desktop: &dyn Desktop,
    generation: u64,
    paused: &AtomicBool,
    error: &StepError,
) -> bool {
    !matches!(error, StepError::Halted(_))
        && (desktop.user_activity() != generation || paused.load(Ordering::SeqCst))
}
