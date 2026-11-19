use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub rebuilds: u32,
    pub reasons: BTreeMap<&'static str, u32>,
    pub events: BTreeMap<&'static str, u32>,
}

impl Tally {
    fn count(counts: &mut BTreeMap<&'static str, u32>, key: &'static str) {
        let count = counts.entry(key).or_default();
        *count = count.saturating_add(1);
    }
}

#[derive(Debug, Default)]
struct Window {
    start: Option<Duration>,
    spans: BTreeMap<&'static str, (u32, Duration, Duration)>,
}

impl Window {
    fn span(&mut self, name: &'static str, took: Duration) {
        let entry = self.spans.entry(name).or_default();
        entry.0 += 1;
        entry.1 += took;
        entry.2 = entry.2.max(took);
    }

    fn roll(&mut self, now: Duration) -> Option<String> {
        let start = *self.start.get_or_insert(now);
        (now.saturating_sub(start) >= WINDOW).then(|| {
            let tally = take_tally();
            let report = format!(
                "rebuilds={} reasons={:?} events={:?} spans={}",
                tally.rebuilds,
                tally.reasons,
                tally.events,
                self.spans
                    .iter()
                    .map(|(name, (count, total, max))| format!(
                        "{name}:{count}x{:.2}ms(max {:.2})",
                        total.as_secs_f64() * 1000.0,
                        max.as_secs_f64() * 1000.0
                    ))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            *self = Self {
                start: Some(now),
                ..Self::default()
            };
            report
        })
    }
}

thread_local! {
    static TALLY: RefCell<Tally> = RefCell::new(Tally::default());
    static NOTES: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    static WINDOW_STATS: RefCell<Window> = RefCell::new(Window::default());
    static ORIGIN: Instant = Instant::now();
}

pub fn take_tally() -> Tally {
    TALLY.with(|tally| std::mem::take(&mut *tally.borrow_mut()))
}

fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("AQUA_REBUILD_STATS").is_some()
            || std::env::var("AQUA_PROFILE").is_ok_and(|value| value == "1")
    })
}

fn now() -> Duration {
    ORIGIN.with(Instant::elapsed)
}

fn report_window(record: impl FnOnce(&mut Window)) {
    if !enabled() {
        return;
    }
    let report = WINDOW_STATS.with(|window| {
        let mut window = window.borrow_mut();
        record(&mut window);
        window.roll(now())
    });
    if let Some(report) = report {
        tracing::info!(target: "rebuild_stats", "{report}");
    }
}

pub fn record_event(kind: &'static str) {
    TALLY.with(|tally| Tally::count(&mut tally.borrow_mut().events, kind));
    report_window(|_| {});
}

#[derive(Debug)]
pub struct Span(&'static str, Option<Instant>);

pub fn span(name: &'static str) -> Span {
    Span(name, enabled().then(Instant::now))
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(start) = self.1 {
            let took = start.elapsed();
            report_window(|window| window.span(self.0, took));
        }
    }
}

pub fn record_rebuild(reasons: &[&'static str]) {
    TALLY.with(|tally| {
        let mut tally = tally.borrow_mut();
        tally.rebuilds = tally.rebuilds.saturating_add(1);
        for reason in reasons {
            Tally::count(&mut tally.reasons, reason);
        }
    });
    report_window(|_| {});
}

pub fn note(name: &'static str, changed: bool) -> bool {
    if changed {
        NOTES.with(|notes| {
            let mut notes = notes.borrow_mut();
            if !notes.contains(&name) {
                notes.push(name);
            }
        });
    }
    changed
}

pub fn take_notes() -> Vec<&'static str> {
    NOTES.with(|notes| std::mem::take(&mut *notes.borrow_mut()))
}
