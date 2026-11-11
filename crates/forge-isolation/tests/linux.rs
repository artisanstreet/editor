#![cfg(target_os = "linux")]

use forge_isolation::linux::{
    Controller, Standing, delegated, engine_memory_ceiling, standing, total_memory,
    unified_membership,
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
        Some(16_377_764 * 1024),
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
