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
    let scroll = std::env::var_os("ARTISAN_FRAME_CAPTURE_SCROLL").is_some();
    let original_size = scroll.then(|| window.viewport_size());
    if scroll {
        window.resize(gpui::size(gpui::px(1024.0), gpui::px(480.0)));
    }
    let vsync = std::env::var("ARTISAN_FRAME_CAPTURE_VSYNC")
        .ok()
        .map(|value| value == "1");
    if let Some(vsync) = vsync {
        window.set_vsync(vsync);
    }
    let capture = Capture {
        path: path.into(),
        warmup: Instant::now(),
        started: None,
        collector: None,
        samples: Vec::new(),
        previous_trace: false,
        select_chat: std::env::var_os("ARTISAN_FRAME_CAPTURE_CHAT").is_some(),
        original_size,
        vsync,
        next_wheel: Instant::now(),
        wheel_up: true,
        wheel_events: 0,
        scroll_range: 0.0,
        active_frames: 0,
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
    original_size: Option<gpui::Size<gpui::Pixels>>,
    vsync: Option<bool>,
    next_wheel: Instant,
    wheel_up: bool,
    wheel_events: usize,
    scroll_range: f32,
    active_frames: usize,
}

impl Capture {
    fn scroll_frame(&mut self, window: &mut Window, cx: &mut App) {
        let Some(view) = window.root::<super::NativeApplication>().flatten() else {
            return;
        };
        let Some(host) = view.read(cx).conversation_host.as_ref() else {
            return;
        };
        let surface = host.read(cx).surface();
        let scroll = surface.read(cx).scroll_handle();
        let maximum = f32::from(scroll.max_offset().y);
        self.scroll_range = self.scroll_range.max(maximum);
        if maximum <= 0.0 {
            return;
        }
        let offset = f32::from(scroll.offset().y);
        if offset >= -1.0 {
            self.wheel_up = false;
        }
        if offset <= -maximum + 1.0 {
            self.wheel_up = true;
        }
        let position = scroll.bounds().center();
        window.dispatch_event(
            gpui::PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                position,
                delta: gpui::ScrollDelta::Lines(gpui::point(
                    0.0,
                    if self.wheel_up { 1.0 } else { -1.0 },
                )),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::default(),
            }),
            cx,
        );
        self.wheel_events += 1;
    }

    fn select_capture_chat(&mut self, window: &mut Window, cx: &mut App) {
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
    }

    fn tick(mut self, window: &mut Window, cx: &mut App) {
        self.select_capture_chat(window, cx);
        if self.started.is_none() && self.warmup.elapsed() >= Duration::from_secs(30) {
            window.activate_window();
            self.previous_trace = profiler::trace_enabled();
            profiler::set_trace_enabled(true);
            self.collector = Some(FrameTimingCollector::new());
            self.started = Some(Instant::now());
        }
        if self.original_size.is_some()
            && self.started.is_some()
            && Instant::now() >= self.next_wheel
        {
            self.scroll_frame(window, cx);
            self.next_wheel = Instant::now() + Duration::from_millis(8);
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
                if window.is_window_active() {
                    self.active_frames += 1;
                }
                self.samples.push(match event {
                    FrameEvent::Draw(frame) => serde_json::json!({
                        "kind": "draw", "active": window.is_window_active(), "ms": frame.draw_duration().as_secs_f64() * 1000.0,
                        "dirty_to_draw_ms": frame.dirty_to_draw_duration().map(|d| d.as_secs_f64() * 1000.0),
                    }),
                    FrameEvent::Present(frame) => serde_json::json!({
                        "kind": "present", "active": window.is_window_active(), "ms": frame.present_duration().as_secs_f64() * 1000.0,
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
                "workload": if self.original_size.is_some() { "Continuous synthetic wheel input after thirty seconds of warmup; no forced redraws during measurement" } else { "Full-window redraws after thirty seconds of warmup; no synthetic input" },
                "vsync_override": self.vsync,
                "wheel_events": self.wheel_events,
                "scroll_range_px": self.scroll_range,
                "active_frame_events": self.active_frames,
                "samples": self.samples,
                "recent_foreground_tasks": profiler::get_current_thread_timings(gpui::TasksIncluded::OnlyCompleted).timings.iter().map(|timing| serde_json::json!({
                    "location": timing.location.to_string(),
                    "ms": timing.poll_duration().as_secs_f64() * 1000.0,
                })).collect::<Vec<_>>(),
            });
            profiler::set_trace_enabled(self.previous_trace);
            if let Some(size) = self.original_size {
                window.resize(size);
            }
            if self.vsync.is_some() {
                window.set_vsync(window.max_frame_rate().is_some());
            }
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
        self.schedule_next_sample(window, cx);
    }

    fn schedule_next_sample(self, window: &mut Window, cx: &mut App) {
        if self.original_size.is_some() && self.started.is_some() {
            window
                .spawn(cx, async move |cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(8))
                        .await;
                    let _ = cx.update(|window, cx| self.tick(window, cx));
                })
                .detach();
        } else {
            window.refresh();
            window.on_next_frame(move |window, cx| self.tick(window, cx));
        }
    }
}
