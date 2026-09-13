//! Opt-in, bounded redraw capture for comparing native builds on the same machine.

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use gpui::{
    App, Window,
    profiler::{self, FrameEvent, FrameTimingCollector},
};

pub(super) fn start(window: &mut Window) {
    let Some(path) = std::env::var_os("ARTISAN_FRAME_CAPTURE") else {
        return;
    };
    let capture = Capture {
        path: path.into(),
        warmup: Instant::now(),
        started: None,
        collector: None,
        samples: Vec::new(),
        previous_trace: false,
        select_chat: std::env::var_os("ARTISAN_FRAME_CAPTURE_CHAT").is_some(),
    };
    window.on_next_frame(move |window, cx| capture.tick(window, cx));
}

struct Capture {
    path: PathBuf,
    warmup: Instant,
    started: Option<Instant>,
    collector: Option<FrameTimingCollector>,
    samples: Vec<serde_json::Value>,
    previous_trace: bool,
    select_chat: bool,
}

impl Capture {
    fn tick(mut self, window: &mut Window, cx: &mut App) {
        if self.select_chat
            && let Some(Some(view)) = window.root::<super::NativeApplication>()
        {
            self.select_chat = !view.update(cx, |app, cx| {
                if !app.project_picker_action_is_admissible() {
                    return false;
                }
                let target = app.thread_listing.as_ref().and_then(|listing| {
                    listing
                        .threads()
                        .iter()
                        .filter(|thread| {
                            !thread.has_active_work && thread.last_message_at.is_some()
                        })
                        .max_by_key(|thread| thread.last_message_at)
                        .map(|thread| thread.thread_id.clone())
                });
                let Some(target) = target else {
                    return false;
                };
                app.open_thread_from_sidebar(target, cx);
                true
            });
        }
        if self.started.is_none() && self.warmup.elapsed() >= Duration::from_secs(30) {
            self.previous_trace = profiler::trace_enabled();
            profiler::set_trace_enabled(true);
            self.collector = Some(FrameTimingCollector::new());
            self.started = Some(Instant::now());
        }
        if let Some(collector) = &mut self.collector {
            for event in collector.collect_unseen() {
                let window_id = match event {
                    FrameEvent::Draw(frame) => frame.window_id,
                    FrameEvent::Present(frame) => frame.window_id,
                };
                if window_id != window.window_handle().window_id() {
                    continue;
                }
                self.samples.push(match event {
                    FrameEvent::Draw(frame) => serde_json::json!({
                        "kind": "draw", "ms": frame.draw_duration().as_secs_f64() * 1000.0,
                        "dirty_to_draw_ms": frame.dirty_to_draw_duration().map(|d| d.as_secs_f64() * 1000.0),
                    }),
                    FrameEvent::Present(frame) => serde_json::json!({
                        "kind": "present", "ms": frame.present_duration().as_secs_f64() * 1000.0,
                        "interval_ms": frame.animation_interval.map(|d| d.as_secs_f64() * 1000.0),
                    }),
                });
            }
        }
        if let Some(started) = self.started
            && started.elapsed() >= Duration::from_secs(10)
        {
            let elapsed_seconds = started.elapsed().as_secs_f64();
            let screenshot = window.render_to_image();
            let screenshot_error = screenshot.as_ref().err().map(ToString::to_string);
            let screenshot = screenshot.ok();
            let report = serde_json::json!({
                "debug_assertions": cfg!(debug_assertions),
                "screenshot_error": screenshot_error,
                "window_active": window.is_window_active(),
                "gpu": window.gpu_specs(),
                "route": window.root::<super::NativeApplication>().flatten().map(|view| format!("{:?}", view.read(cx).route())),
                "viewport": format!("{:?}", window.viewport_size()),
                "elapsed_seconds": elapsed_seconds,
                "workload": "Full-window redraws after thirty seconds of warmup; no synthetic input",
                "samples": self.samples,
                "recent_foreground_tasks": profiler::get_current_thread_timings(gpui::TasksIncluded::OnlyCompleted).timings.iter().map(|timing| serde_json::json!({
                    "location": timing.location.to_string(),
                    "ms": timing.poll_duration().as_secs_f64() * 1000.0,
                })).collect::<Vec<_>>(),
            });
            profiler::set_trace_enabled(self.previous_trace);
            cx.background_executor()
                .spawn(async move {
                    if let Some(screenshot) = screenshot
                        && let Err(error) = screenshot.save(self.path.with_extension("png"))
                    {
                        eprintln!("frame capture image failed: {error}");
                    }
                    let result = serde_json::to_vec_pretty(&report)
                        .map_err(std::io::Error::other)
                        .and_then(|bytes| std::fs::write(&self.path, bytes));
                    if let Err(error) = result {
                        eprintln!("frame capture failed: {error}");
                    }
                })
                .detach();
            return;
        }
        window.refresh();
        window.on_next_frame(move |window, cx| self.tick(window, cx));
    }
}
