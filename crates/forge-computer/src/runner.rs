//! Serialized input batches. A batch always releases its held inputs.
use crate::{
    desktop::{Button, Desktop, Input, Point, Rect},
    frame,
    steps::{At, Key, Step, Wait},
};
use anyhow::{Result, bail};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub rect: Rect,
    pub width: u32,
    pub height: u32,
}
impl View {
    pub fn new(rect: Rect) -> Self {
        let scale = frame::scale_for(rect.width, rect.height);
        Self {
            rect,
            width: frame::scaled(rect.width, scale),
            height: frame::scaled(rect.height, scale),
        }
    }
    #[allow(clippy::cast_possible_truncation)] // Floored, bounded screenshot coordinates.
    pub fn point(self, at: At) -> Result<Point> {
        if at.x >= f64::from(self.width) || at.y >= f64::from(self.height) {
            bail!("point lies outside the screenshot");
        }
        Ok(Point {
            x: self.rect.x
                + (at.x * f64::from(self.rect.width) / f64::from(self.width)).floor() as i32,
            y: self.rect.y
                + (at.y * f64::from(self.rect.height) / f64::from(self.height)).floor() as i32,
        })
    }
}

pub struct Outcome {
    pub completed: usize,
    pub interrupted: bool,
    pub screen_changed: bool,
    pub settled: bool,
}

struct Batch<'a> {
    desktop: &'a dyn Desktop,
    view: View,
    generation: u64,
    keys: Vec<Key>,
    buttons: Vec<Button>,
    began: Instant,
    paused: &'a AtomicBool,
}
impl Drop for Batch<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.release() {
            tracing::error!("releasing computer input: {error:#}");
        }
    }
}
impl Batch<'_> {
    fn release(&mut self) -> Result<()> {
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
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    fn check(&self) -> Result<()> {
        if self.paused.load(Ordering::SeqCst) {
            bail!("computer input paused");
        }
        if self.desktop.user_activity() != self.generation {
            bail!("interrupted by user input");
        }
        if self.began.elapsed() > Duration::from_secs(60) {
            bail!("batch exceeded 60 seconds");
        }
        if self.desktop.agent_display()? != self.view.rect {
            bail!("display changed; take a new screenshot");
        }
        Ok(())
    }
    fn event(&mut self, input: Input) -> Result<()> {
        self.check()?;
        // Keyboard and wheel input must stay with a window on the agent's display.
        if matches!(
            input,
            Input::Key { down: true, .. } | Input::Text(_) | Input::Wheel { .. }
        ) {
            let focus = self.desktop.foreground();
            if !focus
                .and_then(|id| self.desktop.window_rect(id).ok())
                .is_some_and(|rect| self.view.rect.contains(rect.center()))
            {
                bail!("focus is outside the agent display; focus an agent window first");
            }
        }
        if matches!(input, Input::Button { down: true, .. })
            && !self.view.rect.contains(self.desktop.cursor()?)
        {
            bail!("cursor is outside the agent display");
        }
        self.desktop.send(&input)?;
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
            _ => {}
        }
        Ok(())
    }
    fn move_to(&mut self, at: At) -> Result<()> {
        self.event(Input::MoveTo(self.view.point(at)?))
    }
    fn button(&mut self, button: Button, down: bool) -> Result<()> {
        self.event(Input::Button { button, down })
    }
    fn delay(&self, duration: Duration) -> Result<()> {
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
    fn wait(&self, wait: Wait) -> Result<bool> {
        let (timeout, settled) = match wait {
            Wait::For(d) => return self.delay(d).map(|()| true),
            Wait::Changed(d) => (d, false),
            Wait::Settled(d) => (d, true),
        };
        let start = Instant::now();
        let mut previous = self.desktop.capture(self.view.rect)?;
        let mut stable = Instant::now();
        while start.elapsed() < timeout {
            self.delay(Duration::from_millis(50))?;
            let current = self.desktop.capture(self.view.rect)?;
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
    fn step(&mut self, step: &Step) -> Result<()> {
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
            Step::Drag { path, button } => {
                self.move_to(path[0])?;
                self.button(*button, true)?;
                for at in &path[1..] {
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
                    bail!("screen wait timed out");
                }
            }
        }
        Ok(())
    }
}

pub fn run(
    desktop: &dyn Desktop,
    view: View,
    steps: &[Step],
    paused: &AtomicBool,
) -> Result<Outcome> {
    // Validate every coordinate before sending any event.
    for step in steps {
        let points: Vec<At> = match step {
            Step::Move(at) | Step::Click { at, .. } | Step::Scroll { at, .. } => vec![*at],
            Step::Down { at, .. } | Step::Up { at, .. } => at.iter().copied().collect(),
            Step::Drag { path, .. } => path.clone(),
            _ => Vec::new(),
        };
        for point in points {
            view.point(point)?;
        }
    }
    if desktop.user_idle() < Duration::from_millis(500) {
        bail!("user is active; no input was sent");
    }
    let generation = desktop.user_activity();
    let before = desktop.capture(view.rect)?;
    let cursor = desktop.cursor()?;
    let focus = desktop.foreground();
    let mut batch = Batch {
        desktop,
        view,
        generation,
        keys: Vec::new(),
        buttons: Vec::new(),
        began: Instant::now(),
        paused,
    };
    let mut outcome = Outcome {
        completed: 0,
        interrupted: false,
        screen_changed: false,
        settled: false,
    };
    let mut failure = None;
    for step in steps {
        if let Err(error) = batch.step(step) {
            outcome.interrupted =
                desktop.user_activity() != generation || paused.load(Ordering::SeqCst);
            if !outcome.interrupted {
                failure = Some(error);
            }
            break;
        }
        outcome.completed += 1;
    }
    if let Err(error) = batch.release() {
        failure.get_or_insert(error);
    }
    if failure.is_none() && !outcome.interrupted {
        match batch.wait(Wait::Settled(Duration::from_secs(1))) {
            Ok(settled) => outcome.settled = settled,
            Err(error) => {
                outcome.interrupted =
                    desktop.user_activity() != generation || paused.load(Ordering::SeqCst);
                if !outcome.interrupted {
                    failure = Some(error);
                }
            }
        }
    }
    if let Ok(after) = desktop.capture(view.rect) {
        outcome.screen_changed = before != after;
    }
    drop(batch);
    // Restoring focus after physical input would steal it back from the user.
    if desktop.user_activity() == generation {
        if let Err(error) = desktop.send(&Input::MoveTo(cursor)) {
            failure.get_or_insert(error);
        }
        if let Some(focus) = focus.filter(|focus| desktop.foreground() != Some(*focus))
            && let Err(error) = desktop.change_window(focus, crate::desktop::WindowChange::Focus)
        {
            failure.get_or_insert(error);
        }
    }
    if let Some(error) = failure {
        bail!(
            "batch stopped after {} completed steps: {error:#}; do not blindly retry",
            outcome.completed
        );
    }
    Ok(outcome)
}
