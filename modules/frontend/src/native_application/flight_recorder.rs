//! UI heartbeat, GPUI frame/task import, and content-free navigation state.

use std::time::{Duration, Instant};

use artisan_tracing::{Heartbeat, completed};
use gpui::{
    App,
    profiler::{self, FrameEvent, FrameTimingCollector},
};

use super::*;

pub(super) fn start(cx: &mut App) {
    if !artisan_tracing::is_recording() {
        return;
    }
    artisan_tracing::name_thread("editor.ui");
    profiler::set_trace_enabled(true);
    let heartbeat = Heartbeat::new("editor.ui");
    let mut frames = FrameTimingCollector::new();
    let mut last_task = Instant::now();
    let mut last_task_collection = last_task;
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            cx.update(|_| {
                heartbeat.tick();
                for frame in frames.collect_unseen() {
                    match frame {
                        FrameEvent::Draw(frame) => {
                            completed("frame", "draw", frame.draw_start, frame.draw_end, serde_json::json!({
                                "invalidations":frame.invalidations,
                                "dirty_to_draw_ms":frame.dirty_to_draw_duration().map(|d| d.as_secs_f64() * 1000.0),
                            }));
                        }
                        FrameEvent::Present(frame) => {
                            completed("frame.present", "present", frame.present_start, frame.present_end, serde_json::json!({
                                "interval_ms":frame.animation_interval.map(|d| d.as_secs_f64() * 1000.0),
                            }));
                        }
                    }
                }
                if last_task_collection.elapsed() < Duration::from_secs(1) {
                    return;
                }
                last_task_collection = Instant::now();
                let mut newest = last_task;
                for task in profiler::get_current_thread_timings(gpui::TasksIncluded::OnlyCompleted).timings {
                    if task.end.0 > last_task {
                        newest = newest.max(task.end.0);
                        if task.poll_duration() >= Duration::from_millis(10) {
                            completed("ui.task", "foreground.poll", task.start, task.end.0, serde_json::json!({"location":task.location.to_string()}));
                        }
                    }
                }
                last_task = newest;
            });
        }
    }).detach();
}

pub(super) const fn route_name(route: &NativeRoute) -> &'static str {
    match route {
        NativeRoute::NewThread { .. } => "new_thread",
        NativeRoute::Thread { .. } => "thread",
        NativeRoute::Editor { .. } => "editor",
        NativeRoute::Settings { .. } => "settings",
        NativeRoute::Onboarding => "onboarding",
    }
}

impl NativeApplication {
    pub(super) fn trace_state(&self) {
        artisan_tracing::counter!("state", "connection.holds",
            "count" => self.service.as_ref().map_or(0, |s| s.holds().status().count),
            "sealed" => self.service.as_ref().is_some_and(|s| s.holds().status().sealed) as u8
        );
        artisan_tracing::instant!("state", "navigation.gates",
            "route" => route_name(self.route()),
            "shutdown" => self.shutdown_prepared,
            "service_stopped" => self.service_stopped,
            "intake" => self.intake_stage.map(|s| format!("{s:?}")),
            "switch_phase" => self.thread_switch_flight.as_ref().map(|f| format!("{:?}", f.phase)),
            "switch_span" => self.thread_switch_flight.as_ref().map(|f| f.trace.id()),
            "switch_generation" => self.thread_switch_flight.as_ref().map(|f| f.generation),
            "unsubscribe_pending" => self.ordinary_unsubscribe_thread.as_ref().map(ThreadId::as_str),
            "pending_thread" => self.pending_thread.as_ref().map(ThreadId::as_str),
            "selected_thread" => self.selected_thread.as_ref().map(ThreadId::as_str),
            "awaiting_threads" => self.project_navigation.awaiting_threads,
            "window_error_present" => self.window_error.is_some(),
            "navigation_admissible" => self.project_picker_action_is_admissible()
        );
    }
}
