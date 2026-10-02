use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::event::{Event, bounded_args, thread_id};
use crate::export::{Snapshot, writer};

const CHANNEL_CAPACITY: usize = 8192;
pub(crate) const HISTORY_CAPACITY: usize = 32768;
pub(crate) const HISTORY_US: u64 = 120_000_000;
const OPEN_CAPACITY: usize = 2048;
const SLOW_OPERATION_US: u64 = 5_000_000;
const POST_INCIDENT_US: u64 = 2_000_000;
const COOLDOWN_US: u64 = 10_000_000;

static RECORDER: OnceLock<Arc<Recorder>> = OnceLock::new();
static NEXT_SPAN: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
struct Recorder {
    origin: Instant,
    epoch_us: u64,
    events: SyncSender<Event>,
    controls: SyncSender<Control>,
    dropped: AtomicU64,
    running: AtomicBool,
}

impl Recorder {
    fn now(&self) -> u64 {
        self.epoch_us.saturating_add(micros(self.origin.elapsed()))
    }

    fn record(&self, event: Event) {
        if self.running.load(Ordering::Relaxed) && self.events.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Debug)]
enum Control {
    Save,
    Incident(&'static str),
}

/// Own until the application exits. Drop flushes on shutdown, after the UI
/// loop has returned. Recording never waits for a writer or queue capacity.
pub struct Session {
    recorder: Arc<Recorder>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.recorder.running.store(false, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Start once per process. Debug builds record by default; set
/// `ARTISAN_TRACE=0` to disable, or `ARTISAN_TRACE_DIR` to choose the directory.
/// Production builds do not compile this function at all.
pub fn start(role: &'static str) -> Option<Session> {
    if RECORDER.get().is_some() || std::env::var("ARTISAN_TRACE").as_deref() == Ok("0") {
        return None;
    }
    let directory = std::env::var_os("ARTISAN_TRACE_DIR").map_or_else(
        || std::env::temp_dir().join("artisan-traces"),
        PathBuf::from,
    );
    start_at(role, directory)
}

fn start_at(role: &'static str, directory: PathBuf) -> Option<Session> {
    let (events, receiver) = mpsc::sync_channel(CHANNEL_CAPACITY);
    let (controls, control_rx) = mpsc::sync_channel(16);
    let recorder = Arc::new(Recorder {
        origin: Instant::now(),
        epoch_us: micros(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default(),
        ),
        events,
        controls,
        dropped: AtomicU64::new(0),
        running: AtomicBool::new(true),
    });
    let worker_recorder = Arc::clone(&recorder);
    let worker = std::thread::Builder::new()
        .name("artisan-trace-recorder".into())
        .spawn(move || collect(&worker_recorder, &receiver, &control_rx, directory, role))
        .ok()?;
    if RECORDER.set(Arc::clone(&recorder)).is_err() {
        recorder.running.store(false, Ordering::Relaxed);
        let _ = worker.join();
        return None;
    }
    Some(Session {
        recorder,
        worker: Some(worker),
    })
}

/// Nonblocking request to save the preceding history immediately.
#[must_use]
pub fn save() -> bool {
    control(Control::Save)
}

#[must_use]
pub fn is_recording() -> bool {
    RECORDER
        .get()
        .is_some_and(|r| r.running.load(Ordering::Relaxed))
}

pub(crate) fn session_start_us() -> Option<u64> {
    RECORDER.get().map(|recorder| recorder.epoch_us)
}

pub fn incident(reason: &'static str) {
    let _ = control(Control::Incident(reason));
}

fn control(control: Control) -> bool {
    RECORDER.get().is_some_and(|recorder| {
        recorder.running.load(Ordering::Relaxed) && recorder.controls.try_send(control).is_ok()
    })
}

fn emit(category: &'static str, name: &'static str, phase: &'static str, args: Value) {
    if let Some(recorder) = RECORDER.get() {
        recorder.record(Event {
            ts: recorder.now(),
            tid: thread_id(),
            category,
            name,
            phase,
            id: None,
            args: bounded_args(args),
        });
    }
}

pub fn instant(category: &'static str, name: &'static str, args: Value) {
    emit(category, name, "i", args);
}

pub fn counter(category: &'static str, name: &'static str, args: Value) {
    emit(category, name, "C", args);
}

pub fn name_thread(name: &'static str) {
    emit("metadata", "thread_name", "M", json!({"name":name}));
}

/// Import an already measured synchronous task or frame using the same
/// monotonic clock as the recorder, rather than its later collection time.
pub fn completed(
    category: &'static str,
    name: &'static str,
    start: Instant,
    end: Instant,
    args: Value,
) {
    if let Some(recorder) = RECORDER.get() {
        let ts = if start >= recorder.origin {
            recorder
                .epoch_us
                .saturating_add(micros(start.duration_since(recorder.origin)))
        } else {
            recorder
                .epoch_us
                .saturating_sub(micros(recorder.origin.duration_since(start)))
        };
        let mut args = bounded_args(args);
        args["duration_us"] = json!(micros(end.saturating_duration_since(start)));
        recorder.record(Event {
            ts,
            tid: thread_id(),
            category,
            name,
            phase: "X",
            id: None,
            args,
        });
    }
}

/// Async begin/end pair. Its ID remains stable if the executor migrates it.
#[derive(Debug)]
pub struct Span {
    begin: Option<Event>,
}

pub fn span(category: &'static str, name: &'static str, args: Value) -> Span {
    let Some(recorder) = RECORDER.get().filter(|r| r.running.load(Ordering::Relaxed)) else {
        return Span { begin: None };
    };
    let begin = Event {
        ts: recorder.now(),
        tid: thread_id(),
        category,
        name,
        phase: "b",
        id: Some(NEXT_SPAN.fetch_add(1, Ordering::Relaxed)),
        args: bounded_args(args),
    };
    recorder.record(begin.clone());
    Span { begin: Some(begin) }
}

impl Span {
    #[must_use]
    pub const fn inactive() -> Self {
        Self { begin: None }
    }

    /// Connect the producer and consumer lanes with a Chrome flow arrow.
    pub fn handoff(&self) {
        if let (Some(begin), Some(recorder)) = (&self.begin, RECORDER.get()) {
            let mut flow = begin.clone();
            flow.phase = "s";
            flow.category = "flow";
            flow.name = "command.handoff";
            flow.args = json!({});
            recorder.record(flow.clone());
            flow.phase = "f";
            flow.ts = recorder.now();
            flow.tid = thread_id();
            recorder.record(flow);
        }
    }

    #[must_use]
    pub fn id(&self) -> u64 {
        self.begin.as_ref().and_then(|event| event.id).unwrap_or(0)
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let (Some(mut end), Some(recorder)) = (self.begin.take(), RECORDER.get()) {
            end.phase = "e";
            end.ts = recorder.now();
            end.args = json!({});
            recorder.record(end);
        }
    }
}

pub(crate) fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

pub(crate) struct History {
    pub events: VecDeque<Event>,
    pub open: HashMap<u64, (Event, bool)>,
    pub threads: HashMap<u64, Event>,
    pub evicted: u64,
}

impl History {
    pub fn new() -> Self {
        Self {
            events: VecDeque::new(),
            open: HashMap::new(),
            threads: HashMap::new(),
            evicted: 0,
        }
    }

    pub fn push(&mut self, event: Event) {
        if event.phase == "M" && event.name == "thread_name" && self.threads.len() < 256 {
            self.threads.insert(event.tid, event.clone());
        }
        if let Some(id) = event.id {
            match event.phase {
                "b" => {
                    if self.open.len() >= OPEN_CAPACITY
                        && let Some(oldest) = self
                            .open
                            .iter()
                            .min_by_key(|(_, (e, _))| e.ts)
                            .map(|(id, _)| *id)
                    {
                        self.open.remove(&oldest);
                        self.evicted += 1;
                    }
                    self.open.insert(id, (event.clone(), false));
                }
                "e" => {
                    self.open.remove(&id);
                }
                _ => {}
            }
        }
        self.events.push_back(event);
        self.prune();
    }

    fn prune(&mut self) {
        // Producers can enqueue in a different order from their timestamps.
        let newest = self.events.back().map_or(0, |event| event.ts);
        while self.events.len() > HISTORY_CAPACITY
            || self
                .events
                .front()
                .is_some_and(|e| newest.saturating_sub(e.ts) > HISTORY_US)
        {
            self.events.pop_front();
            self.evicted += 1;
        }
    }

    fn expire(&mut self, now: u64) {
        let minimum = now.saturating_sub(HISTORY_US);
        let before = self.events.len();
        self.events.retain(|e| e.ts >= minimum);
        self.evicted += (before - self.events.len()) as u64;
    }

    fn slow_operation(&mut self, now: u64) -> Option<&'static str> {
        for (event, warned) in self.open.values_mut() {
            if !*warned
                && matches!(
                    event.category,
                    "navigation" | "project.intake" | "transport.request" | "ui"
                )
                && now.saturating_sub(event.ts) >= SLOW_OPERATION_US
            {
                *warned = true;
                return Some("operation.stalled");
            }
        }
        None
    }
}

fn collect(
    recorder: &Recorder,
    events: &Receiver<Event>,
    controls: &Receiver<Control>,
    directory: PathBuf,
    role: &'static str,
) {
    let (snapshots, snapshot_rx) = mpsc::sync_channel(2);
    let Ok(writer_thread) = std::thread::Builder::new()
        .name("artisan-trace-writer".into())
        .spawn(move || writer(snapshot_rx, &directory, role))
    else {
        recorder.running.store(false, Ordering::Relaxed);
        eprintln!("Artisan trace recorder could not start its writer thread");
        return;
    };
    let mut history = History::new();
    let mut pending: Option<(u64, &'static str)> = None;
    let mut last_incident: Option<u64> = None;
    while recorder.running.load(Ordering::Relaxed) {
        if let Ok(event) = events.recv_timeout(Duration::from_millis(100)) {
            history.push(event);
        }
        // Bound each drain so continuous producers cannot starve controls.
        for event in events.try_iter().take(1024) {
            history.push(event);
        }
        let now = recorder.now();
        history.expire(now);
        for control in controls.try_iter().take(16) {
            match control {
                Control::Save => send_snapshot(&snapshots, &history, recorder, "manual"),
                Control::Incident(reason) => schedule(&mut pending, last_incident, now, reason),
            }
        }
        if let Some(reason) = history.slow_operation(now) {
            schedule(&mut pending, last_incident, now, reason);
        }
        if let Some((due, reason)) = pending
            && now >= due
        {
            send_snapshot(&snapshots, &history, recorder, reason);
            pending = None;
            last_incident = Some(now);
        }
    }
    for event in events.try_iter() {
        history.push(event);
    }
    // Only this background thread may wait for disk on final shutdown.
    let _ = snapshots.send(Snapshot::new(
        &history,
        recorder.now(),
        recorder.dropped.load(Ordering::Relaxed),
        "shutdown",
    ));
    drop(snapshots);
    let _ = writer_thread.join();
}

#[cfg(test)]
#[path = "../../../tests/tracing/recorder.rs"]
mod tests;

fn schedule(
    pending: &mut Option<(u64, &'static str)>,
    last: Option<u64>,
    now: u64,
    reason: &'static str,
) {
    if pending.is_none() && last.is_none_or(|last| now.saturating_sub(last) >= COOLDOWN_US) {
        *pending = Some((now.saturating_add(POST_INCIDENT_US), reason));
    }
}

fn send_snapshot(
    snapshots: &SyncSender<Snapshot>,
    history: &History,
    recorder: &Recorder,
    reason: &'static str,
) {
    if snapshots
        .try_send(Snapshot::new(
            history,
            recorder.now(),
            recorder.dropped.load(Ordering::Relaxed),
            reason,
        ))
        .is_err()
    {
        recorder.dropped.fetch_add(1, Ordering::Relaxed);
    }
}
