#![cfg(target_os = "linux")]

use forge_isolation::{Coverage, EngineTree, Landlock, Reach, Support, contain, support};
use std::path::Path;
use std::process::Command;

const SCRIPT: &str = r#"
echo inside > "$1/made"
mkdir "$1/sub"
mv "$1/made" "$1/sub/moved"
echo quiet > /dev/null || echo "null refused" >&2
echo escaped > "$2/escaped"
echo clobbered > "$2/existing"
rm -f "$2/existing"
mkdir "$2/folder"
exit 0
"#;

#[expect(
    clippy::print_stderr,
    reason = "a test the kernel can't run says so instead of passing silently"
)]
fn landlock() -> Option<Landlock> {
    match support() {
        Support::Enforced(landlock) => Some(landlock),
        Support::Unavailable(reason) => {
            eprintln!(
                "SKIPPED: this kernel can't enforce the sandbox ({reason:?}), so a sandboxed child can't be exercised here"
            );
            None
        }
    }
}

fn run(allowed: &Path, outside: &Path, sandbox: Option<&forge_isolation::Ruleset>) -> String {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", SCRIPT, "sh"]).arg(allowed).arg(outside);
    contain(
        &mut command,
        sandbox,
        &EngineTree::new().expect("an engine tree"),
    );
    let output = command.output().expect("the shell runs");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_sandboxed_child_writes_inside_its_folders_and_nowhere_else() {
    let Some(landlock) = landlock() else {
        return;
    };
    let own = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("existing"), "kept").unwrap();
    let ruleset = landlock
        .writes_only_to(&[allowed.path().to_path_buf()], own.path(), &[])
        .unwrap();
    let stderr = run(allowed.path(), outside.path(), Some(&ruleset));
    assert_eq!(
        std::fs::read_to_string(allowed.path().join("sub").join("moved")).unwrap(),
        "inside\n",
        "the child creates, renames and moves files in its own folder: {stderr}"
    );
    assert!(
        !stderr.contains("null refused"),
        "the child still writes to /dev/null: {stderr}"
    );
    assert!(
        !outside.path().join("escaped").exists(),
        "the child created a file outside its folders"
    );
    assert!(
        !outside.path().join("folder").exists(),
        "the child created a folder outside its folders"
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("existing")).unwrap(),
        "kept",
        "the child overwrote or removed a file outside its folders"
    );
    assert!(
        stderr.contains("Permission denied"),
        "the kernel refused the writes outside: {stderr}"
    );
}

#[test]
fn each_landlock_abi_reports_what_it_leaves_open() {
    let open = Reach::Open;
    let blocked = Reach::Blocked;
    let expected = [
        (1, [open, open, open, open]),
        (2, [open, open, open, open]),
        (3, [blocked, open, open, open]),
        (4, [blocked, open, open, open]),
        (5, [blocked, open, open, open]),
        (6, [blocked, open, blocked, blocked]),
        (7, [blocked, open, blocked, blocked]),
        (8, [blocked, open, blocked, blocked]),
        (9, [blocked, blocked, blocked, blocked]),
        (10, [blocked, blocked, blocked, blocked]),
    ];
    for (abi, [truncation, socket_files, abstract_sockets, signals]) in expected {
        assert_eq!(
            Coverage::of(abi),
            Coverage {
                truncation,
                socket_files,
                abstract_sockets,
                signals,
            },
            "Landlock ABI {abi}"
        );
    }
}

#[expect(
    clippy::print_stderr,
    reason = "a check the kernel can't make says so instead of passing silently"
)]
#[test]
fn a_sandboxed_child_cannot_truncate_files_outside_when_the_kernel_says_so() {
    let Some(landlock) = landlock() else {
        return;
    };
    let own = tempfile::tempdir().unwrap();
    if landlock.coverage().truncation == Reach::Open {
        eprintln!(
            "SKIPPED: Landlock ABI {} can't block truncation, and Forge reports that",
            landlock.abi()
        );
        return;
    }
    let allowed = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("kept");
    std::fs::write(&file, "kept").unwrap();
    let ruleset = landlock
        .writes_only_to(&[allowed.path().to_path_buf()], own.path(), &[])
        .unwrap();
    let mut command = Command::new("truncate");
    command.args(["-s", "0"]).arg(&file);
    contain(&mut command, Some(&ruleset), &EngineTree::new().unwrap());
    let output = command.output().expect("truncate runs");
    assert!(
        !output.status.success(),
        "truncate succeeded outside the sandbox"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "kept",
        "a sandboxed child emptied a file outside its folders"
    );
}

#[expect(
    clippy::print_stderr,
    reason = "a check the kernel can't make says so instead of passing silently"
)]
#[test]
fn a_sandboxed_child_signals_its_own_children_but_not_processes_outside() {
    let Some(landlock) = landlock() else {
        return;
    };
    let own = tempfile::tempdir().unwrap();
    if landlock.coverage().signals == Reach::Open {
        eprintln!(
            "SKIPPED: Landlock ABI {} can't scope signals, and Forge reports that",
            landlock.abi()
        );
        return;
    }
    let mut bystander = Command::new("sleep").arg("600").spawn().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let ruleset = landlock
        .writes_only_to(&[allowed.path().to_path_buf()], own.path(), &[])
        .unwrap();
    let mut command = Command::new("/bin/sh");
    command.args([
        "-c",
        r#"sleep 600 & own=$!; kill "$own" && echo "own child signalled"; kill "$1" || echo "bystander refused""#,
        "sh",
        &bystander.id().to_string(),
    ]);
    contain(&mut command, Some(&ruleset), &EngineTree::new().unwrap());
    let output = command.output().expect("the shell runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let still_running = bystander.try_wait().unwrap().is_none();
    bystander.kill().unwrap();
    bystander.wait().unwrap();
    assert!(
        stdout.contains("own child signalled"),
        "a sandboxed shell couldn't stop its own background job: {stdout}"
    );
    assert!(
        still_running && stdout.contains("bystander refused"),
        "a sandboxed child killed a process outside its sandbox: {stdout}"
    );
}

#[test]
fn a_link_inside_a_writable_folder_does_not_make_its_target_writable() {
    let Some(landlock) = landlock() else {
        return;
    };
    let own = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("existing"), "kept").unwrap();
    let planted = allowed.path().join("config");
    std::os::unix::fs::symlink(outside.path(), &planted).unwrap();
    let ruleset = landlock
        .writes_only_to(&[allowed.path().to_path_buf(), planted], own.path(), &[])
        .unwrap();
    let stderr = run(allowed.path(), outside.path(), Some(&ruleset));
    assert!(
        !outside.path().join("escaped").exists(),
        "a link the sandbox could plant made its target writable: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("existing")).unwrap(),
        "kept",
        "a link the sandbox could plant let it overwrite a file outside: {stderr}"
    );
}

#[test]
fn a_folder_reached_through_a_link_the_sandbox_could_plant_is_refused() {
    let Some(landlock) = landlock() else {
        return;
    };
    let own = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let allowed = base.path().join("allowed");
    let outside = base.path().join("outside");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, allowed.join("hop")).unwrap();
    let project = base.path().join("project");
    std::os::unix::fs::symlink(allowed.join("hop"), &project).unwrap();
    let refused = landlock.writes_only_to(&[allowed.clone(), project.clone()], own.path(), &[]);
    match refused {
        Err(forge_isolation::RulesetError::LinkedThroughWritable { path, link_folder }) => {
            assert_eq!((path, link_folder), (project, allowed));
        }
        other => panic!(
            "a folder resolved through a link inside a writable folder was granted: {other:?}"
        ),
    }
}
