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
    run_script(
        turn,
        SCRIPT,
        &[
            folders.outside.display().to_string(),
            folders.engine_home.display().to_string(),
            folders.user_home.display().to_string(),
        ],
    )
    .await
}

async fn run_script(turn: &Turn, script: &str, args: &[String]) -> Vec<String> {
    let mut command = vec!["-c".to_owned(), script.to_owned(), "sh".to_owned()];
    command.extend_from_slice(args);
    let mut process = spawn(turn, &command).unwrap();
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

const WRITE_EACH: &str = r#"
for folder in "$@"; do
  echo planted > "$folder/planted" && echo "wrote $folder"
done
echo done
"#;

fn repository(folder: &Path) {
    std::fs::create_dir_all(folder.join("objects")).expect("an objects folder");
    std::fs::create_dir_all(folder.join("refs")).expect("a refs folder");
    std::fs::write(folder.join("HEAD"), "ref: refs/heads/main\n").expect("a HEAD");
}

fn worktree_of(common: &Path, project: &Path, commondir: &str) {
    let own = common.join("worktrees").join("project");
    std::fs::create_dir_all(&own).expect("a worktree git folder");
    std::fs::write(
        own.join("gitdir"),
        format!("{}\n", project.join(".git").display()),
    )
    .expect("a backlink");
    std::fs::write(own.join("commondir"), format!("{commondir}\n")).expect("a commondir");
    std::fs::write(own.join("HEAD"), "ref: refs/heads/feature\n").expect("a HEAD");
    std::fs::write(project.join(".git"), format!("gitdir: {}\n", own.display()))
        .expect("a git pointer");
}

async fn written(folders: &Folders, targets: &[&Path]) -> Vec<String> {
    let args: Vec<String> = targets
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    let lines = run_script(&turn(Permissions::Auto, folders), WRITE_EACH, &args).await;
    assert_eq!(lines.last().map(String::as_str), Some("done"), "{lines:?}");
    lines
}

fn sandboxed() -> bool {
    if let Support::Unavailable(reason) = forge_isolation::support() {
        eprintln!("SKIPPED: this kernel can't enforce the sandbox ({reason:?})");
        return false;
    }
    true
}

#[tokio::test]
async fn a_git_pointer_the_engine_wrote_does_not_make_its_target_writable() {
    if !sandboxed() {
        return;
    }
    let folders = folders();
    repository(&folders.outside);
    for target in [&folders.outside, &folders.user_home] {
        std::fs::write(
            folders.project.join(".git"),
            format!("gitdir: {}\n", target.display()),
        )
        .unwrap();
        written(&folders, &[target]).await;
        assert!(
            !target.join("planted").exists(),
            "a `.git` file pointing at {} let an Auto engine write there",
            target.display()
        );
    }
}

#[tokio::test]
async fn a_worktree_commondir_naming_another_folder_does_not_make_it_writable() {
    if !sandboxed() {
        return;
    }
    let folders = folders();
    let common = folders.user_home.join("repository");
    repository(&common);
    repository(&folders.outside);
    let hostile = folders.outside.display().to_string();
    worktree_of(&common, &folders.project, &hostile);
    written(&folders, &[&folders.outside, &common]).await;
    assert!(
        !folders.outside.join("planted").exists(),
        "a worktree's commondir let an Auto engine write another folder"
    );
    assert!(
        !common.join("planted").exists(),
        "a worktree with a tampered commondir still got its shared folder"
    );
}

#[tokio::test]
async fn a_linked_worktree_lets_git_write_its_shared_folder() {
    if !sandboxed() {
        return;
    }
    let folders = folders();
    let common = folders.user_home.join("repository");
    repository(&common);
    worktree_of(&common, &folders.project, "../..");
    let objects = common.join("objects");
    let lines = written(&folders, &[&objects]).await;
    assert!(
        objects.join("planted").exists(),
        "git can't write the repository's shared folder from a worktree: {lines:?}"
    );
}
