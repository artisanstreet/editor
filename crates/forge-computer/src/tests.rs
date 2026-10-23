use crate::{
    desktop::*,
    frame::{self, Frame},
    runner::{self, View},
    steps::{self, Key, Named, Step},
};
use anyhow::{Result, bail};
use base64::Engine as _;
use serde_json::json;
use std::{
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

#[allow(clippy::struct_excessive_bools)] // Independent failure modes for the test backend.
struct Fake {
    events: Mutex<Vec<Input>>,
    cursor: Mutex<Point>,
    activity: AtomicU64,
    interrupt: bool,
    fail: bool,
    focused: bool,
    idle: bool,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            events: Mutex::default(),
            cursor: Mutex::new(Point { x: 110, y: 210 }),
            activity: AtomicU64::new(0),
            interrupt: false,
            fail: false,
            focused: true,
            idle: true,
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
    fn agent_display(&self) -> Result<Rect> {
        Ok(area())
    }
    fn user_display(&self) -> Result<Rect> {
        Ok(area())
    }
    fn capture(&self, r: Rect) -> Result<Frame> {
        Ok(Frame {
            width: r.width,
            height: r.height,
            rgb: vec![0; r.width as usize * r.height as usize * 3],
        })
    }
    fn cursor(&self) -> Result<Point> {
        Ok(*self.cursor.lock().unwrap())
    }
    fn send(&self, input: &Input) -> Result<()> {
        self.events.lock().unwrap().push(*input);
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
            bail!("injected backend failure");
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
    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Ok(vec![WindowInfo {
            id: 1,
            title: "test".into(),
            process: "test".into(),
            pid: 1,
            rect: area(),
            minimized: false,
        }])
    }
    fn change_window(&self, _: WindowId, _: WindowChange) -> Result<()> {
        Ok(())
    }
    fn launch(&self, _: &str, _: &[String], _: Option<&str>) -> Result<u32> {
        Ok(1)
    }
    fn processes(&self) -> Result<Vec<(u32, u32)>> {
        Ok(vec![])
    }
    fn clipboard(&self) -> Result<Option<String>> {
        Ok(None)
    }
    fn set_clipboard(&self, _: &str) -> Result<()> {
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
    assert!(steps::parse(&json!(vec![json!({"type":"x"}); 257])).is_err());
    assert_eq!(steps::key("Return").unwrap(), Key::Named(Named::Enter));
    assert_eq!(steps::key("F24").unwrap(), Key::Named(Named::F(24)));
}
#[test]
fn bad_later_coordinate_does_not_execute_earlier_steps() {
    let desktop = Fake::default();
    let steps = steps::parse(&json!([{"type":"must not type"},{"click":[800,1]}])).unwrap();
    assert!(run(&desktop, View::new(area()), &steps).is_err());
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
    assert!(error.to_string().contains("1 completed steps"));
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
        assert!(run(&desktop, View::new(area()), &[Step::Type("x".into())]).is_err());
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
    assert_eq!(
        view.point(steps::At { x: 0., y: 0. }).unwrap(),
        Point { x: -3840, y: -2160 }
    );
    let point = view
        .point(steps::At {
            x: f64::from(view.width - 1),
            y: f64::from(view.height - 1),
        })
        .unwrap();
    assert!(rect.contains(point));
    assert!(
        view.point(steps::At {
            x: f64::from(view.width),
            y: 0.
        })
        .is_err()
    );
}
#[test]
fn image_crop_downsampling_and_png_are_consistent() {
    let image = Frame {
        width: 2,
        height: 2,
        rgb: vec![0, 0, 0, 100, 100, 100, 200, 200, 200, 100, 100, 100],
    };
    assert_eq!(image.shrink(1, 1).rgb, vec![100, 100, 100]);
    assert_eq!(image.crop(1, 1, 100, 100).rgb, vec![100, 100, 100]);
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(image.png_base64().unwrap())
        .unwrap();
    let reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    assert_eq!((reader.info().width, reader.info().height), (2, 2));
    assert!((frame::scale_for(800, 600) - 1.0).abs() < f64::EPSILON);
}

fn run(desktop: &dyn Desktop, view: View, steps: &[Step]) -> Result<runner::Outcome> {
    runner::run(
        desktop,
        view,
        steps,
        &std::sync::atomic::AtomicBool::new(false),
    )
}

#[tokio::test]
async fn protocol_returns_images_and_rechecks_setting_for_existing_credentials() {
    use forge_mcp::ToolSet;
    let dir = tempfile::tempdir().unwrap();
    let store = forge_store::Store::open(&dir.path().join("forge")).unwrap();
    let hub = forge_runs::Hub::new(
        store.clone(),
        forge_engines::Engines::new(dir.path()),
        dir.path().into(),
    );
    let _shutdown = hub.shutdown_on_drop();
    let set = super::ComputerMcp::with_desktop(hub, std::sync::Arc::new(Fake::default()));
    let caller = artisan_api::ThreadId::new();
    let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"screenshot"}});
    let denied = forge_mcp::server::handle(&set, &caller, call.clone())
        .await
        .unwrap();
    assert_eq!(denied["result"]["isError"], true);
    store
        .set_setting("computer_use".into(), json!(true))
        .await
        .unwrap();
    let reply = forge_mcp::server::handle(&set, &caller, call.clone())
        .await
        .unwrap();
    assert_eq!(reply["result"]["isError"], false);
    assert_eq!(reply["result"]["content"][1]["type"], "image");
    assert_eq!(reply["result"]["content"][1]["mimeType"], "image/png");
    assert_eq!(set.tools().len(), 8);
    let zoom = set
        .call(&caller, "zoom", json!({"x":0,"y":0,"width":5,"height":5}))
        .await
        .unwrap();
    assert!(matches!(zoom[1], forge_mcp::Content::Image { .. }));
    store
        .set_setting("computer_use".into(), json!(false))
        .await
        .unwrap();
    let revoked = forge_mcp::server::handle(&set, &caller, call)
        .await
        .unwrap();
    assert_eq!(revoked["result"]["isError"], true);
}

#[tokio::test]
async fn interruption_pauses_subsequent_batches() {
    use forge_mcp::ToolSet;
    let dir = tempfile::tempdir().unwrap();
    let store = forge_store::Store::open(&dir.path().join("forge")).unwrap();
    store
        .set_setting("computer_use".into(), json!(true))
        .await
        .unwrap();
    let hub = forge_runs::Hub::new(
        store,
        forge_engines::Engines::new(dir.path()),
        dir.path().into(),
    );
    let _shutdown = hub.shutdown_on_drop();
    let set = super::ComputerMcp::with_desktop(
        hub,
        std::sync::Arc::new(Fake {
            interrupt: true,
            ..Default::default()
        }),
    );
    let caller = artisan_api::ThreadId::new();
    set.call(&caller, "screenshot", json!({})).await.unwrap();
    let reply = set
        .call(&caller, "input", json!({"steps":[{"key":"ctrl+s"}]}))
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
        .call(&caller, "input", json!({"steps":[{"type":"do not type"}]}))
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("paused"));
}

#[test]
fn handoff_interrupts_an_in_progress_wait_and_releases_held_inputs() {
    let desktop = Fake::default();
    let paused = std::sync::atomic::AtomicBool::new(false);
    let steps =
        steps::parse(&json!([{"key_down":"shift"},{"wait":{"ms":30000}},{"type":"must not type"}]))
            .unwrap();
    std::thread::scope(|scope| {
        let task =
            scope.spawn(|| runner::run(&desktop, View::new(area()), &steps, &paused).unwrap());
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
        &[Step::Move(steps::At { x: 100., y: 100. })],
    )
    .unwrap();
    assert_eq!(outcome.completed, 1);
    assert!(outcome.settled);
    assert!(!outcome.screen_changed);
    assert_eq!(*desktop.cursor.lock().unwrap(), Point { x: 110, y: 210 });
}
