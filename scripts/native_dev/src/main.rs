//! `dev` binary (`cargo dev`): build, install, and launch the dev Editor.
//!
//! The binary is deliberately thin: every reusable step lives in the
//! [`native_dev`] library so it stays covered by `tests/native_dev`. Here
//! only command dispatch, stage printing, Editor process control, and exit
//! code propagation remain.

#![forbid(unsafe_code)]

use std::{path::Path, time::Duration};

use artisan_build_info::{BuildIdentity, BuildInfo};
use artisan_install::LocalSigner;
use native_dev::{
    Action, Command, DEV_STARTUP_TIMEOUT_MS, DevArgs, DevError, DevLock, DevPaths, EditorOutput,
    EditorProcess, GitState, InstanceOutcome, PAYLOAD_DIRECTORY, ReadinessReconcile, StartupWait,
    Workspace, assemble, clear_stale_receipt, editor_log_path, editor_output, fresh_receipt_path,
    install_tree, locate_binaries, profile_for_bin_dir, provision_forge_home,
    reconcile_stale_readiness, resolve_dev_root, spawn_editor, stage_line, staged_editor,
    staged_forge, streams_are_terminals, usage, wait_for_startup,
};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(code) => std::process::ExitCode::from(code),
        Err(Outcome::Usage) => {
            println!("{}", usage());
            std::process::ExitCode::from(2)
        }
        Err(Outcome::Failure) => std::process::ExitCode::from(1),
    }
}

enum Outcome {
    Usage,
    Failure,
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "a `map_err` adapter receives the error by value"
)]
fn fail(error: DevError) -> Outcome {
    eprintln!("dev: error: {error}");
    Outcome::Failure
}

fn run() -> Result<u8, Outcome> {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let action = DevArgs::parse(&argv).map_err(|error| {
        eprintln!("dev: error: {error}");
        Outcome::Usage
    })?;
    let Action::Execute(options) = action else {
        println!("{}", usage());
        return Ok(0);
    };
    let root = resolve_dev_root(options.root.as_deref()).map_err(fail)?;
    let paths = DevPaths::new(&root).map_err(fail)?;
    match options.command {
        Command::Where => {
            report_where(&paths);
            Ok(0)
        }
        Command::Prune => {
            prune(&paths, options.keep);
            Ok(0)
        }
        Command::Stage | Command::Run => install_and_launch(&options, &paths),
    }
}

/// Prints where the dev installation lives and what it runs.
fn report_where(paths: &DevPaths) {
    println!("root: {}", paths.home.display());
    match paths.active_version_root() {
        Ok(version_root) => {
            let editor = staged_editor(&version_root);
            println!("editor: {}", editor.display());
            println!("build: {}", BuildIdentity::for_executable(&editor));
        }
        Err(_) => println!("build: nothing installed yet; run `cargo dev`"),
    }
}

/// Removes superseded versions, reporting what was kept in use.
fn prune(paths: &DevPaths, keep: usize) {
    match artisan_install::prune(&paths.home, keep) {
        Ok(report) => {
            for version in &report.removed {
                println!("dev: pruned {version}");
            }
            for version in &report.in_use {
                println!("dev: kept {version} (in use)");
            }
        }
        Err(error) => eprintln!("dev: warning: prune skipped: {error}"),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "linear stage-by-stage runbook; each stage's error hint stays adjacent to its call"
)]
fn install_and_launch(options: &DevArgs, paths: &DevPaths) -> Result<u8, Outcome> {
    let launch = options.command == Command::Run;
    let total = if launch { 7 } else { 5 };
    // Prebuilt binaries (for example from Nix) need no Cargo workspace;
    // building does.
    let workspace = match (Workspace::locate(), &options.bin_dir) {
        (Ok(workspace), _) => Some(workspace),
        (Err(_), Some(_)) => None,
        (Err(error), None) => return Err(fail(error)),
    };
    let (bin_dir, profile) = match (&options.bin_dir, &workspace) {
        (Some(bin_dir), _) => (
            bin_dir.clone(),
            options
                .profile
                .clone()
                .unwrap_or_else(|| profile_for_bin_dir(bin_dir)),
        ),
        (None, Some(workspace)) => {
            let profile = options.profile.clone().unwrap_or_else(|| "dev".to_owned());
            (workspace.build(&profile).map_err(fail)?, profile)
        }
        (None, None) => unreachable!("building requires a located workspace"),
    };
    let binaries = locate_binaries(Some(&bin_dir)).map_err(fail)?;
    println!(
        "{}",
        stage_line(
            1,
            total,
            "build",
            &format!("{profile} in {}", bin_dir.display())
        )
    );

    let lock = DevLock::acquire(paths).map_err(fail)?;
    let signer =
        LocalSigner::load_or_create(&paths.home).map_err(|error| fail(DevError::Install(error)))?;
    let (tree, checkout) = match &workspace {
        Some(workspace) => (
            workspace.target_directory.join(PAYLOAD_DIRECTORY),
            workspace.root.clone(),
        ),
        None => (
            paths.runner_dir().join("payload"),
            std::env::current_dir().unwrap_or_default(),
        ),
    };
    let git = GitState::read(&checkout);
    let info = assemble(&binaries, &git, &profile, &tree, &signer).map_err(fail)?;
    println!("{}", stage_line(2, total, "assemble", &info.version));

    install_tree(paths, &tree, &signer).map_err(|error| {
        eprintln!("dev: error: {error}");
        eprintln!("dev: hint: the previously active version is untouched");
        Outcome::Failure
    })?;
    println!("{}", stage_line(3, total, "install", &describe(&info)));

    let outcome = provision_forge_home(paths).map_err(fail)?;
    let detail = match outcome {
        InstanceOutcome::Created => "fresh identity minted",
        InstanceOutcome::Preserved => "identity and data preserved",
    };
    println!("{}", stage_line(4, total, "provision", detail));
    prune(paths, options.keep);
    println!(
        "{}",
        stage_line(5, total, "prune", &format!("keep {}", options.keep))
    );

    let version_root = paths.active_version_root().map_err(fail)?;
    let editor = staged_editor(&version_root);
    if !launch {
        println!(
            "dev: installed without launch; run with {}={} {}",
            native_dev::DEV_HOME_ENV,
            paths.home.display(),
            editor.display()
        );
        return Ok(0);
    }
    let receipt_path = fresh_receipt_path(paths);
    clear_stale_receipt(&receipt_path).map_err(fail)?;
    match reconcile_stale_readiness(paths, &staged_forge(&version_root)).map_err(fail)? {
        ReadinessReconcile::Absent => {}
        ReadinessReconcile::CleanedStale { pid } => {
            println!("dev: removed stale readiness of dead forge pid {pid}");
        }
    }
    println!(
        "{}",
        stage_line(6, total, "launch", &editor.display().to_string())
    );
    let output = editor_output(
        options.attach,
        streams_are_terminals(),
        editor_log_path(paths),
    );
    let mut editor_process =
        spawn_editor(&editor, &paths.home, &receipt_path, &output).map_err(fail)?;
    // Installs and launches are serialized only up to here: the lock is
    // never held for the Editor's lifetime, so the next `cargo dev` can
    // retire this Editor and relaunch its new build.
    drop(lock);
    let startup = wait_for_startup(
        editor_process.child_mut(),
        &receipt_path,
        Duration::from_millis(DEV_STARTUP_TIMEOUT_MS),
    );
    let _ = std::fs::remove_file(&receipt_path);
    match startup {
        StartupWait::Ready { stage } => {
            println!("{}", stage_line(7, total, "startup", &stage));
        }
        StartupWait::Failed { stage, reason } => {
            let _ = editor_process.stop();
            eprintln!("dev: stage 7/{total} startup ... failed ({stage}: {reason})");
            return Err(Outcome::Failure);
        }
        StartupWait::Timeout => {
            let _ = editor_process.stop();
            eprintln!(
                "dev: stage 7/{total} startup ... failed (no startup receipt within {}s: the \
                 editor did not complete its first host connection; it was stopped)",
                DEV_STARTUP_TIMEOUT_MS / 1_000
            );
            return Err(Outcome::Failure);
        }
        StartupWait::EditorExited { code } => {
            eprintln!(
                "dev: stage 7/{total} startup ... failed (editor exited before confirming startup{})",
                code.map_or(String::new(), |code| format!(" with code {code}"))
            );
            return Err(Outcome::Failure);
        }
    }
    if !options.attach {
        let pid = editor_process.pid();
        editor_process.release();
        println!(
            "dev: editor running (pid {pid}); run `cargo dev` again to replace it with a new build"
        );
        if let EditorOutput::Detached { log } = &output {
            if cfg!(windows) {
                println!(
                    "dev: editor output is not captured while this run's output is not a terminal"
                );
            } else {
                println!("dev: editor output goes to {}", log.display());
            }
        }
        return Ok(0);
    }
    wait_for_exit(editor_process, &version_root)
}

/// Follows the dev Editor until it exits, including when a later run
/// retires it for a newer build.
fn wait_for_exit(editor_process: EditorProcess, version_root: &Path) -> Result<u8, Outcome> {
    let status = editor_process.wait().map_err(|_| {
        eprintln!("dev: error: cannot wait for the dev editor");
        Outcome::Failure
    })?;
    match status.code() {
        Some(0) => {
            println!("dev: editor from {} exited", version_root.display());
            Ok(0)
        }
        Some(code) => {
            eprintln!("dev: editor exited with {code}");
            Ok(u8::try_from(code).unwrap_or(1))
        }
        None => {
            eprintln!("dev: editor terminated by signal");
            Err(Outcome::Failure)
        }
    }
}

fn describe(info: &BuildInfo) -> String {
    match info.short_commit() {
        Some(commit) => format!(
            "{} channel, commit {commit}{}",
            info.channel.as_str(),
            if info.dirty {
                " with local changes"
            } else {
                ""
            }
        ),
        None => format!("{} channel", info.channel.as_str()),
    }
}
