//! `ae forge serve | pair | import | devices`.

use anyhow::{Context, Result};
use artisan_api::Timestamp;
use forge_http::{DEFAULT_PORT, ServeConfig, tls};
use forge_store::admin::Admin;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

pub fn default_listen() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_PORT)
}

pub fn serve(data_dir: PathBuf, listen: SocketAddr) -> Result<()> {
    tracing::info!(version = crate::LONG_VERSION, "starting Forge");
    tokio::runtime::Runtime::new()
        .context("start the async runtime")?
        .block_on(forge_http::serve(ServeConfig { data_dir, listen }))
}

/// Works beside a running Forge: never opens its `Store`.
pub fn pair(data_dir: &Path, listen: SocketAddr, json: bool) -> Result<()> {
    let invite = forge_http::invite_offline(data_dir).with_context(|| {
        format!(
            "make a pairing code for the Forge in {}",
            data_dir.display()
        )
    })?;
    let fingerprint = tls::fingerprint_of(&invite.forge_certificate)?;
    // An unspecified listen address (0.0.0.0) is reachable on loopback.
    let address = if listen.ip().is_unspecified() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), listen.port())
    } else {
        listen
    };
    if json {
        let value = serde_json::json!({
            "code": invite.code,
            "forge_certificate": invite.forge_certificate,
            "forge_fingerprint": fingerprint,
            "expires_at": invite.expires_at,
            "address": address.to_string(),
        });
        println!("{value}");
    } else {
        println!(
            "Pairing code: {}  (valid for 10 minutes, once)",
            invite.code
        );
        println!("Forge address: {address}");
        println!("Forge fingerprint: {fingerprint}");
        println!("Check that the Editor shows the same fingerprint before you enter the code.");
    }
    Ok(())
}

/// Imports the old Forge. Needs this Forge stopped (it takes its lock); the
/// old one may keep running, its database is only read.
pub fn import(data_dir: &Path, from: Option<PathBuf>) -> Result<()> {
    let old = match from {
        Some(dir) => dir,
        None => legacy_data_dir()?,
    };
    let old_db = old.join("forge.sqlite3");
    anyhow::ensure!(
        old_db.is_file(),
        "no old Forge database at {}",
        old_db.display()
    );
    std::fs::create_dir_all(data_dir).with_context(|| format!("create {}", data_dir.display()))?;
    let _lock =
        forge_http::lock_data_dir(data_dir).context("stop Forge first (`ae forge stop`)")?;
    tokio::runtime::Runtime::new()
        .context("start the async runtime")?
        .block_on(async {
            let store = forge_store::Store::open(data_dir)?;
            println!("Importing threads from {} …", old_db.display());
            let report = forge_store::legacy::import(&old_db, &old, &store)
                .await
                .with_context(|| format!("import {}", old_db.display()))?;
            println!(
                "{} projects, {} threads, {} items, {} runs, {} images, {} global prompt versions",
                report.projects,
                report.threads,
                report.items,
                report.runs,
                report.images,
                report.prompt_versions
            );
            for (reason, count) in &report.skipped {
                println!("  skipped {count}: {reason}");
            }
            let skills = forge_engines::import_legacy_extensions(
                &old,
                &data_dir.join("extensions").join("skills"),
            );
            if !skills.is_empty() {
                println!("Skills: {}", skills.join(", "));
            }
            // Engine homes are copied; only toolchain caches are shared.
            let engines = forge_engines::Engines::new(data_dir)
                .import_legacy(&old.join("toolchain"))
                .await?;
            if !engines.is_empty() {
                let names: Vec<String> = engines.iter().map(ToString::to_string).collect();
                println!("Engines: {}", names.join(", "));
            }
            anyhow::Ok(())
        })
}

/// Explicit offline repair; taking the same data lock as Forge prevents any
/// mutation of an active instance. Dry-run never opens the recovering Store.
pub fn normalize(data_dir: &Path, dry_run: bool) -> Result<()> {
    anyhow::ensure!(
        data_dir.join("forge.sqlite3").is_file(),
        "no Forge database at {}",
        data_dir.display()
    );
    if dry_run {
        let count = forge_store::legacy::inspect_agent_messages(data_dir)?;
        println!(
            "Would normalize {count} agent messages in {}",
            data_dir.display()
        );
        return Ok(());
    }
    let _lock = forge_http::lock_data_dir(data_dir)
        .context("the target Forge database must be offline for normalization")?;
    tokio::runtime::Runtime::new()
        .context("start the async runtime")?
        .block_on(async {
            let store = forge_store::Store::open(data_dir)?;
            let count = forge_store::legacy::normalize_agent_messages(&store).await?;
            println!(
                "Normalized {count} agent messages in {}",
                data_dir.display()
            );
            anyhow::Ok(())
        })
}

/// Where the previous Forge kept its data.
fn legacy_data_dir() -> Result<PathBuf> {
    let base = if cfg!(windows) {
        crate::paths::local_app_data()?
    } else {
        crate::paths::xdg_data()?
    };
    ["Artisan Street", "Artisan Street Dev"]
        .into_iter()
        .map(|name| base.join(name).join("data"))
        .find(|dir| dir.join("forge.sqlite3").is_file())
        .context("found no previous Forge; pass --from <its data directory>")
}

pub fn devices(data_dir: &Path) -> Result<()> {
    let devices = Admin::open(data_dir)?.devices()?;
    if devices.is_empty() {
        println!("No paired devices. Pair one with `ae forge pair`.");
        return Ok(());
    }
    let now = artisan_api::now();
    println!("{:<12} {:<24} {:<14} LAST SEEN", "ID", "NAME", "PAIRED");
    for device in devices {
        let id: String = device.id.as_str().chars().take(12).collect();
        let last_seen = device
            .last_seen
            .map_or_else(|| "never".to_owned(), |at| ago(now, at));
        println!(
            "{id:<12} {:<24} {:<14} {last_seen}",
            device.name,
            ago(now, device.created_at)
        );
    }
    Ok(())
}

pub fn revoke(data_dir: &Path, id: &str) -> Result<()> {
    match Admin::open(data_dir)?.revoke_device(id)? {
        Some(device) => {
            println!(
                "Revoked {} ({}). Its next request is refused.",
                device.name, device.id
            );
            Ok(())
        }
        None => anyhow::bail!("no paired device has the id {id:?}; see `ae forge devices`"),
    }
}

fn ago(now: Timestamp, at: Timestamp) -> String {
    let seconds = (now - at).max(0) / 1000;
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}
