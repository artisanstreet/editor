mod archive;
mod error;
mod install;
mod integrations;
mod manifest;
mod platform;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use error::{BootstrapError, Result};
use install::{InstallOptions, diagnose, install, repair, uninstall};
use manifest::TrustKey;
use platform::Platform;
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
    #[arg(long, env = "ARTISAN_BOOTSTRAP_PUBLIC_KEY", global = true)]
    public_key: Option<String>,

    /// Components to install. Defaults to Editor, Forge, and the permanent ae CLI.
    #[arg(long, value_enum, global = true)]
    component: Vec<Component>,

    /// Per-user installation root.
    #[arg(long, env = "ARTISAN_INSTALL_ROOT", global = true)]
    install_root: Option<PathBuf>,

    /// Do not invoke permanent ae setup/doctor/status after activation.
    #[arg(long, global = true)]
    skip_setup: bool,

    /// Ask a detached helper to remove this temporary executable after exit.
    #[arg(long, global = true)]
    self_cleanup: bool,
}

#[derive(Debug, Subcommand)]
enum Operation {
    /// Verify bootstrap-owned integrations without changing them.
    #[command(hide = true)]
    Diagnose,
    /// Install the latest signed release without first-time profile setup.
    Update,
    /// Restore bootstrap-owned launchers, PATH integration, and installation health.
    Repair,
    /// Remove installed binaries and owned integrations.
    Uninstall {
        /// Also permanently remove Forge profiles, projects, and conversations.
        #[arg(long)]
        remove_data: bool,
    },
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("artisan bootstrap failed: {error}");
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
        run_setup: !arguments.skip_setup,
    })
    .await?;

    if arguments.self_cleanup {
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
    }
}

fn schedule_self_cleanup() -> Result<()> {
    let executable = std::env::current_exe().map_err(BootstrapError::CurrentExecutable)?;
    let temporary_root = std::env::temp_dir()
        .canonicalize()
        .map_err(BootstrapError::TemporaryDirectory)?;
    let executable = executable
        .canonicalize()
        .map_err(BootstrapError::CurrentExecutable)?;
    if !executable.starts_with(&temporary_root) {
        return Err(BootstrapError::UnsafeSelfCleanup(executable));
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        std::process::Command::new("cmd.exe")
            .args([
                "/d",
                "/s",
                "/c",
                "ping 127.0.0.1 -n 3 > nul & del /f /q \"%ARTISAN_BOOTSTRAP_DELETE%\"",
            ])
            .env("ARTISAN_BOOTSTRAP_DELETE", &executable)
            .creation_flags(DETACHED_PROCESS)
            .spawn()
            .map_err(BootstrapError::CleanupHelper)?;
    }
    #[cfg(unix)]
    {
        std::process::Command::new("sh")
            .args([
                "-c",
                "sleep 1; rm -f -- \"$1\"",
                "artisan-bootstrap-cleanup",
                executable
                    .to_str()
                    .ok_or_else(|| BootstrapError::NonUtf8Path(executable.clone()))?,
            ])
            .spawn()
            .map_err(BootstrapError::CleanupHelper)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Arguments, Operation};

    #[test]
    fn permanent_cli_maintenance_argument_order_is_supported() {
        let arguments = Arguments::try_parse_from([
            "artisan-bootstrap",
            "update",
            "--install-root",
            "/tmp/artisan",
        ])
        .expect("maintenance invocation");
        assert!(matches!(arguments.operation, Some(Operation::Update)));
    }

    #[test]
    fn data_removal_is_explicit() {
        let arguments =
            Arguments::try_parse_from(["artisan-bootstrap", "uninstall"]).expect("uninstall");
        assert!(matches!(
            arguments.operation,
            Some(Operation::Uninstall { remove_data: false })
        ));
    }

    #[test]
    fn diagnostic_operation_is_available_to_permanent_ae() {
        let arguments =
            Arguments::try_parse_from(["artisan-bootstrap", "diagnose"]).expect("diagnose");
        assert!(matches!(arguments.operation, Some(Operation::Diagnose)));
    }
}
