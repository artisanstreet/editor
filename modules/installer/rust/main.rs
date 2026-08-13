mod archive;
mod background_process;
mod error;
mod install;
mod integrations;
mod manifest;
mod payload;
mod platform;
mod processes;
mod shortcuts;

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use error::{InstallerError, Result};
use install::{InstallIntegrationOptions, InstallOptions, diagnose, install, repair, uninstall};
use manifest::TrustKey;
use platform::Platform;
use processes::RetirementPolicy;
use url::Url;

const DEFAULT_MANIFEST: &str = "https://github.com/sandersonstabo/artisan-editor/releases/latest/download/release-manifest.json";

#[derive(Clone, Debug, ValueEnum)]
enum Component {
    Editor,
    Forge,
    Cli,
}

impl Component {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Editor => "editor",
            Self::Forge => "forge",
            Self::Cli => "cli",
        }
    }
}

#[derive(Args, Debug)]
struct AutomationArguments {
    /// Answer every prompt with its safe default. Never implies a destructive
    /// action: ending a Forge that will not stop still requires `--force`.
    #[arg(long, short = 'y', global = true)]
    yes: bool,

    /// Ask a detached helper to remove this temporary executable after exit.
    #[arg(long, global = true)]
    self_cleanup: bool,
}

#[derive(Args, Debug)]
struct IntegrationArguments {
    /// Leave the desktop and Start Menu launchers alone. For a secondary
    /// install that must not claim the user's shortcuts.
    #[arg(long, global = true)]
    skip_shortcuts: bool,

    /// Leave the `artisan://` handler with its current owner. For a secondary
    /// install beside an existing installation.
    #[arg(long, global = true)]
    skip_protocol: bool,
}

#[derive(Args, Debug)]
struct ActivationArguments {
    /// Permit ending a Forge that did not stop when asked. Whatever it was
    /// running is lost.
    #[arg(long, global = true)]
    force: bool,

    /// Leave editor and Forge processes from superseded versions running. The
    /// activated release will not load until they are closed by hand.
    #[arg(long, global = true)]
    skip_retire: bool,

    /// Do not invoke permanent ae setup/doctor/status after activation.
    #[arg(long, global = true)]
    skip_setup: bool,
}

#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    #[command(subcommand)]
    operation: Option<Operation>,

    /// Signed release manifest URL.
    #[arg(long, default_value = DEFAULT_MANIFEST, global = true)]
    manifest_url: Url,

    /// Detached signature URL. Defaults to the manifest URL with `.json` replaced by `.sig`.
    #[arg(long, global = true)]
    signature_url: Option<Url>,

    /// Ed25519 public key as 32-byte hexadecimal. Overrides embedded release trust.
    #[arg(long, env = "ARTISAN_INSTALLER_PUBLIC_KEY", global = true)]
    public_key: Option<String>,

    /// Components to install, comma separated or repeated. Defaults to Editor,
    /// Forge, and the permanent ae CLI.
    #[arg(long, value_enum, value_delimiter = ',', global = true)]
    component: Vec<Component>,

    #[command(flatten)]
    automation: AutomationArguments,

    #[command(flatten)]
    integrations: IntegrationArguments,

    #[command(flatten)]
    activation: ActivationArguments,

    /// Per-user installation root.
    #[arg(long, env = "ARTISAN_INSTALL_ROOT", global = true)]
    install_root: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Operation {
    /// Verify bootstrap-owned integrations without changing them.
    #[command(hide = true)]
    Diagnose,
    /// Install the latest signed release without first-time Forge setup.
    Update,
    /// Restore bootstrap-owned launchers, PATH integration, and installation health.
    Repair,
    /// Remove installed binaries and owned integrations.
    Uninstall {
        /// Also permanently remove Forge data, projects, and conversations.
        #[arg(long)]
        remove_data: bool,
    },
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("ae installer failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let arguments = Arguments::parse();
    let platform = Platform::detect()?;
    let components = if arguments.component.is_empty() {
        vec!["editor", "forge", "cli"]
    } else {
        arguments.component.iter().map(Component::as_str).collect()
    };
    let root = arguments
        .install_root
        .clone()
        .unwrap_or_else(Platform::default_install_root);
    #[cfg(debug_assertions)]
    platform::forbid_default_install_root(&root)?;

    if let Some(operation) = arguments.operation.as_ref() {
        match operation {
            Operation::Diagnose => diagnose(&root)?,
            Operation::Update => {
                let trust = TrustKey::resolve(arguments.public_key.as_deref())?;
                install(make_install_options(
                    &arguments, platform, components, root, trust, false,
                ))
                .await?;
            }
            Operation::Repair => repair(&root)?,
            Operation::Uninstall { remove_data } => uninstall(&root, *remove_data)?,
        }
        return Ok(());
    }

    let trust = TrustKey::resolve(arguments.public_key.as_deref())?;
    install(InstallOptions {
        signature_url: arguments.signature_url.unwrap_or_else(|| {
            Url::parse(&arguments.manifest_url.as_str().replace(".json", ".sig"))
                .expect("derived signature URL")
        }),
        manifest_url: arguments.manifest_url,
        platform,
        components,
        install_root: root,
        trust,
        run_setup: !arguments.activation.skip_setup,
        integrations: InstallIntegrationOptions {
            register_protocol: !arguments.integrations.skip_protocol,
            register_shortcuts: !arguments.integrations.skip_shortcuts,
        },
        retirement: (!arguments.activation.skip_retire).then_some(RetirementPolicy {
            force: arguments.activation.force,
        }),
    })
    .await?;

    if arguments.automation.self_cleanup {
        schedule_self_cleanup()?;
    }
    Ok(())
}

fn make_install_options(
    arguments: &Arguments,
    platform: Platform,
    components: Vec<&'static str>,
    install_root: PathBuf,
    trust: TrustKey,
    run_setup: bool,
) -> InstallOptions {
    InstallOptions {
        signature_url: arguments.signature_url.clone().unwrap_or_else(|| {
            Url::parse(&arguments.manifest_url.as_str().replace(".json", ".sig"))
                .expect("derived signature URL")
        }),
        manifest_url: arguments.manifest_url.clone(),
        platform,
        components,
        install_root,
        trust,
        run_setup,
        integrations: InstallIntegrationOptions {
            register_protocol: !arguments.integrations.skip_protocol,
            register_shortcuts: !arguments.integrations.skip_shortcuts,
        },
        retirement: (!arguments.activation.skip_retire).then_some(RetirementPolicy {
            force: arguments.activation.force,
        }),
    }
}

fn schedule_self_cleanup() -> Result<()> {
    let executable = std::env::current_exe().map_err(InstallerError::CurrentExecutable)?;
    let temporary_root = std::env::temp_dir()
        .canonicalize()
        .map_err(InstallerError::TemporaryDirectory)?;
    let executable = executable
        .canonicalize()
        .map_err(InstallerError::CurrentExecutable)?;
    if !executable.starts_with(&temporary_root) {
        return Err(InstallerError::UnsafeSelfCleanup(executable));
    }

    #[cfg(windows)]
    {
        background_process::detached_background_command("cmd.exe")
            .args([
                "/d",
                "/s",
                "/c",
                "ping 127.0.0.1 -n 3 > nul & del /f /q \"%ARTISAN_BOOTSTRAP_DELETE%\"",
            ])
            .env("ARTISAN_BOOTSTRAP_DELETE", &executable)
            .spawn()
            .map_err(InstallerError::CleanupHelper)?;
    }
    #[cfg(unix)]
    {
        std::process::Command::new("sh")
            .args([
                "-c",
                "sleep 1; rm -f -- \"$1\"",
                "ae-installer-cleanup",
                executable
                    .to_str()
                    .ok_or_else(|| InstallerError::NonUtf8Path(executable.clone()))?,
            ])
            .spawn()
            .map_err(InstallerError::CleanupHelper)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Arguments, Operation};

    #[test]
    fn permanent_cli_maintenance_argument_order_is_supported() {
        let arguments =
            Arguments::try_parse_from(["ae-installer", "update", "--install-root", "/tmp/artisan"])
                .expect("maintenance invocation");
        assert!(matches!(arguments.operation, Some(Operation::Update)));
    }

    #[test]
    fn data_removal_is_explicit() {
        let arguments =
            Arguments::try_parse_from(["ae-installer", "uninstall"]).expect("uninstall");
        assert!(matches!(
            arguments.operation,
            Some(Operation::Uninstall { remove_data: false })
        ));
    }

    /// One spelling for component selection, comma separated or repeated,
    /// rather than a second flag meaning the same thing.
    #[test]
    fn components_accept_a_comma_separated_list() {
        let arguments =
            Arguments::try_parse_from(["ae-installer", "update", "--component", "editor,forge"])
                .expect("component list");
        let selected: Vec<&str> = arguments
            .component
            .iter()
            .map(super::Component::as_str)
            .collect();
        assert_eq!(selected, vec!["editor", "forge"]);
    }

    /// `--yes` answers prompts; it must never imply the destructive path.
    #[test]
    fn unattended_runs_do_not_imply_force() {
        let arguments = Arguments::try_parse_from(["ae-installer", "update", "--yes"])
            .expect("unattended update");
        assert!(arguments.automation.yes);
        assert!(!arguments.activation.force);
        assert!(!arguments.activation.skip_retire);
        assert!(!arguments.integrations.skip_shortcuts);
    }

    #[test]
    fn diagnostic_operation_is_available_to_permanent_ae() {
        let arguments = Arguments::try_parse_from(["ae-installer", "diagnose"]).expect("diagnose");
        assert!(matches!(arguments.operation, Some(Operation::Diagnose)));
    }
}
