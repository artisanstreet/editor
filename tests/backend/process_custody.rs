//! Focused whole-process Forge custody tests.
//!
//! The one child fixture is this same Rust test binary. The parent sets the
//! selector and explicit lock path only on each spawned child, so the ambient
//! test environment is never mutated and no separate fixture executable is
//! needed.

use std::{
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{self, Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use artisan_backend::{ForgeProcessCustody, ForgeProcessCustodyError};

const CHILD_SCENARIO_ENV: &str = "ARTISAN_BACKEND_PROCESS_CUSTODY_TEST_SCENARIO";
const CHILD_LOCK_PATH_ENV: &str = "ARTISAN_BACKEND_PROCESS_CUSTODY_TEST_LOCK_PATH";
const CHILD_FILTER: &str = "process_custody_child_fixture";
const CHILD_HOLD: &str = "hold";
const CHILD_CONTEND: &str = "contend";
const CHILD_CREATE_RACE: &str = "create-race";
const CHILD_READY_MARKER: &str = "FORGE_PROCESS_CUSTODY_READY";
const CHILD_BARRIER_READY_MARKER: &str = "FORGE_PROCESS_CUSTODY_BARRIER_READY";
const CHILD_CONTENDED_MARKER: &str = "FORGE_PROCESS_CUSTODY_CONTENDED";
const CHILD_ACQUIRED_MARKER: &str = "FORGE_PROCESS_CUSTODY_ACQUIRED";
const CHILD_RACE_CONTENDED_MARKER: &str = "FORGE_PROCESS_CUSTODY_RACE_CONTENDED";
const CHILD_RACE_ACQUIRED_MARKER: &str = "FORGE_PROCESS_CUSTODY_RACE_ACQUIRED";
const CHILD_FAILURE_EXIT: i32 = 86;
const CHILD_PROTOCOL_FAILURE_EXIT: i32 = 87;
const CHILD_WATCHDOG_EXIT: i32 = 88;
const CHILD_SUCCESS_EXIT: i32 = 0;
const MARKER_TIMEOUT: Duration = Duration::from_secs(5);
const CHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(5);
const CHILD_KILL_TIMEOUT: Duration = Duration::from_secs(2);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CHILD_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(15);

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const _: fn() = || {
    struct CloneMarker;
    trait AmbiguousIfClone<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    impl<T: Clone> AmbiguousIfClone<CloneMarker> for T {}
    let _ = <ForgeProcessCustody as AmbiguousIfClone<_>>::marker;
};

const _: fn() = || {
    struct DefaultMarker;
    trait AmbiguousIfDefault<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDefault<()> for T {}
    impl<T: Default> AmbiguousIfDefault<DefaultMarker> for T {}
    let _ = <ForgeProcessCustody as AmbiguousIfDefault<_>>::marker;
};

/// A uniquely named directory owned by this test process and removed only
/// after every guard and child has been settled.
struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "artisan-process-custody-{}-{sequence}",
            process::id()
        ));
        fs::create_dir(&path).expect("isolated process-custody directory should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.path);
    }
}

/// Parent-side ownership of one child, its stdin release handle, and a
/// one-line stderr monitor. The monitor never needs to outlive the child.
struct FixtureChild {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    marker: Receiver<io::Result<Option<String>>>,
    marker_reader: Option<JoinHandle<()>>,
}

impl FixtureChild {
    fn wait_for_marker(&self) -> String {
        match self.marker.recv_timeout(MARKER_TIMEOUT) {
            Ok(Ok(Some(marker))) => marker,
            Ok(Ok(None)) => panic!("custody fixture exited without a marker"),
            Ok(Err(_)) => panic!("custody fixture marker read failed"),
            Err(RecvTimeoutError::Timeout) => {
                panic!("custody fixture marker exceeded its bounded wait")
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("custody fixture marker monitor disconnected")
            }
        }
    }

    fn release(&mut self) {
        drop(self.stdin.take());
    }

    fn release_barrier(&mut self) {
        let stdin = self
            .stdin
            .as_mut()
            .expect("custody fixture barrier stdin should remain open");
        stdin
            .write_all(&[1_u8])
            .expect("custody fixture barrier should be released");
        stdin
            .flush()
            .expect("custody fixture barrier should be flushed");
    }

    fn finish(&mut self) -> ExitStatus {
        self.release();
        let status = if let Ok(status) = self.wait_until(CHILD_EXIT_TIMEOUT) {
            status
        } else {
            if let Some(child) = self.child.as_mut() {
                let _kill_result = child.kill();
            }
            self.wait_until(CHILD_KILL_TIMEOUT)
                .expect("killed custody fixture must be reaped within the bounded grace")
        };
        self.join_marker_reader();
        status
    }

    fn wait_until(&mut self, budget: Duration) -> Result<ExitStatus, &'static str> {
        let deadline = Instant::now() + budget;
        loop {
            let result = {
                let child = self.child.as_mut().ok_or("child was already reaped")?;
                child.try_wait()
            };
            match result {
                Ok(Some(status)) => {
                    let _finished_child = self.child.take();
                    return Ok(status);
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(CHILD_POLL_INTERVAL),
                Ok(None) => return Err("child did not exit within the bounded wait"),
                Err(_) => return Err("child wait inspection failed"),
            }
        }
    }

    fn join_marker_reader(&mut self) {
        if let Some(reader) = self.marker_reader.take() {
            reader
                .join()
                .expect("custody fixture marker reader should not panic");
        }
    }
}

impl Drop for FixtureChild {
    fn drop(&mut self) {
        self.release();
        if let Some(child) = self.child.as_mut() {
            let _kill_result = child.kill();
            // `kill` is issued only after the bounded normal path has failed;
            // this final wait is the collection step that prevents a failed
            // assertion from leaving the exact fixture process behind.
            let _reap_result = child.wait();
        }
        let _finished_child = self.child.take();
        self.join_marker_reader();
    }
}

fn spawn_fixture(path: &Path, scenario: &str) -> FixtureChild {
    let executable = env::current_exe().expect("current test executable should be available");
    let mut command = Command::new(executable);
    command
        .arg(CHILD_FILTER)
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(CHILD_SCENARIO_ENV, scenario)
        .env(CHILD_LOCK_PATH_ENV, path.as_os_str())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = command.spawn().expect("custody fixture child should spawn");
    let stdin = child.stdin.take();
    let stderr = child
        .stderr
        .take()
        .expect("custody fixture stderr should be piped");
    let (sender, marker) = mpsc::channel();
    let marker_reader = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _send_result = sender.send(Ok(None));
                    break;
                }
                Ok(_) => {
                    if sender
                        .send(Ok(Some(line.trim_end_matches(['\r', '\n']).to_owned())))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => {
                    let _send_result = sender.send(Err(error));
                    break;
                }
            }
        }
    });

    FixtureChild {
        child: Some(child),
        stdin,
        marker,
        marker_reader: Some(marker_reader),
    }
}

fn write_child_marker(marker: &str) {
    let mut stderr = io::stderr().lock();
    if writeln!(stderr, "{marker}")
        .and_then(|()| stderr.flush())
        .is_err()
    {
        process::exit(CHILD_FAILURE_EXIT);
    }
}

fn child_watchdog() {
    let _watchdog = thread::Builder::new()
        .name("process-custody-fixture-watchdog".to_owned())
        .spawn(|| {
            thread::sleep(CHILD_WATCHDOG_TIMEOUT);
            process::exit(CHILD_WATCHDOG_EXIT);
        })
        .expect("custody fixture watchdog should spawn");
}

fn child_wait_for_release() {
    let mut stdin = io::stdin().lock();
    let mut byte = [0_u8; 1];
    let _read_result = stdin.read(&mut byte);
}

fn child_wait_for_barrier() -> bool {
    let mut stdin = io::stdin().lock();
    let mut byte = [0_u8; 1];
    stdin.read_exact(&mut byte).is_ok()
}

fn run_child_fixture() -> ! {
    let scenario = match env::var_os(CHILD_SCENARIO_ENV) {
        Some(value) => match value.to_str() {
            Some(value) => value.to_owned(),
            None => process::exit(CHILD_PROTOCOL_FAILURE_EXIT),
        },
        None => process::exit(CHILD_PROTOCOL_FAILURE_EXIT),
    };
    let lock_path = match env::var_os(CHILD_LOCK_PATH_ENV) {
        Some(path) => PathBuf::from(path),
        None => process::exit(CHILD_PROTOCOL_FAILURE_EXIT),
    };

    if scenario == CHILD_CONTEND {
        match ForgeProcessCustody::acquire(&lock_path) {
            Err(error) if error.is_contention() => {
                write_child_marker(CHILD_CONTENDED_MARKER);
                process::exit(CHILD_SUCCESS_EXIT);
            }
            Ok(_) => {
                write_child_marker(CHILD_ACQUIRED_MARKER);
                process::exit(CHILD_FAILURE_EXIT);
            }
            Err(_) => process::exit(CHILD_FAILURE_EXIT),
        }
    }
    if scenario == CHILD_CREATE_RACE {
        child_watchdog();
        write_child_marker(CHILD_BARRIER_READY_MARKER);
        if !child_wait_for_barrier() {
            process::exit(CHILD_PROTOCOL_FAILURE_EXIT);
        }
        match ForgeProcessCustody::acquire(&lock_path) {
            Ok(custody) => {
                write_child_marker(CHILD_RACE_ACQUIRED_MARKER);
                child_wait_for_release();
                drop(custody);
                process::exit(CHILD_SUCCESS_EXIT);
            }
            Err(error) if error.is_contention() => {
                write_child_marker(CHILD_RACE_CONTENDED_MARKER);
                process::exit(CHILD_SUCCESS_EXIT);
            }
            Err(_) => process::exit(CHILD_FAILURE_EXIT),
        }
    }
    if scenario != CHILD_HOLD {
        process::exit(CHILD_PROTOCOL_FAILURE_EXIT);
    }

    child_watchdog();
    let Ok(custody) = ForgeProcessCustody::acquire(&lock_path) else {
        process::exit(CHILD_FAILURE_EXIT);
    };
    write_child_marker(CHILD_READY_MARKER);
    child_wait_for_release();
    drop(custody);
    process::exit(CHILD_SUCCESS_EXIT);
}

fn create_symlink(target: &Path, link: &Path, directory: bool, label: &str) -> bool {
    #[cfg(unix)]
    let result = {
        let _ = directory;
        std::os::unix::fs::symlink(target, link)
    };
    #[cfg(windows)]
    let result = if directory {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    #[cfg(not(any(unix, windows)))]
    let result: io::Result<()> = {
        let _ = (target, link, directory);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "symbolic links are not supported by this test platform",
        ))
    };

    match result {
        Ok(()) => true,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
            ) =>
        {
            eprintln!("SKIP {label} symlink fixture: platform denied creation");
            false
        }
        Err(error) => panic!("{label} symlink fixture should be creatable: {error}"),
    }
}

#[cfg(unix)]
fn assert_owner_only_mode(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .expect("created custody file metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "missing lock file must be owner-only");
}

#[cfg(not(unix))]
fn assert_owner_only_mode(_: &Path) {}

fn assert_contention(error: &ForgeProcessCustodyError) {
    assert!(
        error.is_contention(),
        "second custody acquisition should be typed contention: {error}"
    );
    assert!(matches!(error, ForgeProcessCustodyError::Contended { .. }));
}

fn acquire_error(path: &Path) -> ForgeProcessCustodyError {
    match ForgeProcessCustody::acquire(path) {
        Ok(_) => panic!("custody acquisition unexpectedly succeeded"),
        Err(error) => error,
    }
}

#[allow(clippy::too_many_lines)]
fn run_parent_scenarios() {
    let directory = TemporaryDirectory::new();

    let created_path = directory.path().join("created.lock");
    assert!(!created_path.exists());
    let first = ForgeProcessCustody::acquire(&created_path).expect("missing lock should create");
    assert!(created_path.is_file());
    assert_owner_only_mode(&created_path);
    assert_contention(&acquire_error(&created_path));
    drop(first);
    assert!(created_path.is_file(), "drop must not unlink the lock file");
    assert!(
        fs::read(&created_path)
            .expect("created lock should be readable after custody drops")
            .is_empty()
    );
    let mut later_process = spawn_fixture(&created_path, CHILD_HOLD);
    assert_eq!(later_process.wait_for_marker(), CHILD_READY_MARKER);
    later_process.release();
    assert!(later_process.finish().success());
    let after_drop = ForgeProcessCustody::acquire(&created_path)
        .expect("dropping the first guard should release custody");
    drop(after_drop);

    let race_path = directory.path().join("create-race.lock");
    assert!(!race_path.exists());
    let mut race_first = spawn_fixture(&race_path, CHILD_CREATE_RACE);
    let mut race_second = spawn_fixture(&race_path, CHILD_CREATE_RACE);
    assert_eq!(race_first.wait_for_marker(), CHILD_BARRIER_READY_MARKER);
    assert_eq!(race_second.wait_for_marker(), CHILD_BARRIER_READY_MARKER);
    // Both children have reached the pipe barrier before either release byte
    // is written, so neither creator is started by a timing guess.
    race_first.release_barrier();
    race_second.release_barrier();

    let first_outcome = race_first.wait_for_marker();
    let second_outcome = race_second.wait_for_marker();
    let first_won = first_outcome == CHILD_RACE_ACQUIRED_MARKER;
    let second_won = second_outcome == CHILD_RACE_ACQUIRED_MARKER;
    let first_contended = first_outcome == CHILD_RACE_CONTENDED_MARKER;
    let second_contended = second_outcome == CHILD_RACE_CONTENDED_MARKER;
    assert!(
        first_won ^ second_won,
        "simultaneous missing-file creators must have exactly one winner; outcomes were {first_outcome:?} and {second_outcome:?}"
    );
    assert!(
        first_contended ^ second_contended,
        "the losing creator must report typed contention; outcomes were {first_outcome:?} and {second_outcome:?}"
    );
    assert_contention(&acquire_error(&race_path));

    if first_won {
        race_first.release();
    } else {
        race_second.release();
    }
    assert!(race_first.finish().success());
    assert!(race_second.finish().success());

    let mut later_race_process = spawn_fixture(&race_path, CHILD_HOLD);
    assert_eq!(later_race_process.wait_for_marker(), CHILD_READY_MARKER);
    later_race_process.release();
    assert!(later_race_process.finish().success());

    let sentinel_path = directory.path().join("sentinel.lock");
    let sentinel = b"sentinel payload must survive";
    fs::write(&sentinel_path, sentinel).expect("sentinel lock should be created");
    assert_eq!(
        fs::read(&sentinel_path).expect("sentinel should be readable before custody"),
        sentinel
    );
    let sentinel_guard =
        ForgeProcessCustody::acquire(&sentinel_path).expect("regular lock should be accepted");
    drop(sentinel_guard);
    assert_eq!(
        fs::read(&sentinel_path).expect("sentinel should remain after drop"),
        sentinel
    );

    let contention_path = directory.path().join("contention.lock");
    let contention_guard =
        ForgeProcessCustody::acquire(&contention_path).expect("contention fixture owner");
    let mut contender = spawn_fixture(&contention_path, CHILD_CONTEND);
    assert_eq!(contender.wait_for_marker(), CHILD_CONTENDED_MARKER);
    assert!(contender.finish().success());
    drop(contention_guard);

    let held_by_child_path = directory.path().join("child-held.lock");
    let mut holder = spawn_fixture(&held_by_child_path, CHILD_HOLD);
    assert_eq!(holder.wait_for_marker(), CHILD_READY_MARKER);
    assert_contention(&acquire_error(&held_by_child_path));
    holder.release();
    assert!(holder.finish().success());
    let later_guard = ForgeProcessCustody::acquire(&held_by_child_path)
        .expect("later acquisition should succeed after child guard drop");
    drop(later_guard);

    let moved_path = directory.path().join("moved.lock");
    let moved = move_guard(ForgeProcessCustody::acquire(&moved_path).expect("move fixture"));
    assert_contention(&acquire_error(&moved_path));
    let moved_again = move_guard(moved);
    assert_contention(&acquire_error(&moved_path));
    drop(moved_again);
    let after_move = ForgeProcessCustody::acquire(&moved_path)
        .expect("dropping the moved guard should release custody");
    drop(after_move);

    let missing_parent = directory.path().join("missing-parent").join("lock");
    assert!(matches!(
        ForgeProcessCustody::acquire(&missing_parent),
        Err(ForgeProcessCustodyError::ParentMissing { .. })
    ));
    let parent_file = directory.path().join("parent-file");
    fs::write(&parent_file, b"parent").expect("non-directory parent fixture");
    let child_of_file = parent_file.join("lock");
    assert!(matches!(
        ForgeProcessCustody::acquire(&child_of_file),
        Err(ForgeProcessCustodyError::ParentNotDirectory { .. })
    ));

    let directory_lock = directory.path().join("directory.lock");
    fs::create_dir(&directory_lock).expect("directory lock fixture");
    assert!(matches!(
        ForgeProcessCustody::acquire(&directory_lock),
        Err(ForgeProcessCustodyError::LockPathNotRegular { .. })
    ));

    let final_target = directory.path().join("final-target");
    let final_link = directory.path().join("final-link");
    fs::write(&final_target, b"final target").expect("final symlink target");
    if create_symlink(&final_target, &final_link, false, "final") {
        let error = acquire_error(&final_link);
        assert!(matches!(
            error,
            ForgeProcessCustodyError::LockPathSymlink { .. }
                | ForgeProcessCustodyError::LockPathReparsePoint { .. }
        ));
        assert!(!format!("{error}").contains("final target"));
        assert_eq!(
            fs::read(&final_target).expect("final target should survive"),
            b"final target"
        );
    }

    let parent_target = directory.path().join("parent-target");
    let parent_link = directory.path().join("parent-link");
    fs::create_dir(&parent_target).expect("parent symlink target");
    if create_symlink(&parent_target, &parent_link, true, "parent") {
        let error = acquire_error(&parent_link.join("lock"));
        assert!(matches!(
            error,
            ForgeProcessCustodyError::ParentSymlink { .. }
                | ForgeProcessCustodyError::ParentReparsePoint { .. }
        ));
    }
}

fn move_guard(guard: ForgeProcessCustody) -> ForgeProcessCustody {
    guard
}

#[test]
fn process_custody_child_fixture() {
    if env::var_os(CHILD_SCENARIO_ENV).is_some() {
        run_child_fixture();
    }
    run_parent_scenarios();
}
