use crate::process::spawn;
use crate::{Launch, Turn, UserInput};
use artisan_api::{EngineConfig, EngineId, Permissions, ThreadId};
use forge_isolation::Support;
use std::path::Path;
use std::time::Duration;

const SCRIPT: &str = r#"
echo inside > made
echo state > "$2/state"
echo scratch > "$TMPDIR/scratch" && echo "temporary $TMPDIR"
echo escaped > "$1/escaped"
echo done
"#;

fn turn(permissions: Permissions, project: &Path, home: &Path) -> Turn {
    let mut config = EngineConfig::new(EngineId::Codex);
    config.permissions = permissions;
    Turn {
        thread_id: ThreadId::from("sandboxed"),
        engine: EngineId::Codex,
        binary: "/bin/sh".into(),
        home: home.into(),
        cwd: project.into(),
        config,
        session_id: None,
        input: UserInput::default(),
        launch: Launch::default(),
        trace_dir: None,
    }
}

async fn run(turn: &Turn, outside: &Path) -> Vec<String> {
    let mut process = spawn(
        turn,
        &[
            "-c".to_owned(),
            SCRIPT.to_owned(),
            "sh".to_owned(),
            outside.display().to_string(),
            turn.home.display().to_string(),
        ],
    )
    .unwrap();
    let mut lines = Vec::new();
    while let Some(line) = tokio::time::timeout(Duration::from_secs(10), process.next_line())
        .await
        .unwrap()
        .unwrap()
    {
        lines.push(line);
    }
    process.wait().await;
    lines
}

#[tokio::test]
async fn an_auto_engine_writes_its_project_home_and_run_folder_but_nothing_else() {
    if let Support::Unavailable(reason) = forge_isolation::support() {
        eprintln!("SKIPPED: this kernel can't enforce the sandbox ({reason:?})");
        return;
    }
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let lines = run(
        &turn(Permissions::Auto, project.path(), home.path()),
        outside.path(),
    )
    .await;
    assert_eq!(lines.last().map(String::as_str), Some("done"), "{lines:?}");
    assert!(project.path().join("made").exists(), "{lines:?}");
    assert!(home.path().join("state").exists(), "{lines:?}");
    assert!(
        !outside.path().join("escaped").exists(),
        "an Auto engine wrote outside its folders"
    );
    let temporary = lines
        .iter()
        .find_map(|line| line.strip_prefix("temporary "))
        .unwrap_or_else(|| panic!("the engine wrote to its run folder: {lines:?}"));
    assert!(
        !Path::new(temporary).exists(),
        "the run folder {temporary} is removed when the engine ends"
    );
}

#[tokio::test]
async fn a_full_access_engine_writes_wherever_the_user_can() {
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let lines = run(
        &turn(Permissions::Full, project.path(), home.path()),
        outside.path(),
    )
    .await;
    assert_eq!(lines.last().map(String::as_str), Some("done"), "{lines:?}");
    assert!(
        outside.path().join("escaped").exists(),
        "Full access is not sandboxed"
    );
}
