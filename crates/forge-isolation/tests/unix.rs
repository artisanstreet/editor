#![cfg(target_os = "linux")]

use forge_isolation::{EngineTree, contain};
use rustix::event::{PollFd, PollFlags, poll};
use rustix::process::{Pid, PidfdFlags, pidfd_open};
use std::collections::BTreeSet;
use std::io::BufRead;
use std::process::{Command, Stdio};
use std::time::Duration;

const STAND_IN: &str = "FORGE_ISOLATION_STAND_IN_FOR_FORGE";
const TEST: &str = "finished_launches_leave_no_watcher_behind_while_forge_runs";
const LAUNCHES: usize = 8;
const WATCHERS: &str = "watchers:";

fn pipes_of(process: &str) -> BTreeSet<String> {
    std::fs::read_dir(format!("/proc/{process}/fd"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .map(|target| target.to_string_lossy().into_owned())
        .filter(|target| target.starts_with("pipe:"))
        .collect()
}

fn watchers_of_this_process() -> Vec<String> {
    let own = pipes_of("self");
    let me = std::process::id().to_string();
    std::fs::read_dir("/proc")
        .expect("a /proc to list")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|pid| pid.chars().all(|digit| digit.is_ascii_digit()) && *pid != me)
        .filter(|pid| {
            std::fs::read_link(format!("/proc/{pid}/fd/0"))
                .is_ok_and(|stdin| own.contains(&stdin.to_string_lossy().into_owned()))
        })
        .collect()
}

#[expect(
    clippy::print_stdout,
    reason = "the stand-in Forge tells the test which watchers it left"
)]
fn stand_in_for_forge() -> ! {
    for _ in 0..LAUNCHES {
        let tree =
            EngineTree::new(forge_isolation::Containment::OwnCgroup).expect("an engine tree");
        let mut engine = Command::new("/bin/sh");
        engine
            .args(["-c", "exit 0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        contain(&mut engine, &tree).expect("the engine is contained");
        let mut child = engine.spawn().expect("the engine starts");
        let group = child.id();
        child.wait().expect("the engine exits");
        let pgid = Pid::from_raw(i32::try_from(group).expect("a pid fits an i32"))
            .expect("a running engine has a pid");
        match rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
            Err(error) => panic!("Forge couldn't end engine group {group}: {error}"),
        }
        tree.end().expect("the engine's tree ends");
    }
    println!("{WATCHERS} {}", watchers_of_this_process().join(" "));
    loop {
        std::thread::park();
    }
}

#[test]
fn finished_launches_leave_no_watcher_behind_while_forge_runs() {
    if std::env::var_os(STAND_IN).is_some() {
        stand_in_for_forge();
    }
    let mut forge = Command::new(std::env::current_exe().expect("the test binary"))
        .args([TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(STAND_IN, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the stand-in Forge starts");
    let announced = std::io::BufReader::new(forge.stdout.take().expect("piped stdout"))
        .lines()
        .map_while(Result::ok)
        .find_map(|line| line.split_once(WATCHERS).map(|(_, pids)| pids.to_owned()))
        .expect("the stand-in Forge lists its watchers");
    let lingering: Vec<String> = announced
        .split_whitespace()
        .filter(|pid| !exits_within(pid, Duration::from_secs(5)))
        .map(str::to_owned)
        .collect();
    forge.kill().expect("the stand-in Forge stops");
    forge.wait().expect("the stand-in Forge is reaped");
    assert_eq!(
        lingering,
        Vec::<String>::new(),
        "watchers of {LAUNCHES} finished launches are still alive while Forge runs"
    );
}

fn exits_within(pid: &str, within: Duration) -> bool {
    let pid = pid
        .parse::<i32>()
        .ok()
        .and_then(Pid::from_raw)
        .expect("a listed watcher pid");
    let process = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(process) => process,
        Err(rustix::io::Errno::SRCH) => return true,
        Err(error) => panic!("watch watcher {pid:?}: {error}"),
    };
    let timeout = rustix::time::Timespec::try_from(within).expect("a short wait");
    let mut watched = [PollFd::new(&process, PollFlags::IN)];
    poll(&mut watched, Some(&timeout)).expect("waiting on a watcher") > 0
}
