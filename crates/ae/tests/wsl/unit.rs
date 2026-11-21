use super::*;
use std::sync::{Condvar, Mutex};

#[test]
fn distributions_are_probed_at_once_up_to_the_bound_and_reported_in_order() {
    let distros: Vec<String> = (0..9).map(|n| format!("distro-{n}")).collect();
    let running = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let gate = Mutex::new(());
    let changed = Condvar::new();

    let reports = in_parallel(&distros, 3, |distro| {
        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(now, Ordering::SeqCst);
        let open = gate.lock().unwrap();
        changed.notify_all();
        let (open, waited) = changed
            .wait_timeout_while(open, test_support::wait::LIVENESS, |()| {
                peak.load(Ordering::SeqCst) < 3
            })
            .unwrap();
        drop(open);
        assert!(
            !waited.timed_out(),
            "{distro} waited alone: probes run one after another"
        );
        running.fetch_sub(1, Ordering::SeqCst);
        format!("{distro} probed")
    });

    assert_eq!(peak.load(Ordering::SeqCst), 3, "never more than the bound");
    assert_eq!(
        reports,
        distros
            .iter()
            .map(|distro| format!("{distro} probed"))
            .collect::<Vec<_>>()
    );
}
