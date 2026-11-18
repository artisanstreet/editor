use crate::paths::{self, LocateError};
use crate::service::{self, AddressError};
use crate::ui::{self, AskError};
use artisan_api::Timestamp;
use artisan_paths::App;
use forge_http::tls;
use forge_store::admin::{Admin, AdminError, DevicesError, PairCodeError, RevokeError};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ForgeDataError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("Forge isn't installed here; run `ae install`")]
    NotInstalled,
}

#[derive(Debug, thiserror::Error)]
pub enum AdministerError {
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("open {}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("lock {}", path.display())]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("open the Forge database in {}", data.display())]
    Admin {
        data: PathBuf,
        #[source]
        source: Box<AdminError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum PairError {
    #[error(transparent)]
    Data(#[from] ForgeDataError),
    #[error(transparent)]
    Administer(#[from] AdministerError),
    #[error("make a pairing code for the Forge in {}", data.display())]
    Invite {
        data: PathBuf,
        #[source]
        source: forge_http::InviteError,
    },
    #[error("read the fingerprint of Forge's certificate")]
    Fingerprint(#[source] artisan_api::tls::DecodeCertificateError),
    #[error(transparent)]
    Address(#[from] AddressError),
    #[error("the pairing code expired")]
    Expired,
    #[error("check whether an Editor used the pairing code for the Forge in {}", data.display())]
    Pending {
        data: PathBuf,
        #[source]
        source: PairCodeError,
    },
    #[error("list the devices paired with the Forge in {}", data.display())]
    Devices {
        data: PathBuf,
        #[source]
        source: DevicesError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ListDevicesError {
    #[error(transparent)]
    Data(#[from] ForgeDataError),
    #[error("open the Forge database in {}", data.display())]
    Open {
        data: PathBuf,
        #[source]
        source: Box<AdminError>,
    },
    #[error("list the devices paired with the Forge in {}", data.display())]
    List {
        data: PathBuf,
        #[source]
        source: DevicesError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum UnpairError {
    #[error(transparent)]
    List(#[from] ListDevicesError),
    #[error(transparent)]
    Administer(#[from] AdministerError),
    #[error("no paired device has the id {id:?}; see `ae devices`")]
    NoMatch { id: String },
    #[error("{id:?} matches several devices; give more of the id")]
    Ambiguous { id: String },
    #[error("no paired devices")]
    NoDevices,
    #[error(transparent)]
    Ask(#[from] AskError),
    #[error("unpair {id}")]
    Revoke {
        id: String,
        #[source]
        source: Box<RevokeError>,
    },
    #[error("no paired device has the id {id:?}")]
    Gone { id: String },
}

fn data() -> Result<PathBuf, ForgeDataError> {
    let data = paths::data(App::Forge)?;
    if !paths::binary(App::Forge)?.is_file() && !data.exists() {
        return Err(ForgeDataError::NotInstalled);
    }
    Ok(data)
}

struct Administration {
    admin: Admin,
    _data_lock: Option<std::fs::File>,
}

fn administer(data: &Path) -> Result<Administration, AdministerError> {
    let lock = data.join("forge.lock");
    std::fs::create_dir_all(data).map_err(|source| AdministerError::Create {
        path: data.to_owned(),
        source,
    })?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock)
        .map_err(|source| AdministerError::Open {
            path: lock.clone(),
            source,
        })?;
    let admin = |source| AdministerError::Admin {
        data: data.to_owned(),
        source: Box::new(source),
    };
    match file.try_lock() {
        Ok(()) => Ok(Administration {
            admin: Admin::open_exclusive(data, &file).map_err(admin)?,
            _data_lock: Some(file),
        }),
        Err(std::fs::TryLockError::WouldBlock) => Ok(Administration {
            admin: Admin::open(data).map_err(admin)?,
            _data_lock: None,
        }),
        Err(std::fs::TryLockError::Error(source)) => {
            Err(AdministerError::Lock { path: lock, source })
        }
    }
}

fn paired_devices(data: &Path) -> Result<Vec<forge_store::Device>, ListDevicesError> {
    Admin::open_read_only(data)
        .map_err(|source| ListDevicesError::Open {
            data: data.to_owned(),
            source: Box::new(source),
        })?
        .devices()
        .map_err(|source| ListDevicesError::List {
            data: data.to_owned(),
            source,
        })
}

fn reachable_address() -> Result<SocketAddr, AddressError> {
    let address = service::address()?;
    Ok(if address.ip().is_unspecified() {
        SocketAddr::from(([127, 0, 0, 1], address.port()))
    } else {
        address
    })
}

pub fn pair(json: bool) -> Result<(), PairError> {
    let data = data()?;
    let invite = forge_http::invite_offline(&mut administer(&data)?.admin).map_err(|source| {
        PairError::Invite {
            data: data.clone(),
            source,
        }
    })?;
    let fingerprint =
        tls::fingerprint_of(&invite.forge_certificate).map_err(PairError::Fingerprint)?;
    let address = reachable_address()?;
    if json {
        let value = serde_json::json!({
            "code": invite.code,
            "forge_certificate": invite.forge_certificate,
            "forge_fingerprint": fingerprint,
            "expires_at": invite.expires_at,
            "address": address.to_string(),
        });
        println!("{value}");
        return Ok(());
    }
    let paste = format!("{address} {} {fingerprint}", invite.code);
    if !ui::interactive() {
        println!("{paste}");
        eprintln!(
            "Paste this into the Editor (Settings → Hosts → Add). Valid for 10 minutes, once."
        );
        return Ok(());
    }
    ui::intro("Pair an Editor");
    ui::note(
        "Paste this into the Editor: Settings → Hosts → Add",
        format!(
            "{paste}\n\nCode         {}\nForge        {address}\nFingerprint  {fingerprint}",
            invite.code
        ),
    );
    if !service::listening(address) {
        ui::warning("Forge isn't running; the Editor can't pair until it is (`ae forge start`).");
    }
    let code = forge_store::PairCodeHash::of(&invite.code);
    let issued = artisan_api::now();
    let progress = ui::Progress::spinner("Waiting for an Editor");
    loop {
        let left = (invite.expires_at - artisan_api::now()) / 1000;
        if left <= 0 {
            progress.fail("The code expired; run `ae pair` again");
            return Err(PairError::Expired);
        }
        progress.message(format!(
            "Waiting for an Editor (code valid for {}:{:02})",
            left / 60,
            left % 60
        ));
        let administration = administer(&data)?;
        let pending = administration
            .admin
            .pair_code_pending(&code)
            .map_err(|source| PairError::Pending {
                data: data.clone(),
                source,
            })?;
        if !pending {
            let devices = administration
                .admin
                .devices()
                .map_err(|source| PairError::Devices {
                    data: data.clone(),
                    source,
                })?;
            let paired = devices
                .iter()
                .filter(|device| device.created_at >= issued)
                .max_by_key(|device| device.created_at)
                .map_or("an Editor that was already paired", |device| {
                    device.name.as_str()
                });
            progress.done(format!("Paired {paired}"));
            ui::outro("Done.");
            return Ok(());
        }
        drop(administration);
        std::thread::sleep(Duration::from_secs(1));
    }
}

pub fn devices(json: bool) -> Result<(), ListDevicesError> {
    let devices = paired_devices(&data()?)?;
    if json {
        let list: Vec<_> = devices
            .iter()
            .map(|device| {
                serde_json::json!({
                    "id": device.id.as_str(),
                    "name": device.name,
                    "paired_at": device.created_at,
                    "last_seen": device.last_seen,
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(list));
        return Ok(());
    }
    if devices.is_empty() {
        println!("No paired devices. Pair one with `ae pair`.");
        return Ok(());
    }
    let now = artisan_api::now();
    let rows: Vec<Vec<String>> = devices
        .iter()
        .map(|device| {
            vec![
                device.id.as_str().chars().take(12).collect(),
                device.name.clone(),
                ago(now, device.created_at),
                device
                    .last_seen
                    .map_or_else(|| "never".to_owned(), |at| ago(now, at)),
            ]
        })
        .collect();
    println!(
        "{}",
        ui::table(&["ID", "NAME", "PAIRED", "LAST SEEN"], &rows)
    );
    Ok(())
}

pub fn revoke(id: Option<&str>, yes: bool) -> Result<(), UnpairError> {
    let data = data().map_err(ListDevicesError::Data)?;
    let devices = paired_devices(&data)?;
    let (id, name) = match id {
        Some(id) => {
            let matches: Vec<_> = devices
                .iter()
                .filter(|device| device.id.as_str().starts_with(id))
                .collect();
            match matches.as_slice() {
                [device] => (device.id.as_str().to_owned(), device.name.clone()),
                [] => return Err(UnpairError::NoMatch { id: id.to_owned() }),
                _ => return Err(UnpairError::Ambiguous { id: id.to_owned() }),
            }
        }
        None if ui::interactive() => {
            if devices.is_empty() {
                return Err(UnpairError::NoDevices);
            }
            let now = artisan_api::now();
            let mut select = cliclack::select("Unpair which device?");
            for device in &devices {
                let seen = device.last_seen.map_or_else(
                    || "never seen".to_owned(),
                    |at| format!("seen {}", ago(now, at)),
                );
                select = select.item(
                    (device.id.as_str().to_owned(), device.name.clone()),
                    &device.name,
                    seen,
                );
            }
            select.interact().map_err(ui::prompt_error)?
        }
        None => {
            return Err(
                ui::needs_flag("which device to unpair", "an id (see `ae devices`)").into(),
            );
        }
    };
    if !yes && ui::interactive() {
        let go = cliclack::confirm(format!("Unpair {name}? It must pair again to connect."))
            .initial_value(true)
            .interact()
            .map_err(ui::prompt_error)?;
        if !go {
            return Ok(());
        }
    }
    let revoked = administer(&data)?
        .admin
        .revoke_device(&id)
        .map_err(|source| UnpairError::Revoke {
            id: id.clone(),
            source: Box::new(source),
        })?;
    match revoked {
        Some(device) => {
            ui::success(format!(
                "Unpaired {}. Its next request is refused.",
                device.name
            ));
            Ok(())
        }
        None => Err(UnpairError::Gone { id }),
    }
}

pub fn ago(now: Timestamp, at: Timestamp) -> String {
    let seconds = (now - at).max(0) / 1000;
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}
