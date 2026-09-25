use std::path::PathBuf;

use artisan_install::{
    InstallIntegrationOptions, InstallOptions, LOCAL_CHANNEL, Platform, ReleaseSource, Result,
    RetirementPolicy, TrustKey, diagnose, install, local_trust, prepare_update, prune, repair,
    schedule_self_cleanup, uninstall,
};
use clap::{Args, Parser, Subcommand};
use url::Url;

const DEFAULT_MANIFEST: &str = "https://github.com/sandersonstabo/artisan-editor/releases/latest/download/release-manifest.json";

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

    /// Leave the user PATH alone. For a side-by-side install whose `ae` must
    /// not shadow the primary installation's.
    #[arg(long, global = true)]
    skip_path: bool,
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
#[command(version = artisan_build_info::version_line(), about)]
struct Arguments {
    #[command(subcommand)]
    operation: Option<Operation>,

    /// Signed release manifest URL.
    #[arg(long, default_value = DEFAULT_MANIFEST, global = true)]
    manifest_url: Url,

    /// Detached signature URL. Defaults to the manifest URL with `.json` replaced by `.sig`.
    #[arg(long, global = true)]
    signature_url: Option<Url>,

    /// Install from a manifest URL, a release directory (release-tool output),
    /// or an unpacked payload with a signed tree manifest, instead of
    /// --manifest-url.
    #[arg(long, global = true, value_name = "URL_OR_DIRECTORY", conflicts_with_all = ["manifest_url", "signature_url"])]
    from: Option<String>,

    /// Channel the release must belong to. `dev` releases are verified with
    /// the installation's own local signing key instead of the release key.
    #[arg(long, global = true, value_parser = ["stable", "beta", "nightly", "dev"])]
    channel: Option<String>,

    /// Ed25519 public key as 32-byte hexadecimal. Development builds only:
    /// release builds refuse this override and use their embedded release key.
    #[arg(long, env = "ARTISAN_INSTALLER_PUBLIC_KEY", global = true)]
    public_key: Option<String>,

    #[command(flatten)]
    automation: AutomationArguments,

    #[command(flatten)]
    integrations: IntegrationArguments,

    #[command(flatten)]
    activation: ActivationArguments,

    /// Per-user installation root.
    #[arg(long, global = true)]
    install_root: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Operation {
    /// Verify bootstrap-owned integrations without changing them.
    #[command(hide = true)]
    Diagnose,
    /// Close the editor and retire Forge before a local release build begins.
    #[command(hide = true)]
    PrepareUpdate,
    /// Install the latest signed release without first-time Forge setup.
    Update,
    /// Restore bootstrap-owned launchers, PATH integration, and installation health.
    Repair,
    /// Remove superseded versions, keeping the active one and the most recent
    /// others for rollback. Versions a running Editor or Forge uses are kept.
    Prune {
        /// Inactive versions to keep.
        #[arg(long, default_value_t = 2)]
        keep: usize,
    },
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
    let install_root_env = std::env::var_os("ARTISAN_INSTALL_ROOT").map(PathBuf::from);
    let artisan_home_env = std::env::var_os("ARTISAN_HOME").map(PathBuf::from);
    let root = artisan_install::resolve_install_root(
        arguments.install_root.as_deref(),
        install_root_env.as_deref(),
        artisan_home_env.as_deref(),
    )?;
    #[cfg(debug_assertions)]
    artisan_install::forbid_default_install_root(&root)?;

    if let Some(operation) = arguments.operation.as_ref() {
        match operation {
            Operation::Diagnose => diagnose(&root)?,
            Operation::PrepareUpdate => prepare_update(
                &root,
                (!arguments.activation.skip_retire).then_some(RetirementPolicy {
                    force: arguments.activation.force,
                    close_editors_first: false,
                }),
            )?,
            Operation::Update => {
                install(make_install_options(&arguments, platform, root, false)?).await?;
            }
            Operation::Repair => repair(&root)?,
            Operation::Prune { keep } => {
                let report = prune(&root, *keep)?;
                for version in &report.removed {
                    println!("removed {version}");
                }
                for version in &report.in_use {
                    println!("kept {version} (in use)");
                }
            }
            Operation::Uninstall { remove_data } => uninstall(&root, *remove_data)?,
        }
        return Ok(());
    }

    let run_setup = !arguments.activation.skip_setup;
    install(make_install_options(&arguments, platform, root, run_setup)?).await?;

    if arguments.automation.self_cleanup {
        schedule_self_cleanup()?;
    }
    Ok(())
}

fn make_install_options(
    arguments: &Arguments,
    platform: Platform,
    install_root: PathBuf,
    run_setup: bool,
) -> Result<InstallOptions> {
    let source = match &arguments.from {
        Some(location) => ReleaseSource::from_location(location)?,
        None => match &arguments.signature_url {
            Some(signature_url) => ReleaseSource::Remote {
                manifest_url: arguments.manifest_url.clone(),
                signature_url: signature_url.clone(),
            },
            None => ReleaseSource::remote(arguments.manifest_url.clone())?,
        },
    };
    // The dev channel trusts only the installation's own local key; every
    // other channel trusts this build's release anchor.
    let trust = if arguments.channel.as_deref() == Some(LOCAL_CHANNEL) {
        local_trust(&install_root)?
    } else {
        TrustKey::resolve(arguments.public_key.as_deref())?
    };
    Ok(InstallOptions {
        source,
        platform,
        install_root,
        trust,
        expected_channel: arguments.channel.clone(),
        run_setup,
        restore_forge: true,
        integrations: InstallIntegrationOptions {
            register_protocol: !arguments.integrations.skip_protocol,
            register_shortcuts: !arguments.integrations.skip_shortcuts,
            register_path: !arguments.integrations.skip_path,
        },
        retirement: (!arguments.activation.skip_retire).then_some(RetirementPolicy {
            force: arguments.activation.force,
            close_editors_first: false,
        }),
    })
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
    fn ordinary_install_invocation_is_supported_without_component_flags() {
        let arguments =
            Arguments::try_parse_from(["ae-installer", "--install-root", "/tmp/artisan"])
                .expect("ordinary install invocation");
        assert!(arguments.operation.is_none());
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

    #[test]
    fn former_component_selection_invocations_are_rejected() {
        for invocation in [
            ["ae-installer", "update", "--component", "editor,forge"],
            ["ae-installer", "update", "--component", "editor,forge,cli"],
        ] {
            assert!(
                Arguments::try_parse_from(invocation).is_err(),
                "former component invocation must be rejected"
            );
        }
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

    #[test]
    fn prepare_update_is_a_manifest_free_lifecycle_operation() {
        let arguments = Arguments::try_parse_from(["ae-installer", "prepare-update", "--yes"])
            .expect("prepare update");
        assert!(matches!(
            arguments.operation,
            Some(Operation::PrepareUpdate)
        ));
        assert!(!arguments.activation.force);
    }
}
