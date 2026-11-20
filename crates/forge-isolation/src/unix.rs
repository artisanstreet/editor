use std::ffi::{CStr, CString};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::sync::OnceLock;

const ENGINE_NICENESS: i32 = 5;
const SHELL: &CStr = c"/bin/sh";
const SHELL_COMMAND: &CStr = c"-c";
const SCRIPT_NAME: &CStr = c"sh";
const DEV_NULL: &CStr = c"/dev/null";
const ROOT: &CStr = c"/";
const NO_ARGUMENT: &CStr = c"";
#[cfg(target_os = "linux")]
const KILL_RUN_WHEN_FORGE_ENDS: &CStr = c"read -r _; echo 1 > \"$1\"; kill -s KILL 0";
const KILL_GROUP_WHEN_FORGE_ENDS: &CStr = c"read -r _; kill -s KILL 0";
#[cfg(not(target_os = "linux"))]
const ASK_FOR_A_SWEEP_WHEN_FORGE_ENDS: &CStr = c"read -r _; echo sweep; kill -s KILL 0";
#[cfg(not(target_os = "linux"))]
const SWEEP_GROUP_WHEN_ASKED: &CStr = c"read -r asked; [ \"$asked\" = sweep ] || exit 0; while kill -s KILL -- \"-$1\" 2>/dev/null; do /bin/sleep 0.05; done";

struct Lifeline {
    forge_alive: OwnedFd,
    _held_by_forge: OwnedFd,
}

static LIFELINE: OnceLock<Lifeline> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum ContainError {
    #[error("open the pipe that ends engine runs when Forge exits")]
    Lifeline(#[source] std::io::Error),
}

fn lifeline() -> Result<RawFd, ContainError> {
    if let Some(lifeline) = LIFELINE.get() {
        return Ok(lifeline.forge_alive.as_raw_fd());
    }
    let opened = open_lifeline().map_err(|errno| ContainError::Lifeline(errno.into()))?;
    Ok(LIFELINE.get_or_init(|| opened).forge_alive.as_raw_fd())
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

#[derive(Debug, Clone)]
struct Watch {
    script: &'static CStr,
    argument: CString,
}

impl Watch {
    #[cfg(target_os = "linux")]
    fn of(tree: &crate::EngineTree) -> Self {
        match tree.kill_switch() {
            Some(kill_switch) => Self {
                script: KILL_RUN_WHEN_FORGE_ENDS,
                argument: kill_switch,
            },
            None => Self {
                script: KILL_GROUP_WHEN_FORGE_ENDS,
                argument: NO_ARGUMENT.to_owned(),
            },
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn of(_tree: &crate::EngineTree) -> Self {
        Self {
            script: KILL_GROUP_WHEN_FORGE_ENDS,
            argument: NO_ARGUMENT.to_owned(),
        }
    }
}

pub(crate) fn contain(
    command: &mut std::process::Command,
    sandbox: Option<crate::Ruleset>,
    tree: &crate::EngineTree,
) -> Result<(), ContainError> {
    #[cfg(target_os = "linux")]
    let membership = tree.membership();
    let watch = Watch::of(tree);
    let forge_alive = lifeline()?;
    let enter = move || {
        #[cfg(target_os = "linux")]
        crate::linux::enter(membership.as_deref())?;
        #[cfg(target_os = "macos")]
        crate::macos::yield_disk()?;
        rustix::process::nice(ENGINE_NICENESS)?;
        lead_group_watched_by(forge_alive, &watch)?;
        if let Some(ruleset) = &sandbox {
            ruleset.restrict_self()?;
        }
        Ok(())
    };
    unsafe {
        command.pre_exec(enter);
    }
    Ok(())
}

fn last_errno() -> libc::c_int {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EIO)
}

fn lead_group_watched_by(forge_alive: RawFd, watch: &Watch) -> std::io::Result<()> {
    unsafe {
        if libc::setpgid(0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let group = libc::getpid().unsigned_abs();
        match libc::fork() {
            -1 => Err(std::io::Error::last_os_error()),
            0 => libc::_exit(start_watchers(forge_alive, watch, group)),
            intermediate => {
                let mut status: libc::c_int = 0;
                loop {
                    if libc::waitpid(intermediate, &raw mut status, 0) != -1 {
                        break;
                    }
                    let errno = last_errno();
                    if errno != libc::EINTR {
                        return Err(std::io::Error::from_raw_os_error(errno));
                    }
                }
                match (libc::WIFEXITED(status), libc::WEXITSTATUS(status)) {
                    (true, 0) => Ok(()),
                    (true, errno) => Err(std::io::Error::from_raw_os_error(errno)),
                    (false, _) => Err(std::io::Error::from_raw_os_error(libc::ECHILD)),
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
unsafe fn start_watchers(forge_alive: RawFd, watch: &Watch, _group: u32) -> libc::c_int {
    unsafe {
        match libc::fork() {
            -1 => last_errno(),
            0 => become_watcher(
                forge_alive,
                Output::Discarded,
                watch.script,
                watch.argument.as_ptr(),
            ),
            _ => 0,
        }
    }
}

#[cfg(not(target_os = "linux"))]
unsafe fn start_watchers(forge_alive: RawFd, watch: &Watch, group: u32) -> libc::c_int {
    let group = Decimal::of(group);
    unsafe {
        let mut asks: [RawFd; 2] = [0; 2];
        if libc::pipe(asks.as_mut_ptr()) != 0 {
            return match libc::fork() {
                -1 => last_errno(),
                0 => become_watcher(
                    forge_alive,
                    Output::Discarded,
                    watch.script,
                    watch.argument.as_ptr(),
                ),
                _ => 0,
            };
        }
        let [asked, ask] = asks;
        let asking = libc::fork();
        if asking == 0 {
            libc::close(asked);
            become_watcher(
                forge_alive,
                Output::Into(ask),
                ASK_FOR_A_SWEEP_WHEN_FORGE_ENDS,
                NO_ARGUMENT.as_ptr(),
            );
        }
        if asking == -1 {
            return last_errno();
        }
        let sweeping = libc::fork();
        if sweeping == 0 {
            libc::close(ask);
            libc::setpgid(0, 0);
            become_watcher(
                asked,
                Output::Discarded,
                SWEEP_GROUP_WHEN_ASKED,
                group.as_ptr(),
            );
        }
        if sweeping == -1 {
            return last_errno();
        }
        libc::close(asked);
        libc::close(ask);
        0
    }
}

#[derive(Clone, Copy)]
enum Output {
    Discarded,
    #[cfg(not(target_os = "linux"))]
    Into(RawFd),
}

unsafe fn become_watcher(
    input: RawFd,
    output: Output,
    script: &CStr,
    argument: *const std::ffi::c_char,
) -> ! {
    unsafe {
        let null = libc::open(DEV_NULL.as_ptr(), libc::O_RDWR);
        let out = match output {
            Output::Discarded => null,
            #[cfg(not(target_os = "linux"))]
            Output::Into(fd) => fd,
        };
        libc::dup2(input, 0);
        libc::dup2(out, 1);
        libc::dup2(null, 2);
        libc::chdir(ROOT.as_ptr());
        let argv = [
            SHELL.as_ptr(),
            SHELL_COMMAND.as_ptr(),
            script.as_ptr(),
            SCRIPT_NAME.as_ptr(),
            argument,
            std::ptr::null(),
        ];
        let environment = [std::ptr::null::<std::ffi::c_char>()];
        libc::execve(SHELL.as_ptr(), argv.as_ptr(), environment.as_ptr());
        libc::_exit(127)
    }
}

#[cfg(not(target_os = "linux"))]
struct Decimal([u8; 11]);

#[cfg(not(target_os = "linux"))]
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
