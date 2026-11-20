use crate::Outcome;
use rustix::fs::{Mode, OFlags};
use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const HIERARCHY: &str = "/sys/fs/cgroup";
const FORGE: &str = "forge";
const ENGINES: &str = "engines";
const GIB: u64 = 1 << 30;
const FORGE_WEIGHT: &str = "1000";
const ENGINE_OOM_SCORE: &[u8] = b"500";
const OWN_OOM_SCORE: &CStr = c"/proc/self/oom_score_adj";
const RUN_PREFIX: &str = "run-";
const EMPTY_WITHIN: Duration = Duration::from_secs(10);
const EVENTS_BYTES: usize = 256;

static ENGINES_FOLDER: OnceLock<PathBuf> = OnceLock::new();
static RUNS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Founder,
    Member,
    Outsider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Controller {
    Memory,
    Cpu,
    Io,
}

impl Controller {
    const ALL: [Self; 3] = [Self::Memory, Self::Cpu, Self::Io];

    const fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Cpu => "cpu",
            Self::Io => "io",
        }
    }
}

pub fn unified_membership(proc_self_cgroup: &str) -> Option<&str> {
    proc_self_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(|path| path.trim_end_matches(" (deleted)"))
}

pub fn standing(
    own: &Path,
    systemd_exec_pid: Option<u32>,
    me: u32,
    engines_exist: bool,
) -> Standing {
    let in_subgroup = own.file_name().is_some_and(|name| name == FORGE);
    match (in_subgroup, systemd_exec_pid == Some(me), engines_exist) {
        (true, true, _) => Standing::Founder,
        (true, false, true) => Standing::Member,
        (true, false, false) | (false, _, _) => Standing::Outsider,
    }
}

pub fn delegated(cgroup_controllers: &str) -> Vec<Controller> {
    Controller::ALL
        .into_iter()
        .filter(|controller| {
            cgroup_controllers
                .split_whitespace()
                .any(|name| name == controller.name())
        })
        .collect()
}

pub fn total_memory(meminfo: &str) -> Result<u64, String> {
    let line = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .ok_or_else(|| "/proc/meminfo names no MemTotal".to_owned())?;
    let kilobytes = line
        .trim()
        .strip_suffix("kB")
        .ok_or_else(|| format!("MemTotal `{}` isn't in kB", line.trim()))?;
    kilobytes
        .trim()
        .parse::<u64>()
        .map(|kilobytes| kilobytes * 1024)
        .map_err(|error| format!("MemTotal `{}`: {error}", line.trim()))
}

pub fn engine_memory_ceiling(total: u64) -> u64 {
    let reserve = (total / 5).clamp(2 * GIB, 4 * GIB).min(total / 2);
    total - reserve
}

struct Layout {
    root: PathBuf,
    forge: PathBuf,
    engines: PathBuf,
}

fn locate() -> Result<(Layout, Standing), String> {
    let membership = std::fs::read_to_string("/proc/self/cgroup")
        .map_err(|error| format!("read /proc/self/cgroup: {error}"))?;
    let own = unified_membership(&membership)
        .ok_or_else(|| "this system has no unified cgroup hierarchy".to_owned())?;
    let forge = Path::new(HIERARCHY).join(own.trim_start_matches('/'));
    let root = forge.parent().unwrap_or(&forge).to_path_buf();
    let engines = root.join(ENGINES);
    let systemd_exec_pid = match std::env::var("SYSTEMD_EXEC_PID") {
        Ok(pid) => Some(
            pid.parse::<u32>()
                .map_err(|error| format!("SYSTEMD_EXEC_PID `{pid}`: {error}"))?,
        ),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(pid)) => {
            return Err(format!("SYSTEMD_EXEC_PID `{}` isn't a pid", pid.display()));
        }
    };
    let standing = standing(
        &forge,
        systemd_exec_pid,
        std::process::id(),
        engines.is_dir(),
    );
    Ok((
        Layout {
            root,
            forge,
            engines,
        },
        standing,
    ))
}

fn found(layout: &Layout) -> Result<(), String> {
    match std::fs::create_dir(&layout.engines) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!("create {}: {error}", layout.engines.display())),
    }
}

fn enable(layout: &Layout, controller: Controller) -> Result<(), String> {
    let name = controller.name();
    let controllers = layout.root.join("cgroup.controllers");
    let available = std::fs::read_to_string(&controllers)
        .map(|listed| delegated(&listed))
        .map_err(|error| format!("read {}: {error}", controllers.display()))?;
    if !available.contains(&controller) {
        return Err(format!(
            "the {name} controller isn't delegated to this service"
        ));
    }
    std::fs::write(
        layout.root.join("cgroup.subtree_control"),
        format!("+{name}"),
    )
    .map_err(|error| format!("enable the {name} controller: {error}"))
}

fn set(group: &Path, knob: &str, value: &str) -> Result<(), String> {
    std::fs::write(group.join(knob), value).map_err(|error| format!("write {knob}: {error}"))
}

fn remember(layout: &Layout) -> Result<(), String> {
    CString::new(layout.engines.as_os_str().as_bytes())
        .map(|_| {
            ENGINES_FOLDER.get_or_init(|| layout.engines.clone());
        })
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOwner {
    pub process: u32,
    pub started: Option<u64>,
}

pub fn run_owner(folder_name: &str) -> Option<RunOwner> {
    let parts: Vec<&str> = folder_name.strip_prefix(RUN_PREFIX)?.split('-').collect();
    match parts.as_slice() {
        [process, started, run] => {
            run.parse::<u64>().ok()?;
            Some(RunOwner {
                process: process.parse().ok()?,
                started: Some(started.parse().ok()?),
            })
        }
        [process, run] => {
            run.parse::<u64>().ok()?;
            Some(RunOwner {
                process: process.parse().ok()?,
                started: None,
            })
        }
        _ => None,
    }
}

pub fn start_time(stat: &str) -> Option<u64> {
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

#[derive(Debug, thiserror::Error)]
pub enum StartTimeError {
    #[error("read /proc/{process}/stat")]
    Read {
        process: u32,
        #[source]
        source: std::io::Error,
    },
    #[error("/proc/{process}/stat names no start time")]
    Unnamed { process: u32 },
    #[error("/proc lists no process {process}")]
    Unlisted { process: u32 },
}

fn start_of(process: u32) -> Result<Option<u64>, StartTimeError> {
    match std::fs::read_to_string(format!("/proc/{process}/stat")) {
        Ok(stat) => start_time(&stat)
            .map(Some)
            .ok_or(StartTimeError::Unnamed { process }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(StartTimeError::Read { process, source }),
    }
}

fn abandoned(owner: RunOwner) -> Result<bool, StartTimeError> {
    let alive = start_of(owner.process)?;
    Ok(match (alive, owner.started) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(now), Some(then)) => now != then,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum LeftoverError {
    #[error("list the engine run cgroups in {}", path.display())]
    List {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("tell whether the Forge that made engine run cgroup {} has exited", path.display())]
    Owner {
        path: PathBuf,
        #[source]
        source: StartTimeError,
    },
    #[error("read whether engine run cgroup {} still has processes", path.display())]
    Events {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} names no populated state", path.display())]
    NoState { path: PathBuf },
    #[error("name engine run cgroup {}", path.display())]
    Name {
        path: PathBuf,
        #[source]
        source: RunGroupError,
    },
    #[error("end engine run cgroup {} an earlier Forge left", path.display())]
    End {
        path: PathBuf,
        #[source]
        source: EndRunError,
    },
}

#[derive(Debug, Default)]
struct Leftovers {
    abandoned: Vec<PathBuf>,
    failures: Vec<LeftoverError>,
}

fn leftovers(engines: &Path) -> Leftovers {
    let mut found = Leftovers::default();
    let entries = match std::fs::read_dir(engines) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return found,
        Err(source) => {
            found.failures.push(LeftoverError::List {
                path: engines.to_path_buf(),
                source,
            });
            return found;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(source) => {
                found.failures.push(LeftoverError::List {
                    path: engines.to_path_buf(),
                    source,
                });
                continue;
            }
        };
        let Some(owner) = entry.file_name().to_str().and_then(run_owner) else {
            continue;
        };
        match abandoned(owner) {
            Ok(true) => found.abandoned.push(entry.path()),
            Ok(false) => {}
            Err(source) => found.failures.push(LeftoverError::Owner {
                path: entry.path(),
                source,
            }),
        }
    }
    found
}

#[derive(Debug, Default)]
struct Sweep {
    ended: usize,
    failures: Vec<LeftoverError>,
}

fn sweep_leftover_runs(engines: &Path) -> Sweep {
    let Leftovers {
        abandoned,
        failures,
    } = leftovers(engines);
    let mut sweep = Sweep { ended: 0, failures };
    for path in abandoned {
        let ended = RunGroup::at(path.clone())
            .map_err(|source| LeftoverError::Name {
                path: path.clone(),
                source,
            })
            .and_then(|group| {
                group
                    .end()
                    .map_err(|source| LeftoverError::End { path, source })
            });
        match ended {
            Ok(()) => sweep.ended += 1,
            Err(failure) => sweep.failures.push(failure),
        }
    }
    sweep
}

fn has_processes(group: &Path) -> Result<bool, LeftoverError> {
    let path = group.join("cgroup.events");
    let events = match std::fs::read_to_string(&path) {
        Ok(events) => events,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(LeftoverError::Events { path, source }),
    };
    populated(&events).ok_or(LeftoverError::NoState { path })
}

pub(crate) fn leftovers_alive() -> Result<bool, LeftoverError> {
    let Some(engines) = ENGINES_FOLDER.get() else {
        return Ok(false);
    };
    let Leftovers {
        abandoned,
        failures,
    } = leftovers(engines);
    let mut unknown = failures.into_iter().next();
    for group in abandoned {
        match has_processes(&group) {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(failure) => unknown = unknown.or(Some(failure)),
        }
    }
    unknown.map_or(Ok(false), Err)
}

fn swept(sweep: Sweep) -> impl Iterator<Item = Outcome> {
    const SWEEP: &str = "run cgroups left by an earlier Forge are killed and removed";
    let ended = Outcome::Applied(format!(
        "{} run cgroups left by an earlier Forge were killed and removed",
        sweep.ended
    ));
    let failed = sweep.failures.into_iter().map(|failure| Outcome::Skipped {
        measure: SWEEP.to_owned(),
        reason: format!(
            "{}; engine configuration stays guarded while such a cgroup has processes",
            diagnostics::chain(&failure)
        ),
    });
    std::iter::once(ended).chain(failed)
}

pub fn protect_forge() -> Vec<Outcome> {
    const SEPARATION: &str = "each engine run has its own cgroup, killed whole when the run ends";
    let unavailable = |reason: String| {
        vec![Outcome::Skipped {
            measure: SEPARATION.to_owned(),
            reason: format!(
                "{reason}; an engine run is killed by its process group instead, so a process it detaches with setsid outlives the run"
            ),
        }]
    };
    let (layout, founder) = match locate() {
        Ok((layout, Standing::Founder)) => (layout, true),
        Ok((layout, Standing::Member)) => (layout, false),
        Ok((_, Standing::Outsider)) => {
            return unavailable(
                "Forge wasn't started by systemd in a delegated `forge` subgroup".to_owned(),
            );
        }
        Err(reason) => return unavailable(reason),
    };
    let entered = if founder {
        found(&layout).and_then(|()| remember(&layout))
    } else {
        remember(&layout)
    };
    if let Err(reason) = entered {
        return unavailable(reason);
    }
    let mut outcomes = vec![Outcome::Applied(SEPARATION.to_owned())];
    outcomes.extend(swept(sweep_leftover_runs(&layout.engines)));
    if founder {
        outcomes.extend(share(&layout));
    }
    outcomes
}

fn share(layout: &Layout) -> [Outcome; 4] {
    let memory = enable(layout, Controller::Memory);
    let ceiling = std::fs::read_to_string("/proc/meminfo")
        .map_err(|error| format!("read /proc/meminfo: {error}"))
        .and_then(|meminfo| total_memory(&meminfo))
        .map(engine_memory_ceiling);
    let capped = memory
        .clone()
        .and(ceiling)
        .and_then(|bytes| set(&layout.engines, "memory.high", &bytes.to_string()).map(|()| bytes));
    let unswappable = memory.and_then(|()| set(&layout.forge, "memory.swap.max", "0"));
    let cpu = enable(layout, Controller::Cpu)
        .and_then(|()| set(&layout.forge, "cpu.weight", FORGE_WEIGHT));
    let io = enable(layout, Controller::Io).and_then(|()| {
        set(
            &layout.forge,
            "io.weight",
            &format!("default {FORGE_WEIGHT}"),
        )
    });
    [
        match capped {
            Ok(bytes) => Outcome::Applied(format!(
                "engines reclaim their own memory above {} MiB",
                bytes >> 20
            )),
            Err(reason) => Outcome::Skipped {
                measure: "engines reclaim their own memory".to_owned(),
                reason,
            },
        },
        Outcome::of("Forge is never swapped out", unswappable),
        Outcome::of("Forge is scheduled ahead of engines", cpu),
        Outcome::of("Forge's disk requests go ahead of engines'", io),
    ]
}

#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("move engine process {pid} into {path}")]
    JoinGroup {
        pid: u32,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("raise the OOM score of engine process {pid} at {path}")]
    OomScore {
        pid: u32,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn exited(error: &std::io::Error) -> bool {
    rustix::io::Errno::from_io_error(error) == Some(rustix::io::Errno::SRCH)
}

fn write_existing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::io::Write::write_all(
        &mut std::fs::OpenOptions::new().write(true).open(path)?,
        bytes,
    )
}

pub fn attach(engines: Option<&Path>, proc_root: &Path, pid: u32) -> Result<(), AttachError> {
    if let Some(path) = engines {
        match write_existing(path, pid.to_string().as_bytes()) {
            Ok(()) => {}
            Err(source) if exited(&source) => return Ok(()),
            Err(source) => {
                return Err(AttachError::JoinGroup {
                    pid,
                    path: path.to_owned(),
                    source,
                });
            }
        }
    }
    let process = proc_root.join(pid.to_string());
    let path = process.join("oom_score_adj");
    match write_existing(&path, ENGINE_OOM_SCORE) {
        Ok(()) => Ok(()),
        Err(source) if exited(&source) || !process.exists() => Ok(()),
        Err(source) => Err(AttachError::OomScore { pid, path, source }),
    }
}

#[derive(Debug)]
pub struct RunGroup {
    folder: PathBuf,
    procs: CString,
}

#[derive(Debug, thiserror::Error)]
pub enum RunGroupError {
    #[error("create the cgroup {} for an engine run", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("remove the stale cgroup {} before an engine run takes its name", path.display())]
    Stale {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("tell which Forge owns the engine run cgroups in {}", path.display())]
    Owner {
        path: PathBuf,
        #[source]
        source: StartTimeError,
    },
    #[error("name the cgroup {} for an engine run", path.display())]
    Name {
        path: PathBuf,
        #[source]
        source: std::ffi::NulError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum EndRunError {
    #[error("kill the processes of engine run cgroup {}", path.display())]
    Kill {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("watch engine run cgroup {} until it is empty", path.display())]
    Watch {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("engine run cgroup {} still had processes {}s after they were killed", path.display(), after.as_secs())]
    StillPopulated { path: PathBuf, after: Duration },
    #[error("remove engine run cgroup {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn populated(events: &str) -> Option<bool> {
    events
        .lines()
        .find_map(|line| line.strip_prefix("populated "))
        .and_then(|value| match value.trim() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        })
}

impl RunGroup {
    pub(crate) fn create() -> Result<Option<Self>, RunGroupError> {
        match ENGINES_FOLDER.get() {
            Some(engines) => Self::create_in(engines).map(Some),
            None => Ok(None),
        }
    }

    fn create_in(engines: &Path) -> Result<Self, RunGroupError> {
        let run = RUNS.fetch_add(1, Ordering::Relaxed);
        let process = std::process::id();
        let own_start = start_of(process)
            .and_then(|started| started.ok_or(StartTimeError::Unlisted { process }))
            .map_err(|source| RunGroupError::Owner {
                path: engines.to_path_buf(),
                source,
            })?;
        let folder = engines.join(format!("{RUN_PREFIX}{process}-{own_start}-{run}"));
        let created = match std::fs::create_dir(&folder) {
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                std::fs::remove_dir(&folder).map_err(|source| RunGroupError::Stale {
                    path: folder.clone(),
                    source,
                })?;
                std::fs::create_dir(&folder)
            }
            other => other,
        };
        created.map_err(|source| RunGroupError::Create {
            path: folder.clone(),
            source,
        })?;
        Self::at(folder)
    }

    fn at(folder: PathBuf) -> Result<Self, RunGroupError> {
        let procs = folder.join("cgroup.procs");
        let procs =
            CString::new(procs.as_os_str().as_bytes()).map_err(|source| RunGroupError::Name {
                path: procs.clone(),
                source,
            })?;
        Ok(Self { folder, procs })
    }

    pub(crate) fn kill_switch(&self) -> CString {
        CString::new(self.folder.join("cgroup.kill").as_os_str().as_bytes()).expect(
            "invariant: a cgroup folder named without NUL bytes has a kill file named without them",
        )
    }

    pub(crate) fn membership(&self) -> CString {
        self.procs.clone()
    }

    pub(crate) fn end(&self) -> Result<(), EndRunError> {
        let kill = self.folder.join("cgroup.kill");
        match std::fs::write(&kill, b"1") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(EndRunError::Kill { path: kill, source }),
        }
        self.wait_empty(EMPTY_WITHIN)?;
        match std::fs::remove_dir(&self.folder) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(EndRunError::Remove {
                path: self.folder.clone(),
                source,
            }),
        }
    }

    fn wait_empty(&self, within: Duration) -> Result<(), EndRunError> {
        let path = self.folder.join("cgroup.events");
        let watching = |source: std::io::Error| EndRunError::Watch {
            path: path.clone(),
            source,
        };
        let events = match std::fs::File::open(&path) {
            Ok(events) => events,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(watching(source)),
        };
        let deadline = Instant::now() + within;
        loop {
            let mut buffer = [0_u8; EVENTS_BYTES];
            let read = events.read_at(&mut buffer, 0).map_err(watching)?;
            let text = String::from_utf8_lossy(&buffer[..read]);
            match populated(&text) {
                Some(false) => return Ok(()),
                Some(true) => {}
                None => {
                    return Err(watching(std::io::Error::other(format!(
                        "{} names no populated state",
                        path.display()
                    ))));
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(EndRunError::StillPopulated {
                    path: self.folder.clone(),
                    after: within,
                });
            }
            let timeout = rustix::time::Timespec::try_from(remaining)
                .expect("invariant: a wait of at most ten seconds fits a timespec");
            let mut watched = [rustix::event::PollFd::new(
                &events,
                rustix::event::PollFlags::PRI,
            )];
            match rustix::event::poll(&mut watched, Some(&timeout)) {
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(source) => return Err(watching(source.into())),
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum Cgroup {
    Unseparated,
    Shared(CString),
    Run(RunGroup),
}

impl Cgroup {
    pub(crate) fn own() -> Result<Self, RunGroupError> {
        RunGroup::create().map(|group| group.map_or(Self::Unseparated, Self::Run))
    }

    pub(crate) fn shared() -> Self {
        ENGINES_FOLDER.get().map_or(Self::Unseparated, |engines| {
            Self::Shared(
                CString::new(engines.join("cgroup.procs").as_os_str().as_bytes()).expect(
                    "invariant: the engines cgroup was checked to be named without NUL bytes",
                ),
            )
        })
    }

    pub(crate) fn membership(&self) -> Option<CString> {
        match self {
            Self::Unseparated => None,
            Self::Shared(procs) => Some(procs.clone()),
            Self::Run(group) => Some(group.membership()),
        }
    }

    pub(crate) fn kill_switch(&self) -> Option<CString> {
        match self {
            Self::Unseparated | Self::Shared(_) => None,
            Self::Run(group) => Some(group.kill_switch()),
        }
    }

    pub(crate) fn end(&self) -> Result<(), EndRunError> {
        match self {
            Self::Unseparated | Self::Shared(_) => Ok(()),
            Self::Run(group) => group.end(),
        }
    }
}

pub(crate) fn started(cgroup: &Cgroup, pid: u32) {
    let membership = cgroup.membership();
    let membership = membership
        .as_deref()
        .map(|procs| Path::new(std::ffi::OsStr::from_bytes(procs.to_bytes())));
    if let Err(failure) = attach(membership, Path::new("/proc"), pid) {
        let error: &dyn std::error::Error = &failure;
        tracing::warn!(error, "engine isolation is incomplete");
    }
}

fn overwrite(path: &CStr, bytes: &[u8]) -> rustix::io::Result<()> {
    let file = rustix::fs::open(path, OFlags::WRONLY | OFlags::CLOEXEC, Mode::empty())?;
    rustix::io::write(&file, bytes)?;
    Ok(())
}

pub(crate) fn enter(group: Option<&CStr>) -> rustix::io::Result<()> {
    if let Some(membership) = group {
        overwrite(membership, b"0")?;
    }
    overwrite(OWN_OOM_SCORE, ENGINE_OOM_SCORE)
}
