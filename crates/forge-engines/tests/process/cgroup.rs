use crate::{Launch, Turn, UserInput};
use artisan_api::{EngineConfig, EngineId, Permissions, ThreadId};
use std::path::Path;

fn founding_forge() -> bool {
    let in_forge_subgroup = std::fs::read_to_string("/proc/self/cgroup").is_ok_and(|membership| {
        membership
            .lines()
            .any(|line| line.starts_with("0::") && line.ends_with("/forge"))
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

fn turn(home: &Path, permissions: Permissions) -> Turn {
    let mut config = EngineConfig::new(EngineId::Claude);
    config.permissions = permissions;
    Turn {
        thread_id: ThreadId::from("cgroup"),
        engine: EngineId::Claude,
        binary: "/bin/sh".into(),
        home: home.to_path_buf(),
        cwd: home.to_path_buf(),
        config,
        session_id: None,
        input: UserInput::default(),
        launch: Launch::default(),
        trace_dir: None,
    }
}

async fn run(turn: &Turn, script: &str) -> Vec<String> {
    let mut process = super::spawn(turn, &["-c".to_owned(), script.to_owned()])
        .await
        .unwrap();
    let mut lines = Vec::new();
    while let Some(line) = tokio::time::timeout(test_support::wait::LIVENESS, process.next_line())
        .await
        .unwrap()
        .unwrap()
    {
        lines.push(line);
    }
    process.wait().await;
    assert_eq!(lines.last().map(String::as_str), Some("done"), "{lines:?}");
    lines
}

fn alive(process: &str) -> bool {
    std::fs::read_to_string(Path::new("/proc").join(process).join("stat"))
        .is_ok_and(|stat| !stat.contains(") Z "))
}

const DETACH: &str = "mkfifo ready; setsid sh -c 'echo up > ready; exec sleep 600' </dev/null >/dev/null 2>&1 & read _ < ready; echo $!; echo done";

#[tokio::test]
async fn a_process_an_auto_run_detached_ends_with_the_run() {
    if !founding_forge() {
        return;
    }
    let separation = forge_isolation::protect_forge();
    assert!(
        matches!(
            separation.first(),
            Some(forge_isolation::Outcome::Applied(_))
        ),
        "{separation:?}"
    );
    let home = tempfile::tempdir().unwrap();

    let lines = run(&turn(home.path(), Permissions::Auto), DETACH).await;

    assert!(
        !alive(&lines[0]),
        "a process the Auto run detached is still alive after its run ended"
    );
}

#[tokio::test]
async fn a_process_a_full_access_run_detached_outlives_the_run() {
    if !founding_forge() {
        return;
    }
    forge_isolation::protect_forge();
    let home = tempfile::tempdir().unwrap();

    let lines = run(&turn(home.path(), Permissions::Full), DETACH).await;

    let outlived = alive(&lines[0]);
    let killed = std::process::Command::new("kill")
        .args(["-KILL", &lines[0]])
        .status()
        .unwrap();
    assert!(
        outlived && killed.success(),
        "a dev server a Full-access run started with setsid died when the run ended"
    );
}
