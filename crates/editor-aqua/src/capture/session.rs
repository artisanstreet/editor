use super::{
    record::{CaptureRecord, Context as RecordContext},
    scenario::{NoThread, Route, Scenario, ScenarioError},
    script::{Button, Keystroke, ScriptError, Step, parse, types_text},
    surface::{SaveError, Surface, SurfaceError},
};
use crate::shell::{application::ShellApplication, host::ShellResources, root::RootViews};
use aqua::prelude::{AquaWindowId, ObservedWindowState};
use aqua::{Application, Aqua, AquaConfig};
use aqua_core::{
    ColorProfile, InputEvent, Key, KeyEvent, Modifiers, Point, PointerButton, PointerEvent,
    PointerKind, ScrollDelta,
};
use aqua_protocol::{LogicalSize, WindowEvent};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const STABLE_FRAMES: usize = 5;
const STABLE_FOR: Duration = Duration::from_millis(400);
const PUMP_INTERVAL: Duration = Duration::from_millis(8);

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("a {width}x{height} capture is below the Editor's minimum size")]
    Viewport { width: f64, height: f64 },
    #[error("capture scale {scale} is not positive")]
    Scale { scale: f64 },
    #[error("start the window system: {message}")]
    Start { message: String },
    #[error("set up the capture scenario")]
    Scenario(#[source] ScenarioError),
    #[error("choose the screen to capture")]
    Screen(#[source] NoThread),
    #[error("configure the Editor layout at scale {scale}")]
    Layout { scale: f64 },
    #[error("render the window")]
    Surface(#[source] SurfaceError),
    #[error("the window did not settle within {timeout:?}")]
    Unsettled { timeout: Duration },
    #[error("the capture path {} has no file name", path.display())]
    NoFileName { path: PathBuf },
    #[error("save the capture")]
    Save(#[source] SaveError),
    #[error("encode the capture record {name}")]
    EncodeRecord {
        name: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("write the capture record {}", path.display())]
    WriteRecord {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("script step {step} failed")]
    Step {
        step: usize,
        #[source]
        source: Box<Self>,
    },
    #[error("repeat captures differ: {}", names.join(", "))]
    Unstable { names: Vec<String> },
    #[error("parse the capture script")]
    Script(#[source] ScriptError),
}

#[derive(Clone, Debug)]
pub struct Options {
    pub fixture: PathBuf,
    pub route: Route,
    pub route_name: String,
    pub logical: (f64, f64),
    pub scale: f64,
    pub focused: bool,
    pub out_dir: PathBuf,
    pub latency: Option<Duration>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
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
    pub fn start(options: Options) -> Result<Self, SessionError> {
        if options.logical.0 < 480.0 || options.logical.1 < 420.0 {
            return Err(SessionError::Viewport {
                width: options.logical.0,
                height: options.logical.1,
            });
        }
        if !(options.scale.is_finite() && options.scale > 0.0) {
            return Err(SessionError::Scale {
                scale: options.scale,
            });
        }
        let mut config = AquaConfig {
            headless: true,
            ..AquaConfig::default()
        };
        "Artisan Editor".clone_into(&mut config.window.title);
        let mut aqua = Aqua::new(config).map_err(|message| SessionError::Start { message })?;
        let (scenario, mut core) =
            Scenario::start(&options.fixture, options.latency).map_err(SessionError::Scenario)?;
        core.navigate(
            scenario
                .screen(&options.route)
                .map_err(SessionError::Screen)?,
        );
        let resources = ShellResources::for_runtime(&aqua, crate::measure::narrow(options.scale))
            .ok_or(SessionError::Layout {
                scale: options.scale,
            })?
            .with_color_profile(ColorProfile::GpuiWindowsEncoded);
        let surface = Surface::new(
            options.logical.0,
            options.logical.1,
            options.scale,
            aqua.fonts().clone(),
            aqua.runs().clone(),
        )
        .map_err(SessionError::Surface)?;
        let mut app = ShellApplication::for_qualification(core, resources, RootViews::default());
        let tap = app.tap_render();
        let id = AquaWindowId::new(1).expect("invariant: window id 1 is nonzero");
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

    fn frame(&mut self) -> Result<(), SessionError> {
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
        self.surface
            .draw(self.render.scene(), now)
            .map_err(SessionError::Surface)?;
        self.sequence += 1;
        self.aqua
            .dispatch_frame_completed(&mut self.app, self.id, self.sequence);
        Ok(())
    }

    fn dispatch(&mut self, events: &[InputEvent]) -> Result<(), SessionError> {
        for event in events {
            self.aqua.dispatch_input(&mut self.app, self.id, event);
        }
        self.frame()
    }

    pub fn settle(&mut self, timeout: Duration) -> Result<(), SessionError> {
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
                .is_some_and(editor_core::HostData::is_live)
                && !self.app.shell.core.test_startup_pending();
            let pixels = self.surface.read().map_err(SessionError::Surface)?.rgba;
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
            if Instant::now() >= deadline {
                return Err(SessionError::Unsettled { timeout });
            }
            std::thread::sleep(PUMP_INTERVAL);
        }
    }

    fn wait(&mut self, duration: Duration) -> Result<(), SessionError> {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.frame()?;
            std::thread::sleep(PUMP_INTERVAL.min(until.saturating_duration_since(Instant::now())));
        }
        self.frame()
    }

    fn click(&mut self, at: Point, button: Button) -> Result<(), SessionError> {
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

    fn key(&mut self, keystroke: &Keystroke) -> Result<(), SessionError> {
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

    fn type_text(&mut self, text: &str) -> Result<(), SessionError> {
        for character in text.chars() {
            self.key(&Keystroke {
                key: Key::Char(character),
                modifiers: Modifiers::NONE,
            })?;
        }
        Ok(())
    }

    fn capture(&mut self, path: &Path) -> Result<(), SessionError> {
        self.frame()?;
        let first = self.surface.read().map_err(SessionError::Surface)?;
        self.frame()?;
        let second = self.surface.read().map_err(SessionError::Surface)?;
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| SessionError::NoFileName {
                path: path.to_owned(),
            })?
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
        first.save(&path).map_err(SessionError::Save)?;
        let bytes =
            serde_json::to_vec_pretty(&record).map_err(|source| SessionError::EncodeRecord {
                name: name.clone(),
                source,
            })?;
        let record_path = path.with_extension("json");
        std::fs::write(&record_path, bytes).map_err(|source| SessionError::WriteRecord {
            path: record_path,
            source,
        })?;
        tracing::info!(path = %path.display(), identical = record.repeat_identical, "captured the window");
        self.records.push(record);
        Ok(())
    }

    pub fn perform(&mut self, step: &Step) -> Result<bool, SessionError> {
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

    pub fn run(&mut self, steps: &[Step]) -> Result<(), SessionError> {
        for (index, step) in steps.iter().enumerate() {
            tracing::info!(step = index + 1, ?step, "script step");
            if !self.perform(step).map_err(|source| SessionError::Step {
                step: index + 1,
                source: Box::new(source),
            })? {
                break;
            }
        }
        let names: Vec<_> = self
            .records
            .iter()
            .filter(|record| !record.repeat_identical)
            .map(|record| record.name.clone())
            .collect();
        if !names.is_empty() {
            return Err(SessionError::Unstable { names });
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

pub fn run_script(options: Options, script: &str) -> Result<Vec<CaptureRecord>, SessionError> {
    let steps = parse(script).map_err(SessionError::Script)?;
    let mut session = Session::start(options)?;
    let result = session.run(&steps);
    let records = session.records().to_vec();
    session.finish();
    result.map(|()| records)
}
