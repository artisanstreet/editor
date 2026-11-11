use crate::Outcome;
use rustix::fs::{Mode, OFlags};
use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const HIERARCHY: &str = "/sys/fs/cgroup";
const FORGE: &str = "forge";
const ENGINES: &str = "engines";
const GIB: u64 = 1 << 30;
const FORGE_WEIGHT: &str = "1000";
const ENGINE_OOM_SCORE: &[u8] = b"500";
const OWN_OOM_SCORE: &CStr = c"/proc/self/oom_score_adj";

static ENGINES_MEMBERSHIP: OnceLock<CString> = OnceLock::new();

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

pub fn total_memory(meminfo: &str) -> Option<u64> {
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.trim().strip_suffix("kB"))
        .and_then(|kilobytes| kilobytes.trim().parse::<u64>().ok())
        .map(|kilobytes| kilobytes * 1024)
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
    let systemd_exec_pid = std::env::var("SYSTEMD_EXEC_PID")
        .ok()
        .and_then(|pid| pid.parse().ok());
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
    let available = std::fs::read_to_string(layout.root.join("cgroup.controllers"))
        .map(|controllers| delegated(&controllers))
        .unwrap_or_default();
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
    CString::new(layout.engines.join("cgroup.procs").as_os_str().as_bytes())
        .map(|path| {
            ENGINES_MEMBERSHIP.get_or_init(|| path);
        })
        .map_err(|error| error.to_string())
}

pub fn protect_forge() -> Vec<Outcome> {
    const SEPARATION: &str = "engines run in their own cgroup";
    let unavailable = |reason: String| {
        vec![Outcome::Skipped {
            measure: SEPARATION.to_owned(),
            reason,
        }]
    };
    let layout = match locate() {
        Ok((layout, Standing::Founder)) => layout,
        Ok((layout, Standing::Member)) => return vec![Outcome::of(SEPARATION, remember(&layout))],
        Ok((_, Standing::Outsider)) => {
            return unavailable(
                "Forge wasn't started by systemd in a delegated `forge` subgroup".to_owned(),
            );
        }
        Err(reason) => return unavailable(reason),
    };
    let entered = found(&layout).and_then(|()| remember(&layout));
    if let Err(reason) = entered {
        return unavailable(reason);
    }
    let memory = enable(&layout, Controller::Memory);
    let ceiling = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .as_deref()
        .and_then(total_memory)
        .map(engine_memory_ceiling)
        .ok_or_else(|| "read the machine's memory size".to_owned());
    let capped = memory
        .clone()
        .and(ceiling)
        .and_then(|bytes| set(&layout.engines, "memory.high", &bytes.to_string()).map(|()| bytes));
    let unswappable = memory.and_then(|()| set(&layout.forge, "memory.swap.max", "0"));
    let cpu = enable(&layout, Controller::Cpu)
        .and_then(|()| set(&layout.forge, "cpu.weight", FORGE_WEIGHT));
    let io = enable(&layout, Controller::Io).and_then(|()| {
        set(
            &layout.forge,
            "io.weight",
            &format!("default {FORGE_WEIGHT}"),
        )
    });
    vec![
        Outcome::Applied(SEPARATION.to_owned()),
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

pub(crate) fn engines_membership() -> Option<&'static CStr> {
    ENGINES_MEMBERSHIP.get().map(CString::as_c_str)
}

fn overwrite(path: &CStr, bytes: &[u8]) {
    if let Ok(file) = rustix::fs::open(path, OFlags::WRONLY | OFlags::CLOEXEC, Mode::empty()) {
        rustix::io::write(&file, bytes).ok();
    }
}

pub(crate) fn enter(engines: Option<&CStr>) {
    if let Some(membership) = engines {
        overwrite(membership, b"0");
    }
    overwrite(OWN_OOM_SCORE, ENGINE_OOM_SCORE);
}
