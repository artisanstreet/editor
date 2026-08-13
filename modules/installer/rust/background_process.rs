use std::{
    ffi::OsStr,
    process::{Command, Stdio},
};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// Builds an internal child process that must never allocate a Windows console.
pub fn background_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
}

/// Builds a self-owned cleanup helper with no inherited terminal handles.
pub fn detached_background_command(program: impl AsRef<OsStr>) -> Command {
    let mut command = background_command(program);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }

    command
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn every_background_process_uses_the_no_console_creation_flag() {
        assert_ne!(super::CREATE_NO_WINDOW, 0);
        assert_eq!(super::CREATE_NO_WINDOW, 0x0800_0000);
    }
}
