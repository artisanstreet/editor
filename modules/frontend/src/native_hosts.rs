//! Private host registration and connection identities, independent of editor windows.
use crate::native_command_menu::{CommandMenuAction, CommandMenuEntry, CommandMenuGroup};
use artisan_editor_cli::credentials::{ForgeCredentialError, hosts};
use std::{
    io::Read as _,
    path::{Path, PathBuf},
};

pub(crate) fn selected_home() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--host-home" {
            return Some(args.next().map(PathBuf::from).unwrap_or_default());
        }
    }
    // An explicit CLI host always wins; otherwise reopen the launch-time
    // reopen-host hint when its credentials still decode, else fall back to local.
    // A newer incarnation of the same host may have superseded the recorded home.
    let home = hosts::current_home(crate::editor_settings::startup().reopen_host()?);
    hosts::read_private(&home, "host.json")
        .and_then(|bytes| hosts::HostInvitation::decode(&bytes))
        .map(|_| home)
        .ok()
}

mod catalog;
pub(crate) mod wsl;
pub(crate) use catalog::{label, refresh, same_host};

pub(crate) fn group() -> CommandMenuGroup {
    let mut entries = vec![CommandMenuEntry {
        id: "local-host".into(),
        title: "This computer".into(),
        keywords: vec![],
        action: CommandMenuAction::OpenHost { home: None },
    }];
    {
        let hosts = catalog::entries();
        entries.extend(hosts.into_iter().map(|(name, home)| CommandMenuEntry {
            id: format!("host-{}", home.to_string_lossy()),
            title: name,
            keywords: vec!["machine remote".into()],
            action: CommandMenuAction::OpenHost { home: Some(home) },
        }));
    }
    entries.push(CommandMenuEntry {
        id: "add-host".into(),
        title: "Add new host".into(),
        keywords: vec!["machine WSL remote connect".into()],
        action: CommandMenuAction::AddHost,
    });
    CommandMenuGroup::new("hosts", "Machines", entries)
}

pub(crate) fn import(path: &Path) -> Result<PathBuf, ForgeCredentialError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take((hosts::MAX_INVITATION_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|_| ForgeCredentialError::Provisioning)?;
    let result = hosts::import(&bytes).and_then(|home| {
        if !home.join("credentials/source.json").exists() {
            let source = serde_json::to_vec(&path.to_path_buf())
                .map_err(|_| ForgeCredentialError::ManifestMalformed)?;
            hosts::install_private(&home, "source.json", &source)?;
        }
        Ok(home)
    });
    // The invitation contains a bootstrap secret; don't retain its serialized bytes.
    bytes.fill(0);
    if let Ok(home) = &result {
        refresh(Some(home));
    }
    result
}

/// Runs explicit headless host diagnostics without constructing a GPUI application.
pub(crate) fn headless() -> Option<std::process::ExitCode> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--import-host") {
        let result = args
            .get(1)
            .ok_or_else(|| "invitation path required".to_owned())
            .and_then(|path| {
                // Credential errors name paths and stages only, never secrets.
                import(Path::new(path)).map_err(|error| format!("host import failed: {error}"))
            });
        return Some(match result {
            Ok(home) => {
                println!("{}", home.display());
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::ExitCode::FAILURE
            }
        });
    }
    if !args.iter().any(|arg| arg == "--probe-host") {
        return None;
    }
    Some(match probe() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Host probe failed: {error}");
            std::process::ExitCode::FAILURE
        }
    })
}

fn probe() -> Result<(), &'static str> {
    use crate::native_transport_service::{
        NativeTransportEvent, NativeTransportService, ServiceStopStatus,
    };
    use std::time::{Duration, Instant};
    if selected_home().is_none() {
        return Err("--host-home is required");
    }
    let service = NativeTransportService::spawn().map_err(|_| "service start failed")?;
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut received_catalog = false;
    let mut failed = false;
    while Instant::now() < deadline {
        match service.try_recv() {
            Ok(Some(NativeTransportEvent::Projects(_))) => {
                received_catalog = true;
                let _ = service.request_shutdown();
            }
            Ok(Some(NativeTransportEvent::Failed(failure))) => {
                eprintln!("{failure:?}");
                failed = true;
                let _ = service.request_shutdown();
            }
            Ok(Some(NativeTransportEvent::Stopped(status))) => {
                let _ = service.join();
                if received_catalog && !failed && status == ServiceStopStatus::Clean {
                    println!(
                        "Authenticated QUIC connection, project query, and clean disconnect passed."
                    );
                    return Ok(());
                }
                return Err("authentication, project query, or cleanup failed");
            }
            Err(_) if service.is_finished() => break,
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = service.request_shutdown();
    Err("timed out before a successful project query and disconnect")
}

/// Refreshes endpoint/incarnation from the original invitation, retaining the imported certificate pin.
///
/// Importing a newer incarnation retires superseded registrations, so a home
/// held by the caller may have been replaced by its successor.
pub(crate) fn resolve_home(home: &Path) -> Result<PathBuf, ForgeCredentialError> {
    let home = hosts::current_home(home);
    let original = hosts::HostInvitation::decode(&hosts::read_private(&home, "host.json")?)?;
    if !home.join("credentials/source.json").exists() {
        return Ok(home);
    }
    let source: PathBuf = serde_json::from_slice(&hosts::read_private(&home, "source.json")?)
        .map_err(|_| ForgeCredentialError::ManifestMalformed)?;
    if !source.exists() {
        return Ok(home);
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&source)
        .and_then(|file| {
            file.take((hosts::MAX_INVITATION_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|_| ForgeCredentialError::Provisioning)?;
    let fresh = hosts::HostInvitation::decode(&bytes);
    bytes.fill(0);
    let fresh = fresh?;
    if fresh.id()? != original.id()? {
        return Err(ForgeCredentialError::ManifestMalformed);
    }
    let resolved = hosts::import(&fresh.encode()?)?;
    if !resolved.join("credentials/source.json").exists() {
        let bytes =
            serde_json::to_vec(&source).map_err(|_| ForgeCredentialError::ManifestMalformed)?;
        hosts::install_private(&resolved, "source.json", &bytes)?;
    }
    Ok(resolved)
}

/// Presentation data for a registered machine; never includes credentials.
#[derive(Clone)]
pub(crate) struct HostPresentation {
    pub subtitle: String,
    pub avatar_seed: String,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub wsl_distribution: Option<String>,
}

pub(crate) fn presentation(home: Option<&Path>) -> HostPresentation {
    if let Some(home) = home {
        return catalog::presentation(home).unwrap_or_else(|| HostPresentation {
            wsl_distribution: None,
            subtitle: "Address unavailable".into(),
            avatar_seed: home.to_string_lossy().into_owned(),
        });
    }
    read_presentation(None)
}

fn read_presentation(home: Option<&Path>) -> HostPresentation {
    let Some(home) = home else {
        return HostPresentation {
            wsl_distribution: None,
            subtitle: local_host_subtitle(
                cfg!(target_os = "linux") && std::env::var_os("WSL_DISTRO_NAME").is_some(),
            )
            .into(),
            avatar_seed: std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "local-machine".into()),
        };
    };
    let host = hosts::read_private(home, "host.json")
        .and_then(|bytes| hosts::HostInvitation::decode(&bytes));
    let Ok(host) = host else {
        return HostPresentation {
            wsl_distribution: None,
            subtitle: "Address unavailable".into(),
            avatar_seed: home.to_string_lossy().into_owned(),
        };
    };
    let distribution = if cfg!(target_os = "windows") {
        hosts::read_private(home, "source.json")
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PathBuf>(&bytes).ok())
            .and_then(|source| wsl::distribution(&source.to_string_lossy()))
    } else {
        None
    };
    HostPresentation {
        subtitle: if distribution.is_some() {
            "This computer on WSL".into()
        } else {
            host.endpoint.ip().to_string()
        },
        wsl_distribution: distribution,
        avatar_seed: host
            .id()
            .unwrap_or_else(|_| home.to_string_lossy().into_owned()),
    }
}

fn local_host_subtitle(wsl: bool) -> &'static str {
    if wsl {
        "This computer on WSL"
    } else {
        "This computer"
    }
}

#[cfg(test)]
fn is_local_wsl_source(source: &str) -> bool {
    let path = source.replace('/', "\\").to_ascii_lowercase();
    path.starts_with("\\\\wsl.localhost\\") || path.starts_with("\\\\wsl$\\")
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    #[test]
    fn local_labels_and_wsl_provenance_are_explicit() {
        assert_eq!(local_host_subtitle(false), "This computer");
        assert_eq!(local_host_subtitle(true), "This computer on WSL");
        assert!(is_local_wsl_source(
            r"\\wsl.localhost\Ubuntu\home\user\host.json"
        ));
        assert!(is_local_wsl_source(r"\\WSL$\Ubuntu\host.json"));
        assert!(!is_local_wsl_source(r"\\server\Ubuntu\host.json"));
        assert!(!is_local_wsl_source(r"C:\Ubuntu\host.json"));
    }
}
