use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const TRACKED_SCENES: u32 = 8;

struct Opened {
    at: Instant,
    scenes: u32,
    presented: u32,
}

thread_local! {
    static REALIZED: Cell<(u32, Duration)> = const { Cell::new((0, Duration::ZERO)) };
    static FAIL: Cell<&'static str> = const { Cell::new("") };
    static OPENED: RefCell<Option<Opened>> = const { RefCell::new(None) };
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("AQUA_OPEN_TIMING").is_some())
}

fn report(stage: &str, detail: &str) {
    OPENED.with(|opened| {
        if let Some(opened) = opened.borrow().as_ref() {
            eprintln!(
                "open-timing: stage={stage} since_open_ms={:.2} unix_ms={} {detail}",
                opened.at.elapsed().as_secs_f64() * 1000.0,
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
            );
        }
    });
}

pub fn begin() {
    if enabled() {
        OPENED.with(|opened| {
            *opened.borrow_mut() = Some(Opened {
                at: Instant::now(),
                scenes: 0,
                presented: 0,
            });
        });
        report("open_thread", "");
    }
}

pub fn page_applied(items: usize, spans: usize, apply: Duration, rebuild: Duration, rows: usize) {
    if enabled() {
        report(
            "page_applied",
            &format!(
                "items={items} spans={spans} apply_ms={:.2} rebuild_ms={:.2} rows={rows}",
                apply.as_secs_f64() * 1000.0,
                rebuild.as_secs_f64() * 1000.0
            ),
        );
    }
}

pub fn rebuilt(caller: u32, fold: Duration, expand: Duration, project: Duration, rows: usize) {
    if enabled() {
        report(
            "rebuild",
            &format!(
                "caller={caller} fold_ms={:.2} expand_ms={:.2} project_ms={:.2} rows={rows}",
                fold.as_secs_f64() * 1000.0,
                expand.as_secs_f64() * 1000.0,
                project.as_secs_f64() * 1000.0
            ),
        );
    }
}

pub fn scene(build: Duration) {
    if !enabled() {
        return;
    }
    let tracked = OPENED.with(|opened| {
        opened.borrow_mut().as_mut().is_some_and(|opened| {
            opened.scenes += 1;
            opened.scenes <= TRACKED_SCENES
        })
    });
    if tracked {
        let (count, total) = REALIZED.with(Cell::take);
        report(
            "scene",
            &format!(
                "build_ms={:.2} realized={count} realize_ms={:.2}",
                build.as_secs_f64() * 1000.0,
                total.as_secs_f64() * 1000.0
            ),
        );
    }
}

pub fn presented() {
    if !enabled() {
        return;
    }
    let tracked = OPENED.with(|opened| {
        opened.borrow_mut().as_mut().is_some_and(|opened| {
            opened.presented += 1;
            opened.scenes > 0 && opened.presented <= TRACKED_SCENES
        })
    });
    if tracked {
        report("presented", "");
    }
}

pub fn projections_published(bodies: usize, rows: usize) {
    if enabled() {
        report("projections", &format!("bodies={bodies} rows={rows}"));
    }
}

pub fn realized(elapsed: Duration) {
    if enabled() {
        REALIZED.with(|realized| {
            let (count, total) = realized.get();
            realized.set((count + 1, total + elapsed));
        });
    }
}

pub fn acknowledged() {
    if enabled() {
        report("read_ack", "");
    }
}

pub fn gate(
    reason: &str,
    pending: bool,
    changed: bool,
    animating: bool,
    active: bool,
    painted: usize,
    visible: usize,
    candidates: usize,
    elapsed: Duration,
) {
    if enabled() {
        report(
            "gate",
            &format!(
                "reason={reason} fail={} pending={pending} changed={changed} animating={animating} active={active} painted={painted} visible={visible} candidates={candidates} fc_ms={:.2}",
                FAIL.with(|fail| fail.replace("")),
                elapsed.as_secs_f64() * 1000.0
            ),
        );
    }
}

pub fn note_fail(reason: &'static str) {
    if enabled() {
        FAIL.with(|fail| fail.set(reason));
    }
}

pub fn rpc_done(started: Duration, finished: Duration) {
    if enabled() {
        eprintln!(
            "open-timing: read_rpc spawn_ms={:.2} total_ms={:.2}",
            started.as_secs_f64() * 1000.0,
            finished.as_secs_f64() * 1000.0
        );
    }
}

pub fn externals_begin() {
    if enabled() {
        report("externals_begin", "");
    }
}
