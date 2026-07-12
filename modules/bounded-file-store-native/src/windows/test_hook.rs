#[cfg(feature = "native-test-hooks")]
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Write,
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::Duration,
};

#[cfg(feature = "native-test-hooks")]
static REPLACE_BARRIERS: OnceLock<Mutex<HashMap<String, Arc<Rendezvous>>>> = OnceLock::new();

#[cfg(feature = "native-test-hooks")]
struct Rendezvous {
    arrived: Mutex<u8>,
    ready: Condvar,
}

#[cfg(feature = "native-test-hooks")]
impl Rendezvous {
    fn new() -> Self {
        Self {
            arrived: Mutex::new(0),
            ready: Condvar::new(),
        }
    }

    fn wait(&self) -> Result<(), ()> {
        let mut arrived = self.arrived.lock().map_err(|_| ())?;

        *arrived = arrived.checked_add(1).ok_or(())?;

        if *arrived == 2 {
            self.ready.notify_all();

            return Ok(());
        }

        if *arrived != 1 {
            return Err(());
        }

        let (arrived, result) = self
            .ready
            .wait_timeout_while(arrived, Duration::from_secs(5), |arrived| *arrived < 2)
            .map_err(|_| ())?;

        if result.timed_out() || *arrived != 2 {
            return Err(());
        }

        Ok(())
    }
}

#[cfg(feature = "native-test-hooks")]
pub(super) fn wait_for_replace_race() -> Result<(), ()> {
    let Ok(key) = std::env::var("ARTISAN_NATIVE_TEST_REPLACE_BARRIER") else {
        return Ok(());
    };

    let registry = REPLACE_BARRIERS.get_or_init(|| Mutex::new(HashMap::new()));
    let rendezvous = {
        let mut barriers = registry.lock().map_err(|_| ())?;

        Arc::clone(
            barriers
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Rendezvous::new())),
        )
    };
    let result = rendezvous.wait();

    if let Ok(mut barriers) = registry.lock()
        && barriers
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, &rendezvous))
    {
        barriers.remove(&key);
    }

    result
}

#[cfg(not(feature = "native-test-hooks"))]
pub(super) fn wait_for_replace_race() -> Result<(), ()> {
    Ok(())
}

#[cfg(feature = "native-test-hooks")]
pub(super) fn crash_at(point: &str) {
    if !std::env::var("ARTISAN_NATIVE_TEST_CRASH_POINT").is_ok_and(|value| value == point) {
        return;
    }

    let proof_path = std::env::var_os("ARTISAN_NATIVE_TEST_CRASH_PROOF")
        .expect("native test crash proof path is missing");
    let mut proof = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(proof_path)
        .expect("native test crash proof could not be created");

    proof
        .write_all(point.as_bytes())
        .expect("native test crash proof could not be written");
    proof
        .sync_all()
        .expect("native test crash proof could not be flushed");

    std::process::abort();
}

#[cfg(not(feature = "native-test-hooks"))]
pub(super) fn crash_at(_: &str) {}
