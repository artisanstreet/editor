//! `ae pair` and `ae devices`: connecting Editors to this machine's Forge.
//! Both work on Forge's data directly, running or not.

use crate::{service, ui};
use anyhow::{Context, Result, bail};
use artisan_api::Timestamp;
use artisan_paths::App;
use forge_http::tls;
use forge_store::admin::Admin;
use std::net::SocketAddr;
use std::time::Duration;

fn data() -> Result<std::path::PathBuf> {
    let data = App::Forge.data()?;
    anyhow::ensure!(
        App::Forge.binary()?.is_file() || data.exists(),
        "Forge isn't installed here; run `ae install`"
    );
    Ok(data)
}

/// The address Editors on this machine reach Forge at.
fn reachable_address() -> SocketAddr {
    let address = service::address();
    if address.ip().is_unspecified() {
        SocketAddr::from(([127, 0, 0, 1], address.port()))
    } else {
        address
    }
}

/// Makes a one-time pairing code. In a terminal it waits until an Editor
/// uses it; `--json` prints it for the Editor and returns.
pub fn pair(json: bool) -> Result<()> {
    let data = data()?;
    let invite = forge_http::invite_offline(&data)
        .with_context(|| format!("make a pairing code for the Forge in {}", data.display()))?;
    let fingerprint = tls::fingerprint_of(&invite.forge_certificate)?;
    let address = reachable_address();
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
    if !service::listening(service::address()) {
        ui::warning("Forge isn't running; the Editor can't pair until it is (`ae forge start`).");
    }
    let before = Admin::open(&data)?.devices()?.len();
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
        let devices = Admin::open(&data)?.devices()?;
        if devices.len() > before {
            let newest = devices
                .iter()
                .max_by_key(|device| device.created_at)
                .map_or("an Editor", |device| device.name.as_str());
            progress.done(format!("Paired {newest}"));
            ui::outro("Done.");
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

pub fn devices(json: bool) -> Result<()> {
    let devices = Admin::open(&data()?)?.devices()?;
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
    let admin = Admin::open(&data)?;
    let devices = admin.devices()?;
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
    drop(admin);
    match Admin::open(&data)?.revoke_device(&id)? {
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
