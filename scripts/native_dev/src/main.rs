//! `dev` binary: `nix run .#dev` on Linux, and the Editor half on Windows.
//!
//! The binary is deliberately thin: every reusable step lives in the
//! [`native_dev`] library so it stays covered by `tests/native_dev`. Here
//! only command dispatch, the order of the two halves, and exit code
//! propagation remain.

#![forbid(unsafe_code)]

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
};

use artisan_build_info::BuildIdentity;
use artisan_editor_cli::{
    host_access::INVITATION_FILE,
    service::{ForgeService, UserSystemctl},
};
use native_dev::{
    Action, Command, DEV_ROOT_ENV, DevArgs, DevError, DevPaths, EditorArgs, EditorPlatform,
    HostAccess, Progress,
    cargo,
    editor::{self, EditorOptions, EditorOutcome},
    host::{self, HostOptions},
    nix::{self, Checkout, Stage, Target},
    parse, prune, resolve_dev_root, staged_editor, staged_forge, usage, wsl,
};

fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let result = match parse(&argv) {
        Err(error) => {
            eprintln!("dev: error: {}", native_dev::error_chain(&error));
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
            eprintln!("dev: error: {}", native_dev::error_chain(&error));
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
        let runner = windows_runner(options, &checkout()?)?;
        windows_half(&runner, command, None, None, options)
    };
    match options.command {
        Command::Where => {
            report_forge(&paths);
            if windows {
                return windows_maintenance("where");
            }
            report_editor(&paths);
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
    let built = if uses_cargo(options) {
        build_with_cargo(checkout, windows)?
    } else {
        build_with_nix(checkout, options.stage, windows)?
    };
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
            payload: built.forge_payload,
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
    if let Some((runner, payload)) = &built.windows {
        return windows_half(
            runner,
            command,
            Some(payload),
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

/// What one dev run built: the Linux payload the Forge installs from, and,
/// when the Editor runs on Windows, the Windows runner and payload.
struct Built {
    forge_payload: PathBuf,
    windows: Option<(PathBuf, PathBuf)>,
}

/// Whether this run builds with Cargo (the Debug loop) rather than Nix.
fn uses_cargo(options: &DevArgs) -> bool {
    options.stage == Stage::Debug && !options.sandboxed
}

/// Builds the Debug stage with Cargo, one platform after the other: each
/// build already fills every core, and two Debug builds at once exceed the
/// memory of a 16 GB machine.
fn build_with_cargo(checkout: &Checkout, windows: bool) -> Result<Built, DevError> {
    let forge_payload = cargo::payload(checkout, Target::Linux)?;
    let windows = if windows {
        let payload = cargo::payload(checkout, Target::Windows)?;
        Some((cargo::runner(checkout, Target::Windows)?, payload))
    } else {
        None
    };
    Ok(Built {
        forge_payload,
        windows,
    })
}

fn build_with_nix(checkout: &Checkout, stage: Stage, windows: bool) -> Result<Built, DevError> {
    checkout.require_tracked_sources()?;
    let mut attributes = vec![nix::payload_attribute(Target::Linux, stage)];
    if windows {
        attributes.push(nix::payload_attribute(Target::Windows, stage));
        attributes.push(nix::runner_attribute(Target::Windows));
    }
    let outputs = nix_build(checkout, &attributes, stage.concurrent_builds())?;
    Ok(Built {
        forge_payload: outputs[0].clone(),
        windows: windows.then(|| (nix_runner_executable(&outputs[2]), outputs[1].clone())),
    })
}

/// The Windows runner, built the way this run builds.
fn windows_runner(options: &DevArgs, checkout: &Checkout) -> Result<PathBuf, DevError> {
    if uses_cargo(options) {
        return cargo::runner(checkout, Target::Windows);
    }
    let outputs = nix_build(checkout, &[nix::runner_attribute(Target::Windows)], 1)?;
    Ok(nix_runner_executable(&outputs[0]))
}

fn nix_runner_executable(output: &Path) -> PathBuf {
    output.join("bin").join("dev.exe")
}

fn nix_build(
    checkout: &Checkout,
    attributes: &[String],
    concurrent: u8,
) -> Result<Vec<PathBuf>, DevError> {
    println!("dev: building {}", attributes.join(", "));
    let installables: Vec<String> = attributes
        .iter()
        .map(|attribute| checkout.installable(attribute))
        .collect();
    nix::build(&installables, concurrent)
}

/// Runs the Windows runner's Editor half through WSL interop.
fn windows_half(
    executable: &Path,
    command: &str,
    payload: Option<&Path>,
    invitation: Option<&Path>,
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
    let status = std::process::Command::new(executable)
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
            report_editor(&paths);
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

/// Prints the Forge's installation, active build, service, and invitation.
fn report_forge(paths: &DevPaths) {
    println!("forge root: {}", paths.home.display());
    let Ok(version_root) = paths.active_version_root() else {
        println!("forge build: nothing installed yet; run `nix run .#dev`");
        return;
    };
    println!(
        "forge build: {}",
        BuildIdentity::for_executable(&staged_forge(&version_root))
    );
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

/// Prints the Editor's installation and active build (the keys
/// `scripts/verify_visual.ps1` reads).
fn report_editor(paths: &DevPaths) {
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
}

/// This machine's host name.
fn host_name() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}
