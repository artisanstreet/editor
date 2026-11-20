use crate::{
    desktop::*,
    frame::{self, Frame},
    runner::{self, View},
    steps::{self, Key, Named, Step},
};
use base64::Engine as _;
use serde_json::json;
use std::num::NonZeroU32;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[path = "stop.rs"]
mod stop_tests;

#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent fault a test switches on"
)]
struct Fake {
    events: Mutex<Vec<Input>>,
    sent: tokio::sync::watch::Sender<usize>,
    cursor: Mutex<Point>,
    activity: AtomicU64,
    interrupt: bool,
    fail: bool,
    focused: bool,
    idle: bool,
    captures: AtomicU64,
    capture_limit: u64,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            events: Mutex::default(),
            sent: tokio::sync::watch::Sender::new(0),
            cursor: Mutex::new(Point { x: 110, y: 210 }),
            activity: AtomicU64::new(0),
            interrupt: false,
            fail: false,
            focused: true,
            idle: true,
            captures: AtomicU64::new(0),
            capture_limit: u64::MAX,
        }
    }
}
fn area() -> Rect {
    Rect {
        x: 100,
        y: 200,
        width: 800,
        height: 600,
    }
}
impl Desktop for Fake {
    fn agent_display(&self) -> Result<Rect, DisplayError> {
        Ok(area())
    }
    fn user_display(&self) -> Result<Rect, DisplayError> {
        Ok(area())
    }
    fn capture(&self, area: Rect) -> Result<Frame, CaptureError> {
        if self.captures.fetch_add(1, Ordering::SeqCst) >= self.capture_limit {
            return Err(CaptureError::Failed {
                area,
                step: "BitBlt",
            });
        }
        Ok(Frame::new(
            area.width,
            area.height,
            vec![[0; 3]; area.width as usize * area.height as usize],
        )
        .unwrap())
    }
    fn cursor(&self) -> Result<Point, CursorError> {
        Ok(*self.cursor.lock().unwrap())
    }
    fn send(&self, input: &Input) -> Result<(), InputError> {
        self.events.lock().unwrap().push(*input);
        self.sent.send_modify(|sent| *sent += 1);
        if let Input::MoveTo(p) = input {
            *self.cursor.lock().unwrap() = *p;
        }
        if matches!(
            input,
            Input::Key { down: true, .. } | Input::Button { down: true, .. }
        ) && self.interrupt
        {
            self.activity.fetch_add(1, Ordering::SeqCst);
        }
        if matches!(input, Input::Text(_)) && self.fail {
            return Err(InputError::Rejected { input: *input });
        }
        Ok(())
    }
    fn user_idle(&self) -> Duration {
        if self.idle {
            Duration::from_secs(1)
        } else {
            Duration::ZERO
        }
    }
    fn user_activity(&self) -> u64 {
        self.activity.load(Ordering::SeqCst)
    }
    fn foreground(&self) -> Option<WindowId> {
        Some(if self.focused { 1 } else { 2 })
    }
    fn windows(&self) -> Result<Vec<WindowInfo>, WindowError> {
        Ok(vec![WindowInfo {
            id: 1,
            title: "test".into(),
            process: "test".into(),
            pid: 1,
            rect: area(),
            minimized: false,
        }])
    }
    fn change_window(&self, _: WindowId, _: WindowChange) -> Result<(), WindowError> {
        Ok(())
    }
    fn launch(&self, _: &str, _: &[String], _: Option<&str>) -> Result<u32, LaunchError> {
        Ok(1)
    }
    fn processes(&self) -> Result<Vec<(u32, u32)>, ProcessesError> {
        Ok(vec![])
    }
    fn clipboard(&self) -> Result<Option<String>, ClipboardError> {
        Ok(None)
    }
    fn set_clipboard(&self, _: &str) -> Result<(), ClipboardError> {
        Ok(())
    }
}
#[test]
fn rejects_malformed_or_oversized_batches_before_execution() {
    for value in [
        json!([]),
        json!([{"click":[0,0],"type":"oops"}]),
        json!([{"click":[-1,0]}]),
        json!([{"key":"ctrl+unknown"}]),
        json!([{"wait":{"ms":30001}}]),
        json!([{"wait":{"ms":"bad"}}]),
        json!([{"click":[0,0],"button":12}]),
        json!([{"type":"x","typo":true}]),
        json!([{"drag":[[0,0]]}]),
    ] {
        assert!(steps::parse(&value).is_err(), "accepted {value}");
    }
    steps::parse(&json!(vec![json!({"type":"x"}); 257])).unwrap_err();
    assert_eq!(steps::key("Return").unwrap(), Key::Named(Named::Enter));
    assert_eq!(steps::key("F24").unwrap(), Key::Named(Named::F(24)));
}
#[test]
fn bad_later_coordinate_does_not_execute_earlier_steps() {
    let desktop = Fake::default();
    let steps = steps::parse(&json!([{"type":"must not type"},{"click":[800,1]}])).unwrap();
    run(&desktop, View::new(area()), &steps).unwrap_err();
    assert!(desktop.events.lock().unwrap().is_empty());
}
#[test]
fn interrupted_chord_releases_keys_and_does_not_restore_cursor() {
    let desktop = Fake {
        interrupt: true,
        ..Default::default()
    };
    let steps = steps::parse(&json!([{"key":"ctrl+s"},{"type":"must not type"}])).unwrap();
    let outcome = run(&desktop, View::new(area()), &steps).unwrap();
    assert!(outcome.interrupted);
    assert_eq!(outcome.completed, 0);
    assert_eq!(
        *desktop.events.lock().unwrap(),
        vec![
            Input::Key {
                key: Key::Named(Named::Ctrl),
                down: true
            },
            Input::Key {
                key: Key::Named(Named::Ctrl),
                down: false
            }
        ]
    );
}
#[test]
fn backend_failure_releases_held_key() {
    let desktop = Fake {
        fail: true,
        ..Default::default()
    };
    let steps = steps::parse(&json!([{"key_down":"shift"},{"type":"x"}])).unwrap();
    let error = run(&desktop, View::new(area()), &steps).err().unwrap();
    assert!(
        matches!(
            error,
            runner::RunError::Stopped {
                completed: 1,
                source: runner::StepError::Input(InputError::Rejected { .. }),
            }
        ),
        "{error:?}"
    );
    assert!(desktop.events.lock().unwrap().contains(&Input::Key {
        key: Key::Named(Named::Shift),
        down: false
    }));
}
#[test]
fn active_user_or_foreign_focus_prevents_typing() {
    for desktop in [
        Fake {
            idle: false,
            ..Default::default()
        },
        Fake {
            focused: false,
            ..Default::default()
        },
    ] {
        run(&desktop, View::new(area()), &[Step::Type("x".into())]).unwrap_err();
        assert!(
            !desktop
                .events
                .lock()
                .unwrap()
                .iter()
                .any(|i| matches!(i, Input::Text(_)))
        );
    }
}
#[test]
fn scales_coordinates_including_negative_monitor_origins() {
    let rect = Rect {
        x: -3840,
        y: -2160,
        width: 3840,
        height: 2160,
    };
    let view = View::new(rect);
    let at = |x: u32, y: u32| steps::At::new(f64::from(x), f64::from(y)).unwrap();
    assert_eq!(view.point(at(0, 0)).unwrap(), Point { x: -3840, y: -2160 });
    let point = view
        .point(at(view.width.get() - 1, view.height.get() - 1))
        .unwrap();
    assert!(rect.contains(point));
    view.point(at(view.width.get(), 0)).unwrap_err();
}
#[test]
fn points_off_the_screenshot_cannot_be_built() {
    for (x, y) in [
        (-1.0, 0.0),
        (0.0, -0.5),
        (f64::NAN, 0.0),
        (0.0, f64::INFINITY),
        (f64::NEG_INFINITY, 1.0),
    ] {
        assert!(steps::At::new(x, y).is_none(), "built ({x}, {y})");
    }
    let steps = steps::parse(&json!([{"drag":[[1,2],[3,4]]}])).unwrap();
    let [
        Step::Drag {
            from, through, to, ..
        },
    ] = steps.as_slice()
    else {
        panic!("not one drag: {steps:?}");
    };
    assert_eq!((from.x(), from.y(), to.x(), to.y()), (1.0, 2.0, 3.0, 4.0));
    assert!(through.is_empty());
}
#[test]
fn image_crop_downsampling_and_png_are_consistent() {
    let image = Frame::new(
        2,
        2,
        vec![[0, 0, 0], [100, 100, 100], [200, 200, 200], [100, 100, 100]],
    )
    .unwrap();
    let one = NonZeroU32::MIN;
    assert_eq!(image.shrink(one, one).pixels(), [[100, 100, 100]]);
    assert_eq!(
        image.crop(1, 1, 100, 100).unwrap().pixels(),
        [[100, 100, 100]]
    );
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(image.png_base64().unwrap())
        .unwrap();
    let reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    assert_eq!((reader.info().width, reader.info().height), (2, 2));
    assert!((frame::scale_for(800, 600) - 1.0).abs() < f64::EPSILON);
}

#[test]
fn frames_keep_their_size_and_pixels_together() {
    let image = Frame::new(2, 2, vec![[1, 2, 3]; 4]).unwrap();
    assert_eq!(image.crop(2, 0, 1, 2), None);
    assert_eq!(image.crop(0, 2, 2, 1), None);
    assert_eq!(image.crop(0, 0, 0, 2), None);
    Frame::new(2, 2, vec![[0; 3]; 3]).unwrap_err();
    Frame::new(0, 2, Vec::new()).unwrap_err();
}

#[test]
fn a_failed_capture_after_input_reports_the_screen_as_unknown() {
    let desktop = Fake {
        interrupt: true,
        capture_limit: 1,
        ..Default::default()
    };
    let steps = steps::parse(&json!([{"key":"ctrl+s"}])).unwrap();
    let outcome = run(&desktop, View::new(area()), &steps).unwrap();
    assert!(outcome.interrupted);
    assert!(
        matches!(
            outcome.screen,
            runner::Screen::Unknown(CaptureError::Failed { .. })
        ),
        "{:?}",
        outcome.screen
    );
}

fn run(
    desktop: &dyn Desktop,
    view: View,
    steps: &[Step],
) -> Result<runner::Outcome, runner::RunError> {
    runner::run(
        desktop,
        view,
        steps,
        &std::sync::atomic::AtomicBool::new(false),
        &runner::Halt::default(),
    )
}

struct Parked(tokio::sync::mpsc::UnboundedSender<()>);

impl forge_engines::Harness for Parked {
    fn run(
        &self,
        turn: forge_engines::Turn,
        io: forge_engines::RunIo,
    ) -> std::pin::Pin<Box<dyn Future<Output = forge_engines::Outcome> + Send>> {
        let started = self.0.clone();
        Box::pin(async move {
            io.emit(forge_engines::EngineEvent::InputAccepted {
                id: turn.input.id.clone(),
            })
            .await;
            started.send(()).unwrap();
            io.cancel.cancelled().await;
            forge_engines::Outcome::Cancelled
        })
    }
}

struct Live {
    _shutdown: tokio_util::sync::DropGuard,
    hub: forge_runs::Hub,
    store: forge_store::Store,
    set: super::ComputerMcp,
    desktop: Arc<Fake>,
    thread: artisan_api::ThreadId,
    _dir: tempfile::TempDir,
}

async fn live(desktop: Fake) -> Live {
    let dir = tempfile::tempdir().unwrap();
    let engine = artisan_api::EngineId::Claude;
    let engine_dir = dir.path().join("engines").join(engine.to_string());
    let entry = engine_dir
        .join("versions")
        .join("1.0.0")
        .join(forge_engines::host_entry(engine).unwrap());
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(entry, "").unwrap();
    std::fs::write(engine_dir.join("current"), "1.0.0").unwrap();
    let store = forge_store::Store::open(&dir.path().join("forge")).unwrap();
    let (started, mut running) = tokio::sync::mpsc::unbounded_channel();
    let harness: Arc<dyn forge_engines::Harness> = Arc::new(Parked(started));
    let hub = forge_runs::Hub::with_harness(
        store.clone(),
        forge_engines::Engines::new(dir.path()),
        dir.path().into(),
        Arc::new(move |_| Arc::clone(&harness)),
    );
    let project = store
        .create_project(artisan_api::CreateProject {
            id: artisan_api::ProjectId::new(),
            path: dir.path().display().to_string(),
            name: None,
        })
        .await
        .unwrap();
    let thread = store
        .create_thread(artisan_api::CreateThread {
            id: artisan_api::ThreadId::new(),
            project_id: project.id,
            config: artisan_api::EngineConfig::new(engine),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    hub.send_message(
        &thread.id,
        artisan_api::SendMessage {
            draft: None,
            id: artisan_api::ItemId::new(),
            text: "use the computer".to_owned(),
            images: Vec::new(),
            config: None,
        },
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), running.recv())
        .await
        .unwrap()
        .unwrap();
    let desktop = Arc::new(desktop);
    let set = super::ComputerMcp::with_desktop(hub.clone(), Arc::<Fake>::clone(&desktop));
    Live {
        _shutdown: hub.shutdown_on_drop(),
        hub,
        store,
        set,
        desktop,
        thread: thread.id,
        _dir: dir,
    }
}

#[tokio::test]
async fn protocol_returns_images_and_rechecks_setting_for_existing_credentials() {
    use forge_mcp::ToolSet;
    let Live {
        store,
        set,
        thread: caller,
        ..
    } = &live(Fake::default()).await;
    let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"screenshot"}});
    let denied = forge_mcp::server::handle(set, &gateway(caller), call.clone())
        .await
        .unwrap();
    assert_eq!(denied["result"]["isError"], true);
    store
        .set_setting("computer_use".into(), json!(true))
        .await
        .unwrap();
    let reply = forge_mcp::server::handle(set, &gateway(caller), call.clone())
        .await
        .unwrap();
    assert_eq!(reply["result"]["isError"], false);
    assert_eq!(reply["result"]["content"][1]["type"], "image");
    assert_eq!(reply["result"]["content"][1]["mimeType"], "image/png");
    assert_eq!(set.tools().len(), 8);
    let zoom = set
        .call(caller, "zoom", json!({"x":0,"y":0,"width":5,"height":5}))
        .await
        .unwrap();
    assert!(matches!(zoom[1], forge_mcp::Content::Image { .. }));
    store
        .set_setting("computer_use".into(), json!(false))
        .await
        .unwrap();
    let revoked = forge_mcp::server::handle(set, &gateway(caller), call)
        .await
        .unwrap();
    assert_eq!(revoked["result"]["isError"], true);
}

#[tokio::test]
async fn interruption_pauses_subsequent_batches() {
    use forge_mcp::ToolSet;
    let Live {
        store,
        set,
        thread: caller,
        ..
    } = &live(Fake {
        interrupt: true,
        ..Default::default()
    })
    .await;
    store
        .set_setting("computer_use".into(), json!(true))
        .await
        .unwrap();
    set.call(caller, "screenshot", json!({})).await.unwrap();
    let reply = set
        .call(caller, "input", json!({"steps":[{"key":"ctrl+s"}]}))
        .await
        .unwrap();
    let forge_mcp::Content::Text(result) = &reply[0] else {
        panic!("missing result")
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(result).unwrap()["interrupted"],
        true
    );
    let refused = set
        .call(caller, "input", json!({"steps":[{"type":"do not type"}]}))
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, super::ComputerError::Refused(refused) if refused.0.contains("paused")),
        "{refused:?}"
    );
}

#[test]
fn handoff_interrupts_an_in_progress_wait_and_releases_held_inputs() {
    let desktop = Fake::default();
    let paused = std::sync::atomic::AtomicBool::new(false);
    let steps =
        steps::parse(&json!([{"key_down":"shift"},{"wait":{"ms":30000}},{"type":"must not type"}]))
            .unwrap();
    std::thread::scope(|scope| {
        let task = scope.spawn(|| {
            runner::run(
                &desktop,
                View::new(area()),
                &steps,
                &paused,
                &runner::Halt::default(),
            )
            .unwrap()
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while desktop.events.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        paused.store(true, Ordering::SeqCst);
        let outcome = task.join().unwrap();
        assert!(outcome.interrupted);
    });
    let events = desktop.events.lock().unwrap();
    assert!(events.contains(&Input::Key {
        key: Key::Named(Named::Shift),
        down: false
    }));
    assert!(!events.iter().any(|input| matches!(input, Input::Text(_))));
}

#[test]
fn successful_batch_reports_settled_screen_and_restores_cursor() {
    let desktop = Fake::default();
    let outcome = run(
        &desktop,
        View::new(area()),
        &[Step::Move(steps::At::new(100., 100.).unwrap())],
    )
    .unwrap();
    assert_eq!(outcome.completed, 1);
    assert!(outcome.settled);
    assert!(matches!(outcome.screen, runner::Screen::Unchanged));
    assert_eq!(*desktop.cursor.lock().unwrap(), Point { x: 110, y: 210 });
}

fn gateway(thread: &artisan_api::ThreadId) -> forge_runs::GatewayCaller {
    forge_runs::GatewayCaller {
        thread: thread.clone(),
        ended: tokio_util::sync::CancellationToken::new(),
        run_stopped: None,
    }
}
