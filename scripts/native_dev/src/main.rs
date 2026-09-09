//! `dev` binary: stage the native development installation and launch it.
//!
//! The binary is deliberately thin: every reusable step lives in the
//! [`native_dev`] library so it stays covered by `tests/native_dev`. Here
//! only stage printing, Editor process control, and exit code propagation
//! remain.

#![forbid(unsafe_code)]

use std::time::Duration;

use native_dev::{
    Action, DEV_STARTUP_TIMEOUT_MS, DevArgs, DevError, DevLock, DevPaths, InstanceOutcome,
    ReadinessReconcile, StartupWait, clear_stale_receipt, fresh_receipt_path, locate_binaries,
    provision_forge_home, provision_manifest, reconcile_stale_readiness, refuse_live_forge,
    resolve_dev_dir, spawn_editor, stage_binaries, stage_line, staged_editor, staged_forge,
    stop_editor, usage, wait_for_startup,
};

/// Number of stages in a full stage-and-launch run.
const FULL_STAGES: u32 = 7;

/// Number of stages in a `--stage-only` run.
const STAGE_ONLY_STAGES: u32 = 6;

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

fn run() -> Result<u8, Outcome> {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let action = DevArgs::parse(&argv).map_err(|error| {
        eprintln!("dev: error: {error}");
        eprintln!("{}", usage());
        Outcome::Usage
    })?;
    let Action::Run(args) = action else {
        println!("{}", usage());
        return Ok(0);
    };
    let total = if args.stage_only {
        STAGE_ONLY_STAGES
    } else {
        FULL_STAGES
    };
    let fail = |error: DevError| {
        eprintln!("dev: error: {error}");
        Outcome::Failure
    };

    let dev_dir = resolve_dev_dir(args.dev_dir.as_deref()).map_err(fail)?;
    let paths = DevPaths::new(&dev_dir).map_err(fail)?;
    println!(
        "{}",
        stage_line(
            1,
            total,
            "resolve",
            &format!("dev home {}", paths.home.display())
        )
    );

    let binaries = locate_binaries(args.bin_dir.as_deref()).map_err(fail)?;
    println!(
        "{}",
        stage_line(2, total, "binaries", &binaries.forge.display().to_string())
    );

    let _lock = DevLock::acquire(&paths).map_err(fail)?;
    refuse_live_forge(&paths, &staged_forge(&paths)).map_err(fail)?;
    println!("{}", stage_line(3, total, "lock", "staging lock held"));
    let outcome = provision_forge_home(&paths).map_err(fail)?;
    let detail = match outcome {
        InstanceOutcome::Created => "fresh identity minted",
        InstanceOutcome::Preserved => "identity and data preserved",
    };
    println!("{}", stage_line(4, total, "provision", detail));

    let counts = stage_binaries(&binaries, &paths).map_err(|error| {
        eprintln!("dev: error: {error}");
        eprintln!("dev: hint: the active version is untouched; fix the cause and retry");
        Outcome::Failure
    })?;
    println!(
        "{}",
        stage_line(
            5,
            total,
            "stage",
            &format!("{} rewritten, {} reused", counts.rewritten, counts.reused),
        )
    );

    provision_manifest(&paths).map_err(fail)?;
    println!(
        "{}",
        stage_line(6, total, "manifest", "verified by shipping loader")
    );

    if args.stage_only {
        println!(
            "dev: staged without launch; run with ARTISAN_HOME={} {}",
            paths.home.display(),
            staged_editor(&paths).display()
        );
        return Ok(0);
    }

    // `_lock` stays held for the whole owned Editor lifetime: another
    // runner must not stage or start on this home before the Forge receipt
    // exists. It releases when this process exits; the Editor child never
    // acquires it.
    let receipt_path = fresh_receipt_path(&paths);
    clear_stale_receipt(&receipt_path).map_err(fail)?;
    let editor = staged_editor(&paths);
    let forge = staged_forge(&paths);
    match reconcile_stale_readiness(&paths, &forge).map_err(fail)? {
        ReadinessReconcile::Absent => {}
        ReadinessReconcile::CleanedStale { pid } => {
            println!("dev: removed stale readiness of dead forge pid {pid}");
        }
    }
    println!(
        "dev: launching staged editor {} on its owned forge (close the window to stop)",
        editor.display()
    );
    let mut child = spawn_editor(&editor, &paths.home, &receipt_path).map_err(fail)?;
    match wait_for_startup(
        &mut child,
        &receipt_path,
        Duration::from_millis(DEV_STARTUP_TIMEOUT_MS),
    ) {
        StartupWait::Ready { stage } => {
            println!("{}", stage_line(7, total, "startup", &stage));
        }
        StartupWait::Failed { stage, reason } => {
            stop_editor(child);
            eprintln!("dev: stage 7/{total} startup ... failed ({stage}: {reason})");
            return Err(Outcome::Failure);
        }
        StartupWait::Timeout => {
            stop_editor(child);
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
    let status = child.wait().map_err(|_| {
        eprintln!("dev: error: cannot wait for the staged editor");
        Outcome::Failure
    })?;
    match status.code() {
        Some(0) => {
            println!("{}", stage_line(7, total, "editor", "exit 0"));
            Ok(0)
        }
        Some(code) => {
            eprintln!("dev: stage 7/{total} editor ... failed (exit {code})");
            Ok(u8::try_from(code).unwrap_or(1))
        }
        None => {
            eprintln!("dev: stage 7/{total} editor ... failed (terminated by signal)");
            Err(Outcome::Failure)
        }
    }
}
