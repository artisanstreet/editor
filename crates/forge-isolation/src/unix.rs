use std::os::unix::process::CommandExt;

const ENGINE_NICENESS: i32 = 5;

pub fn contain(command: &mut std::process::Command) {
    #[cfg(target_os = "linux")]
    let engines = crate::linux::engines_membership();
    let enter = move || {
        #[cfg(target_os = "linux")]
        crate::linux::enter(engines);
        #[cfg(target_os = "macos")]
        crate::macos::yield_disk();
        rustix::process::nice(ENGINE_NICENESS).ok();
        Ok(())
    };
    unsafe {
        command.pre_exec(enter);
    }
}
