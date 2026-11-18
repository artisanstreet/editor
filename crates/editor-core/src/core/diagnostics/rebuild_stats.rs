use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const WINDOW: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct RebuildStats {
    window_start: Option<Duration>,
    events: BTreeMap<&'static str, u32>,
    rebuilds: u32,
    reasons: BTreeMap<&'static str, u32>,
    spans: BTreeMap<&'static str, (u32, Duration, Duration)>,
}

impl RebuildStats {
    fn event(&mut self, now: Duration, kind: &'static str) -> Option<String> {
        *self.events.entry(kind).or_default() += 1;
        self.roll(now)
    }

    fn rebuild(&mut self, now: Duration, reasons: &[&'static str]) -> Option<String> {
        self.rebuilds += 1;
        for reason in reasons {
            *self.reasons.entry(reason).or_default() += 1;
        }
        self.roll(now)
    }

    fn span(&mut self, now: Duration, name: &'static str, took: Duration) -> Option<String> {
        let entry = self.spans.entry(name).or_default();
        entry.0 += 1;
        entry.1 += took;
        entry.2 = entry.2.max(took);
        self.roll(now)
    }

    fn roll(&mut self, now: Duration) -> Option<String> {
        let start = *self.window_start.get_or_insert(now);
        (now.saturating_sub(start) >= WINDOW).then(|| {
            let report = format!(
                "rebuilds={} reasons={:?} events={:?} spans={}",
                self.rebuilds,
                self.reasons,
                self.events,
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
                window_start: Some(now),
                ..Self::default()
            };
            report
        })
    }
}

#[cfg(feature = "test-support")]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub rebuilds: u32,
    pub reasons: BTreeMap<&'static str, u32>,
    pub events: BTreeMap<&'static str, u32>,
}

#[cfg(feature = "test-support")]
impl Tally {
    #[must_use]
    pub fn event(&self, kind: &str) -> u32 {
        self.events.get(kind).copied().unwrap_or(0)
    }
}

#[cfg(feature = "test-support")]
thread_local! {
    static TALLY: RefCell<Tally> = RefCell::new(Tally::default());
}

#[cfg(feature = "test-support")]
pub fn take_tally() -> Tally {
    TALLY.with(|tally| std::mem::take(&mut *tally.borrow_mut()))
}

thread_local! {
    static NOTES: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    static STATS: RefCell<RebuildStats> = RefCell::new(RebuildStats::default());
    static ORIGIN: Instant = Instant::now();
}

fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("AQUA_REBUILD_STATS").is_some()
            || std::env::var("AQUA_PROFILE").is_ok_and(|value| value == "1")
    })
}

fn now() -> Duration {
    ORIGIN.with(|origin| origin.elapsed())
}

pub fn record_event(kind: &'static str) {
    #[cfg(feature = "test-support")]
    TALLY.with(|tally| *tally.borrow_mut().events.entry(kind).or_default() += 1);
    if enabled() {
        if let Some(report) = STATS.with(|stats| stats.borrow_mut().event(now(), kind)) {
            tracing::info!(target: "rebuild_stats", "{report}");
        }
    }
}

pub struct Span(&'static str, Option<Instant>);

pub fn span(name: &'static str) -> Span {
    Span(name, enabled().then(Instant::now))
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(start) = self.1 {
            let took = start.elapsed();
            if let Some(report) = STATS.with(|stats| stats.borrow_mut().span(now(), self.0, took)) {
                tracing::info!(target: "rebuild_stats", "{report}");
            }
        }
    }
}

pub fn record_rebuild(reasons: &[&'static str]) {
    #[cfg(feature = "test-support")]
    TALLY.with(|tally| {
        let mut tally = tally.borrow_mut();
        tally.rebuilds += 1;
        for reason in reasons {
            *tally.reasons.entry(reason).or_default() += 1;
        }
    });
    if enabled() {
        if let Some(report) = STATS.with(|stats| stats.borrow_mut().rebuild(now(), reasons)) {
            tracing::info!(target: "rebuild_stats", "{report}");
        }
    }
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
