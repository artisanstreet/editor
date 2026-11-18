use crate::{service, ui};
use anyhow::{Context, Result, bail};
use artisan_api::Timestamp;
use artisan_paths::App;
use forge_http::tls;
use forge_store::admin::Admin;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

fn data() -> Result<std::path::PathBuf> {
    let data = App::Forge.data()?;
    anyhow::ensure!(
        App::Forge.binary()?.is_file() || data.exists(),
        "Forge isn't installed here; run `ae install`"
    );
    Ok(data)
}

struct Administration {
    admin: Admin,
    _data_lock: Option<std::fs::File>,
}

fn administer(data: &Path) -> Result<Administration> {
    let lock = data.join("forge.lock");
    std::fs::create_dir_all(data).with_context(|| format!("create {}", data.display()))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock)
        .with_context(|| format!("open {}", lock.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Administration {
            admin: Admin::open_exclusive(data, &file)?,
            _data_lock: Some(file),
        }),
        Err(std::fs::TryLockError::WouldBlock) => Ok(Administration {
            admin: Admin::open(data)?,
            _data_lock: None,
        }),
        Err(std::fs::TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("lock {}", lock.display()))
        }
    }
}

fn reachable_address() -> Result<SocketAddr> {
    let address = service::address()?;
    Ok(if address.ip().is_unspecified() {
        SocketAddr::from(([127, 0, 0, 1], address.port()))
    } else {
        address
    })
}

pub fn pair(json: bool) -> Result<()> {
    let data = data()?;
    let invite = forge_http::invite_offline(&mut administer(&data)?.admin)
        .with_context(|| format!("make a pairing code for the Forge in {}", data.display()))?;
    let fingerprint = tls::fingerprint_of(&invite.forge_certificate)?;
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
            bail!("the pairing code expired");
        }
        progress.message(format!(
            "Waiting for an Editor (code valid for {}:{:02})",
            left / 60,
            left % 60
        ));
        let administration = administer(&data)?;
        if !administration.admin.pair_code_pending(&code)? {
            let devices = administration.admin.devices()?;
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

pub fn devices(json: bool) -> Result<()> {
    let devices = Admin::open_read_only(&data()?)?.devices()?;
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

pub fn revoke(id: Option<&str>, yes: bool) -> Result<()> {
    let data = data()?;
    let devices = Admin::open_read_only(&data)?.devices()?;
    let (id, name) = match id {
        Some(id) => {
            let matches: Vec<_> = devices
                .iter()
                .filter(|device| device.id.as_str().starts_with(id))
                .collect();
            match matches.as_slice() {
                [device] => (device.id.as_str().to_owned(), device.name.clone()),
                [] => bail!("no paired device has the id {id:?}; see `ae devices`"),
                _ => bail!("{id:?} matches several devices; give more of the id"),
            }
        }
        None if ui::interactive() => {
            anyhow::ensure!(!devices.is_empty(), "no paired devices");
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
            return Err(ui::needs_flag(
                "which device to unpair",
                "an id (see `ae devices`)",
            ));
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
    match administer(&data)?.admin.revoke_device(&id)? {
        Some(device) => {
            ui::success(format!(
                "Unpaired {}. Its next request is refused.",
                device.name
            ));
            Ok(())
        }
        None => bail!("no paired device has the id {id:?}"),
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
