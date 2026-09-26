//! `dev` binary: `nix run .#dev` on Linux, and the Editor half on Windows.
//!
//! The binary is deliberately thin: every reusable step lives in the
//! [`native_dev`] library so it stays covered by `tests/native_dev`. Here
//! only command dispatch, the order of the two halves, and exit code
//! propagation remain.

#![forbid(unsafe_code)]

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use artisan_build_info::BuildIdentity;
use artisan_editor_cli::{
    host_access::INVITATION_FILE,
    service::{ForgeService, UserSystemctl},
};
use native_dev::{
    Action, Command, DEV_ROOT_ENV, DevArgs, DevError, DevPaths, EditorArgs, EditorPlatform,
    HostAccess, Progress,
    editor::{self, EditorOptions, EditorOutcome},
    host::{self, HostOptions},
    nix::{self, Checkout, Target},
    parse, prune, resolve_dev_root, staged_editor, staged_forge, usage, wsl,
};

fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let result = match parse(&argv) {
        Err(error) => {
            eprintln!("dev: error: {error}");
            println!("{}", usage());
            return ExitCode::from(2);
        }
        Ok(Action::Help) => {
            println!("{}", usage());
            return ExitCode::SUCCESS;
        }
        Ok(Action::Execute(options)) => orchestrate(&options),
        Ok(Action::Editor(options)) => editor_half(&options),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("dev: error: {error}");
            if matches!(error, DevError::Install(_)) {
                eprintln!("dev: hint: the previously active version is untouched");
            }
            ExitCode::FAILURE
        }
    }
}

/// Both halves, from Linux.
fn orchestrate(options: &DevArgs) -> Result<u8, DevError> {
    if cfg!(windows) {
        return Err(DevError::Usage {
            reason: "run `nix run .#dev` from Linux or WSL; Windows runs only the Editor half"
                .to_owned(),
        });
    }
    let paths = DevPaths::new(&resolve_dev_root(options.root.as_deref())?)?;
    let platform = options.editor.unwrap_or(if wsl::interop_available() {
        EditorPlatform::Windows
    } else {
        EditorPlatform::Linux
    });
    let windows = platform == EditorPlatform::Windows;
    let checkout = || Checkout::locate(&std::env::current_dir().unwrap_or_default());
    let windows_maintenance = |command: &str| {
        let runner = build(&checkout()?, &[nix::runner_attribute(Target::Windows)])?;
        windows_half(&runner[0], command, None, None, options)
    };
    match options.command {
        Command::Where => {
            report_where(&paths);
            if windows {
                return windows_maintenance("where");
            }
            Ok(0)
        }
        Command::Prune => {
            prune(&paths, options.keep);
            if windows {
                return windows_maintenance("prune");
            }
            Ok(0)
        }
        Command::Run | Command::Stage => deploy(options, &checkout()?, &paths, windows),
    }
}

fn deploy(
    options: &DevArgs,
    checkout: &Checkout,
    paths: &DevPaths,
    windows: bool,
) -> Result<u8, DevError> {
    checkout.require_tracked_sources()?;
    let mut attributes = vec![nix::payload_attribute(Target::Linux, options.stage)];
    if windows {
        attributes.push(nix::payload_attribute(Target::Windows, options.stage));
        attributes.push(nix::runner_attribute(Target::Windows));
    }
    let outputs = build(checkout, &attributes)?;
    let default_root = options.root.is_none() && std::env::var_os(DEV_ROOT_ENV).is_none();
    let wsl_host = wsl::interop_available() || wsl::distribution().is_some();
    let access = HostAccess {
        listen: options.listen.clone().unwrap_or_else(|| {
            if wsl_host {
                "auto:4433".to_owned()
            } else {
                "127.0.0.1:4433".to_owned()
            }
        }),
        name: options
            .host_name
            .clone()
            .or_else(wsl::distribution)
            .or_else(host_name)
            .unwrap_or_else(|| "Linux".to_owned()),
    };
    let deployment = host::deploy(
        &HostOptions {
            paths: paths.clone(),
            payload: outputs[0].clone(),
            access,
            keep: options.keep,
            default_root,
        },
        &mut Progress::new("forge"),
    )?;
    let command = if options.command == Command::Run {
        "run"
    } else {
        "stage"
    };
    if windows {
        return windows_half(
            &outputs[2],
            command,
            Some(&outputs[1]),
            Some(&deployment.invitation),
            options,
        );
    }
    let outcome = editor::deploy(
        &EditorOptions {
            paths: paths.clone(),
            payload: None,
            invitation: deployment.invitation,
            launch: options.command == Command::Run,
            attach: options.attach,
            keep: options.keep,
        },
        &mut Progress::new("editor"),
    )?;
    Ok(exit_code(outcome))
}

fn build(checkout: &Checkout, attributes: &[String]) -> Result<Vec<PathBuf>, DevError> {
    println!("dev: building {}", attributes.join(", "));
    let installables: Vec<String> = attributes
        .iter()
        .map(|attribute| checkout.installable(attribute))
        .collect();
    nix::build(&installables)
}

/// Runs the Windows runner's Editor half through WSL interop.
fn windows_half(
    runner: &std::path::Path,
    command: &str,
    payload: Option<&std::path::Path>,
    invitation: Option<&std::path::Path>,
    options: &DevArgs,
) -> Result<u8, DevError> {
    let distribution = wsl::distribution().ok_or_else(|| DevError::Stage {
        stage: "interop",
        reason: "the Windows Editor needs WSL ($WSL_DISTRO_NAME is unset); pass --linux".to_owned(),
    })?;
    let arguments = wsl::editor_arguments(
        &wsl::EditorInvocation {
            command,
            payload,
            invitation,
            keep: options.keep,
            root: options.windows_root.as_ref(),
            attach: options.attach,
        },
        &distribution,
    )?;
    let executable = runner.join("bin").join("dev.exe");
    let status = std::process::Command::new(&executable)
        .args(&arguments)
        .status()
        .map_err(|error| DevError::Stage {
            stage: "interop",
            reason: format!("cannot run {}: {error}", executable.display()),
        })?;
    Ok(status
        .code()
        .map_or(1, |code| u8::try_from(code).unwrap_or(1)))
}

/// The Editor half, on the Editor's platform.
fn editor_half(options: &EditorArgs) -> Result<u8, DevError> {
    let paths = DevPaths::new(&resolve_dev_root(options.root.as_deref())?)?;
    match options.command {
        Command::Where => {
            report_where(&paths);
            Ok(0)
        }
        Command::Prune => {
            prune(&paths, options.keep);
            Ok(0)
        }
        Command::Run | Command::Stage => {
            let invitation = options.invitation.clone().ok_or_else(|| DevError::Usage {
                reason: "no invitation to register".to_owned(),
            })?;
            let outcome = editor::deploy(
                &EditorOptions {
                    paths,
                    payload: options.payload.clone(),
                    invitation,
                    launch: options.command == Command::Run,
                    attach: options.attach,
                    keep: options.keep,
                },
                &mut Progress::new("editor"),
            )?;
            Ok(exit_code(outcome))
        }
    }
}

fn exit_code(outcome: EditorOutcome) -> u8 {
    match outcome {
        EditorOutcome::Staged | EditorOutcome::Running { .. } => 0,
        EditorOutcome::Exited { code } => u8::try_from(code).unwrap_or(1),
    }
}

/// Prints one installation, its active build, and (on Linux) its Forge.
fn report_where(paths: &DevPaths) {
    println!("root: {}", paths.home.display());
    let Ok(version_root) = paths.active_version_root() else {
        println!("build: nothing installed yet; run `nix run .#dev`");
        return;
    };
    println!(
        "build: {}",
        BuildIdentity::for_executable(&staged_editor(&version_root))
    );
    println!("editor: {}", staged_editor(&version_root).display());
    if cfg!(target_os = "linux") {
        println!("forge: {}", staged_forge(&version_root).display());
        if let Ok(service) = ForgeService::for_current_user(&paths.home) {
            let active = service.is_active(&UserSystemctl).unwrap_or(false);
            println!(
                "service: {} ({})",
                service.unit_name(),
                if active { "active" } else { "inactive" }
            );
        }
        let invitation = paths.home.join(INVITATION_FILE);
        if invitation.is_file() {
            println!("invitation: {}", invitation.display());
        }
    }
}

/// This machine's host name.
fn host_name() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}
