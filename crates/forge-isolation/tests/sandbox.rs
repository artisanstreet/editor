#![cfg(target_os = "linux")]

use forge_isolation::{Landlock, Support, contain, support};
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
    contain(&mut command, sandbox);
    let output = command.output().expect("the shell runs");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_sandboxed_child_writes_inside_its_folders_and_nowhere_else() {
    let Some(landlock) = landlock() else {
        return;
    };
    let allowed = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("existing"), "kept").unwrap();
    let ruleset = landlock
        .writes_only_to(&[allowed.path().to_path_buf()])
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
