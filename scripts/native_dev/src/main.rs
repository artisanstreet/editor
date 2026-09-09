//! `dev` binary: stage the native development installation and launch it.
//!
//! The binary is deliberately thin: every reusable step lives in the
//! [`native_dev`] library so it stays covered by `tests/native_dev`. Here
//! only process control remains — stage printing, Editor spawning, and exit
//! code propagation.

#![forbid(unsafe_code)]

use native_dev::{
    Action, DevArgs, DevPaths, exe_name, launch_editor, locate_binaries, provision_forge_home,
    provision_manifest, provision_payload, refuse_live_forge, resolve_dev_dir, stage_binaries,
    stage_line, usage,
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
    let fail = |error: native_dev::DevError| {
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

    let staged_forge = paths.version_bin.join(exe_name("forge"));
    refuse_live_forge(&paths, &staged_forge).map_err(fail)?;

    let staged = stage_binaries(&binaries, &paths).map_err(fail)?;
    let rewritten = staged.iter().filter(|(_, written)| *written).count();
    println!(
        "{}",
        stage_line(
            3,
            total,
            "stage",
            &format!("{rewritten} rewritten, {} reused", staged.len() - rewritten),
        )
    );

    provision_manifest(&paths).map_err(fail)?;
    println!(
        "{}",
        stage_line(4, total, "manifest", "verified by shipping loader")
    );

    provision_payload(&paths).map_err(|error| {
        eprintln!("dev: error: {error}");
        eprintln!(
            "dev: hint: delete {} and retry",
            paths.version_root.display()
        );
        Outcome::Failure
    })?;
    println!("{}", stage_line(5, total, "payload", "verified"));

    let outcome = provision_forge_home(&paths).map_err(fail)?;
    let detail = match outcome {
        native_dev::InstanceOutcome::Created => "fresh identity minted",
        native_dev::InstanceOutcome::Preserved => "identity and data preserved",
    };
    println!("{}", stage_line(6, total, "provision", detail));

    if args.stage_only {
        println!(
            "dev: staged without launch; run with ARTISAN_HOME={} {}",
            paths.home.display(),
            binaries.editor.display()
        );
        return Ok(0);
    }

    println!("dev: launching staged editor on its owned forge (close the window to stop)");
    let code = launch_editor(&binaries.editor, &paths.home).map_err(fail)?;
    if code == 0 {
        println!("{}", stage_line(7, total, "editor", "exit 0"));
        Ok(0)
    } else {
        eprintln!("dev: stage 7/{total} editor ... failed (exit {code})");
        Ok(u8::try_from(code).unwrap_or(1))
    }
}
