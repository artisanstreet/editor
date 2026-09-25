//! `dev` binary: install a Nix-built payload and launch the dev Editor.
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
    InstanceOutcome, ReadinessReconcile, StartupWait, clear_stale_receipt, fresh_receipt_path,
    install_payload, locate_binaries, payload_identity, provision_forge_home,
    reconcile_stale_readiness, resolve_dev_root, sign_payload, spawn_editor, stage_line,
    staged_editor, staged_forge, stop_editor, usage, wait_for_startup,
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
        Err(_) => println!("build: nothing installed yet; run `nix run .#dev`"),
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

/// An attached run shares the runner's streams; otherwise the Editor
/// outlives the runner and writes to a log in the runner directory.
fn editor_output(options: &DevArgs, paths: &DevPaths) -> EditorOutput {
    if options.attach {
        EditorOutput::Inherit
    } else {
        EditorOutput::Detached(paths.runner_dir().join("editor.log"))
    }
}

fn install_and_launch(options: &DevArgs, paths: &DevPaths) -> Result<u8, Outcome> {
    let launch = options.command == Command::Run;
    let total = if launch { 7 } else { 5 };
    let Some(payload) = options.payload.as_deref() else {
        return Err(fail(DevError::Usage {
            reason: "no payload to install".to_owned(),
        }));
    };
    let identity = payload_identity(payload).map_err(fail)?;
    locate_binaries(Some(&payload.join("bin"))).map_err(fail)?;
    println!("{}", stage_line(1, total, "payload", &identity.version));

    let lock = DevLock::acquire(paths).map_err(fail)?;
    let signer =
        LocalSigner::load_or_create(&paths.home).map_err(|error| fail(DevError::Install(error)))?;
    let manifests = paths.runner_dir().join("manifest");
    sign_payload(payload, &manifests, &identity, &signer).map_err(fail)?;
    println!("{}", stage_line(2, total, "sign", signer.key_id()));

    install_payload(paths, payload, &manifests, &signer).map_err(|error| {
        eprintln!("dev: error: {error}");
        eprintln!("dev: hint: the previously active version is untouched");
        Outcome::Failure
    })?;
    println!("{}", stage_line(3, total, "install", &describe(&identity)));

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
    launch_installed(options, paths, lock, &version_root, total)
}

/// Stages 6 and 7: launch the installed Editor and confirm its startup.
/// The install lock is released once the Editor is spawned.
fn launch_installed(
    options: &DevArgs,
    paths: &DevPaths,
    lock: DevLock,
    version_root: &Path,
    total: u32,
) -> Result<u8, Outcome> {
    let editor = staged_editor(version_root);
    let receipt_path = fresh_receipt_path(paths);
    clear_stale_receipt(&receipt_path).map_err(fail)?;
    // Installing retired every superseded version, so a live Forge of the
    // active version means this exact build is already running: an
    // unchanged tree builds the same payload.
    match reconcile_stale_readiness(paths, &staged_forge(version_root)) {
        Ok(ReadinessReconcile::Absent) => {}
        Ok(ReadinessReconcile::CleanedStale { pid }) => {
            println!("dev: removed stale readiness of dead forge pid {pid}");
        }
        Err(DevError::PreviousForgeRunning { pid }) => {
            println!("dev: this build is already running (forge pid {pid}); nothing to relaunch");
            return Ok(0);
        }
        Err(error) => return Err(fail(error)),
    }
    println!(
        "{}",
        stage_line(6, total, "launch", &editor.display().to_string())
    );
    let output = editor_output(options, paths);
    let mut child = spawn_editor(&editor, &paths.home, &receipt_path, &output).map_err(fail)?;
    // Installs and launches are serialized only up to here: the lock is
    // never held for the Editor's lifetime, so the next `nix run .#dev` can
    // retire this Editor and relaunch its new build.
    drop(lock);
    let startup = wait_for_startup(
        &mut child,
        &receipt_path,
        Duration::from_millis(DEV_STARTUP_TIMEOUT_MS),
    );
    let _ = std::fs::remove_file(&receipt_path);
    match startup {
        StartupWait::Ready { stage } => {
            println!("{}", stage_line(7, total, "startup", &stage));
        }
        StartupWait::Failed { stage, reason } => {
            let _ = stop_editor(child);
            eprintln!("dev: stage 7/{total} startup ... failed ({stage}: {reason})");
            return Err(Outcome::Failure);
        }
        StartupWait::Timeout => {
            let _ = stop_editor(child);
            eprintln!(
                "dev: stage 7/{total} startup ... failed (no receipt within {}s)",
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
    if let EditorOutput::Detached(log) = &output {
        println!(
            "dev: editor running (pid {}, output in {}); run `nix run .#dev` again to replace it with a new build",
            child.id(),
            log.display()
        );
        return Ok(0);
    }
    wait_for_exit(child, version_root)
}

/// Follows the dev Editor until it exits, including when a later run
/// retires it for a newer build.
fn wait_for_exit(mut child: std::process::Child, version_root: &Path) -> Result<u8, Outcome> {
    let status = child.wait().map_err(|_| {
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
