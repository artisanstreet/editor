#![cfg(target_os = "linux")]

use forge_isolation::linux::{
    Controller, RunOwner, Standing, delegated, engine_memory_ceiling, run_owner, standing,
    start_time, total_memory, unified_membership,
};
use std::path::Path;

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
