use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::sync::OnceLock;

const ENGINE_NICENESS: i32 = 5;
const SHELL: &std::ffi::CStr = c"/bin/sh";
const SHELL_COMMAND: &std::ffi::CStr = c"-c";
const KILL_GROUP_WHEN_FORGE_ENDS: &std::ffi::CStr = c"read -r _; kill -s KILL 0";
const SWEEP_GROUP_WHEN_FORGE_ENDS: &std::ffi::CStr =
    c"read -r _; while kill -s KILL -- \"-$1\" 2>/dev/null; do sleep 0.05; done";
const SCRIPT_NAME: &std::ffi::CStr = c"sh";
const DEV_NULL: &std::ffi::CStr = c"/dev/null";

struct Lifeline {
    forge_alive: OwnedFd,
    _held_by_forge: OwnedFd,
}

static LIFELINE: OnceLock<Option<Lifeline>> = OnceLock::new();

fn lifeline() -> Option<RawFd> {
    LIFELINE
        .get_or_init(|| match open_lifeline() {
            Ok(lifeline) => Some(lifeline),
            Err(error) => {
                tracing::warn!(%error, "engine trees can outlive Forge: the lifeline pipe can't be opened");
                None
            }
        })
        .as_ref()
        .map(|lifeline| lifeline.forge_alive.as_raw_fd())
}

#[cfg(not(target_vendor = "apple"))]
fn open_lifeline() -> rustix::io::Result<Lifeline> {
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
    Ok(Lifeline {
        forge_alive: read,
        _held_by_forge: write,
    })
}

#[cfg(target_vendor = "apple")]
fn open_lifeline() -> rustix::io::Result<Lifeline> {
    let (read, write) = rustix::pipe::pipe()?;
    rustix::io::fcntl_setfd(&read, rustix::io::FdFlags::CLOEXEC)?;
    rustix::io::fcntl_setfd(&write, rustix::io::FdFlags::CLOEXEC)?;
    Ok(Lifeline {
        forge_alive: read,
        _held_by_forge: write,
    })
}

pub fn contain(command: &mut std::process::Command, sandbox: Option<crate::Ruleset>) {
    #[cfg(target_os = "linux")]
    let engines = crate::linux::engines_membership();
    let forge_alive = lifeline();
    let enter = move || {
        #[cfg(target_os = "linux")]
        crate::linux::enter(engines);
        #[cfg(target_os = "macos")]
        crate::macos::yield_disk();
        rustix::process::nice(ENGINE_NICENESS).ok();
        if let Some(forge_alive) = forge_alive {
            lead_group_watched_by(forge_alive)?;
        }
        if let Some(ruleset) = &sandbox {
            ruleset.restrict_self()?;
        }
        Ok(())
    };
    unsafe {
        command.pre_exec(enter);
    }
}

fn lead_group_watched_by(forge_alive: RawFd) -> std::io::Result<()> {
    unsafe {
        if libc::setpgid(0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let group = Decimal::of(libc::getpid().unsigned_abs());
        match libc::fork() {
            -1 => Err(std::io::Error::last_os_error()),
            0 => {
                if libc::fork() == 0 {
                    become_watcher(forge_alive, KILL_GROUP_WHEN_FORGE_ENDS, &group);
                }
                if libc::fork() == 0 {
                    libc::setpgid(0, 0);
                    become_watcher(forge_alive, SWEEP_GROUP_WHEN_FORGE_ENDS, &group);
                }
                libc::_exit(0)
            }
            intermediate => {
                libc::waitpid(intermediate, std::ptr::null_mut(), 0);
                Ok(())
            }
        }
    }
}

unsafe fn become_watcher(forge_alive: RawFd, script: &std::ffi::CStr, group: &Decimal) -> ! {
    unsafe {
        let null = libc::open(DEV_NULL.as_ptr(), libc::O_RDWR);
        libc::dup2(forge_alive, 0);
        libc::dup2(null, 1);
        libc::dup2(null, 2);
        let argv = [
            SHELL.as_ptr(),
            SHELL_COMMAND.as_ptr(),
            script.as_ptr(),
            SCRIPT_NAME.as_ptr(),
            group.as_ptr(),
            std::ptr::null(),
        ];
        libc::execv(SHELL.as_ptr(), argv.as_ptr());
        libc::_exit(127)
    }
}

struct Decimal([u8; 11]);

impl Decimal {
    const fn of(value: u32) -> Self {
        let mut text = [0_u8; 11];
        let mut length = 1;
        let mut scale = value / 10;
        while scale > 0 {
            length += 1;
            scale /= 10;
        }
        let mut rest = value;
        while length > 0 {
            length -= 1;
            text[length] = b'0' + (rest % 10) as u8;
            rest /= 10;
        }
        Self(text)
    }

    const fn as_ptr(&self) -> *const std::ffi::c_char {
        self.0.as_ptr().cast()
    }
}
