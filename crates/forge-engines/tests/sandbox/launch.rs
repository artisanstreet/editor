use crate::process::spawn;
use crate::{Launch, Turn, UserInput};
use artisan_api::{EngineConfig, EngineId, Permissions, ThreadId};
use forge_isolation::Support;
use std::path::{Path, PathBuf};
use std::time::Duration;

const SCRIPT: &str = r#"
echo inside > made
echo state > "$2/state"
echo scratch > "$TMPDIR/scratch" && echo "temporary $TMPDIR"
echo cached > "$3/.cache/tool/entry"
echo crate > "$3/.cargo/registry/entry"
echo escaped > "$1/escaped"
echo hijacked >> "$3/.bashrc"
echo configured > "$3/.config/tool"
echo done
"#;

struct Folders {
    _base: tempfile::TempDir,
    project: PathBuf,
    engine_home: PathBuf,
    user_home: PathBuf,
    outside: PathBuf,
}

fn folders() -> Folders {
    let beside_the_tests = std::env::current_exe()
        .expect("the test binary has a path")
        .parent()
        .expect("the test binary is in a folder")
        .to_path_buf();
    let base = tempfile::tempdir_in(beside_the_tests).expect("a folder outside /tmp");
    let folder = |name: &str| {
        let path = base.path().join(name);
        std::fs::create_dir_all(&path).expect("a test folder");
        path
    };
    let user_home = folder("user");
    std::fs::create_dir_all(user_home.join(".cache").join("tool")).expect("a cache");
    std::fs::create_dir_all(user_home.join(".cargo")).expect("a cargo home");
    std::fs::create_dir_all(user_home.join(".config")).expect("a config folder");
    std::fs::write(user_home.join(".bashrc"), "kept\n").expect("a shell rc file");
    Folders {
        project: folder("project"),
        engine_home: folder("engine"),
        outside: folder("outside"),
        user_home,
        _base: base,
    }
}

fn turn(permissions: Permissions, folders: &Folders) -> Turn {
    let mut config = EngineConfig::new(EngineId::Codex);
    config.permissions = permissions;
    Turn {
        thread_id: ThreadId::from("sandboxed"),
        engine: EngineId::Codex,
        binary: "/bin/sh".into(),
        home: folders.engine_home.clone(),
        cwd: folders.project.clone(),
        config,
        session_id: None,
        input: UserInput::default(),
        launch: Launch {
            env: vec![
                ("HOME".to_owned(), folders.user_home.display().to_string()),
                (
                    "XDG_CACHE_HOME".to_owned(),
                    folders.user_home.join(".cache").display().to_string(),
                ),
            ],
            ..Launch::default()
        },
        trace_dir: None,
    }
}

async fn run(turn: &Turn, folders: &Folders) -> Vec<String> {
    let mut process = spawn(
        turn,
        &[
            "-c".to_owned(),
            SCRIPT.to_owned(),
            "sh".to_owned(),
            folders.outside.display().to_string(),
            folders.engine_home.display().to_string(),
            folders.user_home.display().to_string(),
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
async fn an_auto_engine_writes_its_folders_and_tool_caches_but_not_the_users_files() {
    if let Support::Unavailable(reason) = forge_isolation::support() {
        eprintln!("SKIPPED: this kernel can't enforce the sandbox ({reason:?})");
        return;
    }
    let folders = folders();
    let lines = run(&turn(Permissions::Auto, &folders), &folders).await;
    assert_eq!(lines.last().map(String::as_str), Some("done"), "{lines:?}");
    assert!(folders.project.join("made").exists(), "{lines:?}");
    assert!(folders.engine_home.join("state").exists(), "{lines:?}");
    assert!(
        folders
            .user_home
            .join(".cache")
            .join("tool")
            .join("entry")
            .exists(),
        "a tool writes its cache"
    );
    assert!(
        folders
            .user_home
            .join(".cargo")
            .join("registry")
            .join("entry")
            .exists(),
        "cargo fetches into a registry Forge made for it"
    );
    assert!(
        !folders.user_home.join(".gradle").exists(),
        "no cache is made for a tool that isn't installed"
    );
    assert!(
        !folders.outside.join("escaped").exists(),
        "an Auto engine wrote outside its folders"
    );
    assert_eq!(
        std::fs::read_to_string(folders.user_home.join(".bashrc")).unwrap(),
        "kept\n",
        "an Auto engine changed a shell rc file"
    );
    assert!(
        !folders.user_home.join(".config").join("tool").exists(),
        "an Auto engine wrote the user's configuration"
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
    let folders = folders();
    let lines = run(&turn(Permissions::Full, &folders), &folders).await;
    assert_eq!(lines.last().map(String::as_str), Some("done"), "{lines:?}");
    assert!(
        folders.outside.join("escaped").exists(),
        "Full access is not sandboxed"
    );
    assert_eq!(
        std::fs::read_to_string(folders.user_home.join(".bashrc")).unwrap(),
        "kept\nhijacked\n"
    );
}
