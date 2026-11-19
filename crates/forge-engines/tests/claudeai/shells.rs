use super::*;
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use test_support::wait::eventually;

#[cfg(unix)]
const PATIENCE: Duration = Duration::from_secs(10);

#[cfg(unix)]
async fn settled(
    shells: &Shells,
    id: &str,
    what: &str,
    until: impl Fn(&ShellState) -> bool + Sync,
) {
    let shell = shells.find(id).unwrap();
    eventually(what, PATIENCE, || std::future::ready(until(&shell.lock()))).await;
}

#[cfg(unix)]
fn printed(state: &ShellState, text: &str) -> bool {
    state
        .unread
        .chunks
        .iter()
        .map(String::as_str)
        .collect::<String>()
        .contains(text)
}

#[cfg(unix)]
fn exited(state: &ShellState) -> bool {
    state.ended.is_some()
}

#[cfg(unix)]
#[tokio::test]
async fn a_background_command_runs_and_reports_its_output_and_exit() {
    let work = tempfile::tempdir().unwrap();
    let shells = Shells::default();
    let id = shells
        .start(
            "echo hello; sleep 0.2; echo done; exit 3",
            work.path(),
            &crate::sandbox::Sandbox::Open,
        )
        .unwrap();
    settled(&shells, &id, "the first line", |state| {
        printed(state, "hello")
    })
    .await;
    let first = shells.read(&id).unwrap();
    assert!(first.contains("hello"), "{first}");
    settled(&shells, &id, "the exit", |state| {
        exited(state) && printed(state, "done")
    })
    .await;
    let rest = shells.read(&id).unwrap();
    assert!(rest.contains("done"), "{rest}");
    assert!(rest.contains("[exit code 3]"), "{rest}");
    assert!(!rest.contains("hello"), "output is only shown once: {rest}");
}

#[cfg(unix)]
#[tokio::test]
async fn killing_a_shell_stops_it_and_says_so() {
    let work = tempfile::tempdir().unwrap();
    let shells = Shells::default();
    let id = shells
        .start(
            "echo up; sleep 30",
            work.path(),
            &crate::sandbox::Sandbox::Open,
        )
        .unwrap();
    settled(&shells, &id, "the shell to start", |state| {
        printed(state, "up")
    })
    .await;
    assert!(shells.kill(&id).unwrap().contains("stopped"));
    settled(&shells, &id, "the shell to stop", exited).await;
    let read = shells.read(&id).unwrap();
    assert!(read.contains("[stopped]"), "{read}");
}

#[cfg(unix)]
#[tokio::test]
async fn an_id_from_another_turn_never_reads_this_turns_shell() {
    let work = tempfile::tempdir().unwrap();
    let earlier = Shells::default();
    let old = earlier
        .start("echo old", work.path(), &crate::sandbox::Sandbox::Open)
        .unwrap();
    drop(earlier);
    let shells = Shells::default();
    let id = shells
        .start("echo new", work.path(), &crate::sandbox::Sandbox::Open)
        .unwrap();
    assert_ne!(id, old, "ids are not reused across turns");
    let error = shells.read(&old).unwrap_err();
    assert!(error.contains("end with the turn"), "{error}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_shell_that_leaves_a_command_running_still_reports_its_exit() {
    let work = tempfile::tempdir().unwrap();
    let shells = Shells::default();
    let id = shells
        .start(
            "sleep 30 & echo started",
            work.path(),
            &crate::sandbox::Sandbox::Open,
        )
        .unwrap();
    settled(&shells, &id, "the launcher to exit", |state| {
        exited(state) && printed(state, "started")
    })
    .await;
    let read = shells.read(&id).unwrap();
    assert!(read.contains("started"), "{read}");
    assert!(read.ends_with("[exit code 0]"), "{read}");
}

#[cfg(unix)]
#[tokio::test]
async fn the_newest_output_is_kept_past_the_bound_and_the_loss_said_once() {
    let work = tempfile::tempdir().unwrap();
    let shells = Shells::default();
    let id = shells
        .start(
            "head -c 300000 /dev/zero | tr '\\0' a; echo; echo tail-marker",
            work.path(),
            &crate::sandbox::Sandbox::Open,
        )
        .unwrap();
    settled(&shells, &id, "the whole output", |state| {
        exited(state) && printed(state, "tail-marker")
    })
    .await;
    let read = shells.read(&id).unwrap();
    assert!(
        read.contains("tail-marker\n[exit code 0]"),
        "{}",
        &read[read.len() - 100..]
    );
    assert!(
        read.starts_with(&format!(
            "[{} bytes of output were dropped before they were read",
            300_013 - MAX_OUTPUT_BYTES
        )),
        "{}",
        &read[..200]
    );
    let again = shells.read(&id).unwrap();
    assert_eq!(again, "[exit code 0]", "the loss is reported once");
}

#[cfg(unix)]
#[tokio::test]
async fn a_turn_runs_a_bounded_number_of_shells_and_forgets_the_oldest_finished() {
    let work = tempfile::tempdir().unwrap();
    let shells = Shells::default();
    let running: Vec<String> = (0..MAX_RUNNING_SHELLS)
        .map(|_| {
            shells
                .start("sleep 30", work.path(), &crate::sandbox::Sandbox::Open)
                .unwrap()
        })
        .collect();
    let refused = shells
        .start("sleep 30", work.path(), &crate::sandbox::Sandbox::Open)
        .unwrap_err();
    assert!(refused.contains("KillShell"), "{refused}");
    shells.kill(&running[0]).unwrap();
    let replacement = shells
        .start("sleep 30", work.path(), &crate::sandbox::Sandbox::Open)
        .unwrap();
    for id in running.iter().skip(1).chain([&replacement]) {
        shells.kill(id).unwrap();
    }
    let mut finished = Vec::new();
    for _ in 0..200 {
        let id = shells
            .start("sleep 30", work.path(), &crate::sandbox::Sandbox::Open)
            .unwrap();
        shells.kill(&id).unwrap();
        finished.push(id);
    }
    assert!(
        shells.shells.lock().unwrap().len() <= MAX_KEPT_SHELLS,
        "{} shells kept",
        shells.shells.lock().unwrap().len()
    );
    assert!(shells.read(&finished[199]).is_ok());
    assert!(shells.read(&finished[0]).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn stopping_the_run_ends_what_a_shell_started() {
    let work = tempfile::tempdir().unwrap();
    let shells = Shells::default();
    let id = shells
        .start(
            "sleep 30 & echo $!; wait",
            work.path(),
            &crate::sandbox::Sandbox::Open,
        )
        .unwrap();
    settled(&shells, &id, "the grandchild's pid", |state| {
        printed(state, "\n")
    })
    .await;
    let pid = shells
        .read(&id)
        .unwrap()
        .trim_end_matches("[still running]")
        .trim()
        .to_owned();
    assert!(pid.parse::<u32>().is_ok(), "the grandchild's pid: {pid:?}");
    let alive = |pid: &str| {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    assert!(alive(&pid));
    drop(shells);
    eventually("the grandchild to end with the run", PATIENCE, || {
        std::future::ready(!alive(&pid))
    })
    .await;
}

#[test]
fn a_character_split_across_reads_stays_whole() {
    let mut decoder = Decoder::new(None);
    let bytes = "café ✓".as_bytes();
    let (head, tail) = bytes.split_at(bytes.len() - 2);
    let mut text = decoder.push(&bytes[..4], false);
    text.push_str(&decoder.push(&head[4..], false));
    text.push_str(&decoder.push(tail, false));
    text.push_str(&decoder.push(&[], true));
    assert_eq!(text, "café ✓");
}

#[test]
fn output_that_is_not_utf8_is_read_in_the_legacy_code_page() {
    let mut decoder = Decoder::new(Some(encoding_rs::IBM866));
    let mut text = decoder.push(b"ok ", false);
    text.push_str(&decoder.push(b"\x8f\xe0\xa8\xa2\xa5\xe2", true));
    assert_eq!(text, "ok Привет");
}
