#![cfg(target_os = "linux")]

use forge_isolation::linux::{
    Controller, RunOwner, Standing, delegated, engine_memory_ceiling, run_owner, standing,
    start_time, total_memory, unified_membership,
};
use std::path::Path;
use std::process::Command;

const GIB: u64 = 1 << 30;

#[test]
fn membership_comes_from_the_unified_hierarchy_line() {
    let hybrid = "12:memory:/user.slice\n1:name=systemd:/user.slice/session-3.scope\n0::/user.slice/user-1001.slice/user@1001.service/app.slice/artisan-forge.service\n";
    assert_eq!(
        unified_membership(hybrid),
        Some("/user.slice/user-1001.slice/user@1001.service/app.slice/artisan-forge.service"),
        "the 0:: line names the cgroup v2 path"
    );
    assert_eq!(
        unified_membership("12:memory:/user.slice\n1:name=systemd:/init.scope\n"),
        None,
        "a pure v1 system has no unified membership"
    );
}

#[test]
fn only_the_process_systemd_placed_in_the_subgroup_founds_the_split() {
    let service = Path::new("/sys/fs/cgroup/app.slice/artisan-forge.service");
    assert_eq!(
        standing(&service.join("forge"), Some(479), 479, false),
        Standing::Founder,
        "the gateway systemd started in the forge subgroup creates the engines group"
    );
    assert_eq!(
        standing(&service.join("forge"), Some(479), 479, true),
        Standing::Founder,
        "a restarted gateway reapplies limits over engines left from the last run"
    );
    assert_eq!(
        standing(&service.join("forge"), Some(479), 566, true),
        Standing::Member,
        "a backend the gateway spawned joins the existing split"
    );
    assert_eq!(
        standing(service, Some(479), 479, false),
        Standing::Outsider,
        "a main process in the service root would block the next start if it split"
    );
    assert_eq!(
        standing(&service.join("engines"), Some(479), 9001, true),
        Standing::Outsider,
        "a forge an agent runs from an engine shell must not touch the layout"
    );
    assert_eq!(
        standing(
            Path::new("/sys/fs/cgroup/user.slice/session-3.scope"),
            None,
            41,
            false
        ),
        Standing::Outsider,
        "a forge run by hand in a terminal leaves the session alone"
    );
}

#[test]
fn controllers_parse_the_kernel_listing() {
    assert_eq!(
        delegated("cpuset memory pids"),
        vec![Controller::Memory],
        "cpuset is not the cpu controller"
    );
    assert_eq!(
        delegated("cpuset cpu io memory hugetlb pids rdma misc"),
        vec![Controller::Memory, Controller::Cpu, Controller::Io],
        "every wanted controller present is reported"
    );
}

#[test]
fn engine_ceiling_leaves_a_bounded_reserve() {
    assert_eq!(
        total_memory("MemTotal:       16377764 kB\nMemFree:  100 kB\n"),
        Ok(16_377_764 * 1024),
        "MemTotal is reported in kibibytes"
    );
    assert_eq!(
        engine_memory_ceiling(16 * GIB),
        16 * GIB - 16 * GIB / 5,
        "a 16 GiB machine keeps a fifth back"
    );
    assert_eq!(
        engine_memory_ceiling(8 * GIB),
        6 * GIB,
        "the reserve never drops below 2 GiB"
    );
    assert_eq!(
        engine_memory_ceiling(128 * GIB),
        124 * GIB,
        "the reserve never exceeds 4 GiB"
    );
    assert_eq!(
        engine_memory_ceiling(2 * GIB),
        GIB,
        "a tiny machine still gives engines half"
    );
}

#[test]
fn an_engine_that_cannot_join_the_engines_group_is_reported_with_its_pid_and_cause() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("engines").join("cgroup.procs");
    let error = forge_isolation::linux::attach(Some(&missing), dir.path(), 4242)
        .expect_err("a missing cgroup cannot take the engine");
    let text = format!("{error}: {}", std::error::Error::source(&error).unwrap());
    assert!(text.contains("4242"), "{text}");
    assert!(text.contains(&missing.display().to_string()), "{text}");
    assert!(text.contains("No such file"), "{text}");
}

#[test]
fn attaching_moves_the_engine_and_raises_its_oom_score() {
    let dir = tempfile::tempdir().unwrap();
    let procs = dir.path().join("cgroup.procs");
    std::fs::write(&procs, b"").unwrap();
    let process = dir.path().join("4242");
    std::fs::create_dir(&process).unwrap();
    std::fs::write(process.join("oom_score_adj"), b"0").unwrap();
    forge_isolation::linux::attach(Some(&procs), dir.path(), 4242).unwrap();
    assert_eq!(std::fs::read_to_string(&procs).unwrap(), "4242");
    assert_eq!(
        std::fs::read_to_string(process.join("oom_score_adj")).unwrap(),
        "500"
    );
}

#[test]
fn an_engine_that_already_exited_is_not_an_isolation_failure() {
    let dir = tempfile::tempdir().unwrap();
    forge_isolation::linux::attach(None, dir.path(), 4242).unwrap();
}

#[test]
fn a_run_cgroup_names_the_forge_that_made_it_down_to_its_start_time() {
    assert_eq!(
        run_owner("run-4182-998877-3"),
        Some(RunOwner {
            process: 4182,
            started: Some(998_877),
        })
    );
    assert_eq!(
        run_owner("run-4182-3"),
        Some(RunOwner {
            process: 4182,
            started: None,
        }),
        "a run cgroup named by an older Forge is owned by its process alone"
    );
    for foreign in [
        "forge",
        "engines",
        "run-",
        "run-x-1",
        "run-1-2-3-4",
        "run-1-2-x",
    ] {
        assert_eq!(run_owner(foreign), None, "{foreign}");
    }
}

#[test]
fn a_process_start_time_survives_a_name_with_spaces_and_parentheses() {
    let stat = "4182 (forge (worker) 2) S 1 4182 4182 0 -1 4194560 2385 0 0 0 9 4 0 0 20 0 7 0 998877 1234 5";
    assert_eq!(start_time(stat), Some(998_877));
    assert_eq!(start_time("4182 (forge) S 1"), None);
}

const MEMBER: &str = "FORGE_ISOLATION_STAND_IN_FOR_A_BACKEND";
const MEMBER_TEST: &str = "a_backend_started_after_another_died_ends_the_run_cgroups_it_left";

fn founding_forge() -> bool {
    let in_forge_subgroup = std::fs::read_to_string("/proc/self/cgroup").is_ok_and(|membership| {
        unified_membership(&membership).is_some_and(|own| own.ends_with("/forge"))
    });
    let started_by_systemd =
        std::env::var("SYSTEMD_EXEC_PID").is_ok_and(|pid| pid == std::process::id().to_string());
    if in_forge_subgroup && started_by_systemd {
        return true;
    }
    test_support::delegation_missing(format_args!(
        "run this test as the main process of a systemd unit with Delegate=yes and DelegateSubgroup=forge"
    ));
    false
}

fn engines_folder() -> std::path::PathBuf {
    let membership = std::fs::read_to_string("/proc/self/cgroup").expect("this process's cgroup");
    let own = unified_membership(&membership).expect("a unified cgroup");
    Path::new("/sys/fs/cgroup")
        .join(own.trim_start_matches('/'))
        .with_file_name("engines")
}

fn left_by_a_dead_forge(engines: &Path) -> (std::path::PathBuf, std::process::Child) {
    let mut gone = Command::new("true").spawn().expect("a short-lived process");
    gone.wait().expect("it exits");
    let leftover = engines.join(format!("run-{}-1-0", gone.id()));
    std::fs::create_dir(&leftover).expect("a run cgroup the dead Forge left");
    let escaped = Command::new("sleep")
        .arg("600")
        .spawn()
        .expect("a process that escaped its run");
    std::fs::write(leftover.join("cgroup.procs"), escaped.id().to_string())
        .expect("the escaped process sits in the leftover cgroup");
    (leftover, escaped)
}

#[expect(
    clippy::print_stdout,
    reason = "the stand-in backend tells the test what isolation it applied"
)]
fn stand_in_for_a_backend() {
    println!("{:?}", forge_isolation::protect_forge());
}

#[test]
fn a_backend_started_after_another_died_ends_the_run_cgroups_it_left() {
    if std::env::var_os(MEMBER).is_some() {
        stand_in_for_a_backend();
        return;
    }
    if !founding_forge() {
        return;
    }
    let founded = forge_isolation::protect_forge();
    assert!(
        matches!(founded.first(), Some(forge_isolation::Outcome::Applied(_))),
        "{founded:?}"
    );
    let (leftover, mut escaped) = left_by_a_dead_forge(&engines_folder());

    let backend = Command::new(std::env::current_exe().expect("the test binary"))
        .args([MEMBER_TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(MEMBER, "1")
        .output()
        .expect("the stand-in backend runs");
    let applied = String::from_utf8_lossy(&backend.stdout).into_owned();

    let status = test_support::wait::wait_for(
        "the escaped process to be killed",
        test_support::wait::LIVENESS,
        || {
            escaped
                .try_wait()
                .expect("the escaped process can be polled")
        },
    );
    assert!(
        !status.success() && !leftover.exists(),
        "a backend left a dead backend's run cgroup populated: {applied}"
    );
}

#[test]
fn an_engine_whose_watcher_cannot_start_does_not_run() {
    if !founding_forge() {
        return;
    }
    forge_isolation::protect_forge();
    let engines = engines_folder();
    let service = engines.parent().expect("the service cgroup").to_path_buf();
    let delegated = std::fs::read_to_string(service.join("cgroup.controllers")).unwrap_or_default();
    if !delegated.split_whitespace().any(|name| name == "pids") {
        test_support::skipped(format_args!(
            "the pids controller isn't delegated to this unit, so a fork can't be made to fail"
        ));
        return;
    }
    std::fs::write(service.join("cgroup.subtree_control"), "+pids").expect("enable pids");
    let marker = tempfile::tempdir().expect("a folder for the engine's mark");
    let ran = marker.path().join("ran");
    std::fs::write(engines.join("pids.max"), "2").expect("room for the engine and one fork");

    let tree = forge_isolation::EngineTree::new(forge_isolation::Containment::OwnCgroup)
        .expect("an engine tree");
    let mut engine = Command::new("/bin/sh");
    engine.args(["-c", "touch \"$1\"", "sh"]).arg(&ran);
    forge_isolation::contain(&mut engine, &tree).expect("the engine is contained");
    let spawned = engine.spawn();
    std::fs::write(engines.join("pids.max"), "max").expect("lift the limit");
    let spawned = spawned.map(|mut child| child.wait().expect("the engine exits"));
    tree.end().expect("the engine's tree ends");

    assert!(
        spawned.is_err() && !ran.exists(),
        "an engine ran without the watcher that kills it when Forge dies: {spawned:?}"
    );
}
