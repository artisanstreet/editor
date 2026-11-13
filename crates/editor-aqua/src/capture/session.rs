use super::{
    record::{CaptureRecord, Context as RecordContext},
    scenario::{Route, Scenario},
    script::{Button, Keystroke, Step, parse, types_text},
    surface::Surface,
};
use crate::shell::{application::ShellApplication, host::ShellResources, root::RootViews};
use anyhow::{Context, Result, bail, ensure};
use aqua_protocol::{LogicalSize, WindowEvent};
use aqua::prelude::{AquaWindowId, ObservedWindowState};
use aqua::{Application, Aqua, AquaConfig};
use aqua_core::{
    ColorProfile, InputEvent, Key, KeyEvent, Modifiers, Point, PointerButton, PointerEvent,
    PointerKind, ScrollDelta,
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const STABLE_FRAMES: usize = 5;
const STABLE_FOR: Duration = Duration::from_millis(400);
const PUMP_INTERVAL: Duration = Duration::from_millis(8);

#[derive(Clone, Debug)]
pub struct Options {
    pub fixture: PathBuf,
    pub route: Route,
    pub route_name: String,
    pub logical: (f64, f64),
    pub scale: f64,
    pub focused: bool,
    pub out_dir: PathBuf,
    pub latency: Option<std::time::Duration>,
}

pub struct Session {
    aqua: Aqua,
    app: ShellApplication<RootViews>,
    id: AquaWindowId,
    observed: ObservedWindowState,
    surface: Surface,
    tap: std::rc::Rc<std::cell::RefCell<Vec<aqua_protocol::RenderCommand>>>,
    render: aqua_runtime::RenderSide,
    scenario: Scenario,
    options: Options,
    pointer: Point,
    sequence: u64,
    records: Vec<CaptureRecord>,
}

fn pointer(at: Point, buttons: u8) -> PointerEvent {
    PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: at,
        buttons,
    }
}

impl Session {
    pub fn start(options: Options) -> Result<Self> {
        ensure!(
            options.logical.0 >= 480.0 && options.logical.1 >= 420.0,
            "capture viewport is below the Editor minimum"
        );
        ensure!(
            options.scale.is_finite() && options.scale > 0.0,
            "capture scale must be positive"
        );
        let mut config = AquaConfig {
            headless: true,
            ..AquaConfig::default()
        };
        config.window.title = "Artisan Editor".to_owned();
        let mut aqua = Aqua::new(config).map_err(anyhow::Error::msg)?;
        let (scenario, mut core) = Scenario::start(&options.fixture, options.latency)?;
        core.navigate(scenario.screen(&options.route)?);
        let resources = ShellResources::for_runtime(&aqua, options.scale as f32)
            .context("could not configure Editor layout")?
            .with_color_profile(ColorProfile::GpuiWindowsEncoded);
        let surface = Surface::new(
            options.logical.0,
            options.logical.1,
            options.scale,
            aqua.fonts().clone(),
            aqua.runs().clone(),
        )?;
        let mut app = ShellApplication::for_qualification(core, resources, RootViews::default());
        let tap = app.tap_render();
        let id = AquaWindowId::new(1).context("window id")?;
        let mut size = aqua::prelude::WindowOptions::default().inner_size;
        size.width = options.logical.0;
        size.height = options.logical.1;
        let mut observed = ObservedWindowState::new(id, "Artisan Editor".to_owned(), size);
        observed.scale_factor = options.scale;
        observed.focused = options.focused;
        aqua.add_headless_window(observed.clone());
        aqua.dispatch_start(&mut app, id);
        aqua.dispatch_window_event(&mut app, id, &WindowEvent::ScaleChanged(id, options.scale));
        aqua.dispatch_window_event(
            &mut app,
            id,
            &WindowEvent::Resized(id, LogicalSize::new(options.logical.0, options.logical.1)),
        );
        if options.focused {
            aqua.dispatch_input(&mut app, id, &InputEvent::Focused(true));
        }
        Ok(Self {
            aqua,
            app,
            id,
            observed,
            surface,
            tap,
            render: aqua_runtime::RenderSide::new(),
            scenario,
            options,
            pointer: Point::new(0.0, 0.0),
            sequence: 0,
            records: Vec::new(),
        })
    }

    fn frame(&mut self) -> Result<()> {
        self.aqua.dispatch_wake(&mut self.app, self.id);
        let now = self.aqua.clock();
        self.aqua.draw_ready(&mut self.app, now);
        self.surface
            .register_externals(&mut self.app, &self.observed, now);
        self.app.build_scene(&self.observed, now);
        self.aqua.flush_output(&mut self.app, self.id);
        let commands = std::mem::take(&mut *self.tap.borrow_mut());
        self.render.step(aqua_protocol::RenderInput {
            commands,
            feedback: Vec::new(),
            present_at: now,
        });
        self.surface.draw(self.render.scene(), now)?;
        self.sequence += 1;
        self.aqua
            .dispatch_frame_completed(&mut self.app, self.id, self.sequence);
        Ok(())
    }

    fn dispatch(&mut self, events: &[InputEvent]) -> Result<()> {
        for event in events {
            self.aqua.dispatch_input(&mut self.app, self.id, event);
        }
        self.frame()
    }

    pub fn settle(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut previous: Option<Vec<u8>> = None;
        let mut identical = 0;
        let mut stable_since = Instant::now();
        loop {
            self.frame()?;
            let ready = self
                .app
                .shell
                .core
                .host(&self.scenario.host)
                .is_some_and(|host| host.is_live())
                && !self.app.shell.core.test_startup_pending();
            let pixels = self.surface.read()?.rgba;
            if ready && previous.as_ref() == Some(&pixels) {
                identical += 1;
            } else {
                identical = 0;
                stable_since = Instant::now();
            }
            previous = Some(pixels);
            if identical >= STABLE_FRAMES && stable_since.elapsed() >= STABLE_FOR {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "the window did not settle within {timeout:?}"
            );
            std::thread::sleep(PUMP_INTERVAL);
        }
    }

    fn wait(&mut self, duration: Duration) -> Result<()> {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.frame()?;
            std::thread::sleep(PUMP_INTERVAL.min(until.saturating_duration_since(Instant::now())));
        }
        self.frame()
    }

    fn click(&mut self, at: Point, button: Button) -> Result<()> {
        let (core_button, mask) = match button {
            Button::Left => (PointerButton::Primary, 1),
            Button::Right => (PointerButton::Secondary, 2),
        };
        self.pointer = at;
        self.dispatch(&[
            InputEvent::PointerMove(pointer(at, 0)),
            InputEvent::PointerDown(pointer(at, mask), core_button),
            InputEvent::PointerUp(pointer(at, 0), core_button),
        ])
    }

    fn key(&mut self, keystroke: &Keystroke) -> Result<()> {
        let event = KeyEvent {
            key: keystroke.key,
            modifiers: keystroke.modifiers,
            repeat: false,
        };
        let mut events = vec![InputEvent::KeyDown(event)];
        if let Some(character) = types_text(keystroke) {
            events.push(InputEvent::TextInput {
                text: character.to_string(),
            });
        }
        events.push(InputEvent::KeyUp(event));
        self.dispatch(&events)
    }

    fn type_text(&mut self, text: &str) -> Result<()> {
        for character in text.chars() {
            self.key(&Keystroke {
                key: Key::Char(character),
                modifiers: Modifiers::NONE,
            })?;
        }
        Ok(())
    }

    fn capture(&mut self, path: &Path) -> Result<()> {
        self.frame()?;
        let first = self.surface.read()?;
        self.frame()?;
        let second = self.surface.read()?;
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .context("capture path has no file name")?
            .to_owned();
        let record = CaptureRecord::describe(
            &RecordContext {
                name: &name,
                route: &self.options.route_name,
                logical: self.options.logical,
                surface: &self.surface,
                repeat_identical: first.rgba == second.rgba,
            },
            &first,
        );
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.options.out_dir.join(path)
        };
        first.save(&path)?;
        std::fs::write(
            path.with_extension("json"),
            serde_json::to_vec_pretty(&record)?,
        )
        .context("write capture record")?;
        tracing::info!(path = %path.display(), identical = record.repeat_identical, "captured the window");
        self.records.push(record);
        Ok(())
    }

    pub fn perform(&mut self, step: &Step) -> Result<bool> {
        match step {
            Step::Wait(duration) => self.wait(*duration)?,
            Step::Move(at) => {
                self.pointer = *at;
                self.dispatch(&[InputEvent::PointerMove(pointer(*at, 0))])?;
            }
            Step::Click(at, button) => self.click(*at, *button)?,
            Step::Scroll(at, pixels) => {
                self.pointer = *at;
                self.dispatch(&[
                    InputEvent::PointerMove(pointer(*at, 0)),
                    InputEvent::Scroll {
                        at: *at,
                        delta: ScrollDelta {
                            x: 0.0,
                            y: -*pixels,
                        },
                    },
                ])?;
            }
            Step::Type(text) => self.type_text(text)?,
            Step::Key(keystroke) => self.key(keystroke)?,
            Step::Settle(timeout) => self.settle(*timeout)?,
            Step::Capture(path) => self.capture(path)?,
            Step::Quit => return Ok(false),
        }
        Ok(true)
    }

    pub fn run(&mut self, steps: &[Step]) -> Result<()> {
        for (index, step) in steps.iter().enumerate() {
            tracing::info!(step = index + 1, ?step, "script step");
            if !self
                .perform(step)
                .with_context(|| format!("script step {} failed", index + 1))?
            {
                break;
            }
        }
        let unstable: Vec<_> = self
            .records
            .iter()
            .filter(|record| !record.repeat_identical)
            .map(|record| record.name.as_str())
            .collect();
        if !unstable.is_empty() {
            bail!("repeat captures differ: {}", unstable.join(", "));
        }
        Ok(())
    }

    pub fn records(&self) -> &[CaptureRecord] {
        &self.records
    }

    pub fn finish(mut self) {
        self.scenario.shutdown();
    }
}

pub fn run_script(options: Options, script: &str) -> Result<Vec<CaptureRecord>> {
    let steps = parse(script)?;
    let mut session = Session::start(options)?;
    let result = session.run(&steps);
    let records = session.records().to_vec();
    session.finish();
    result.map(|()| records)
}
