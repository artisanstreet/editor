//! `ae install | update | uninstall | doctor`.

mod doctor;
mod layout;
pub mod platform;
mod release;
pub mod sys;

pub use doctor::doctor;
pub use layout::Root;

use crate::service;
use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use release::{Release, Source};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use sys::Sys;

#[derive(Args)]
pub struct InstallArgs {
    /// What to install; asks when omitted.
    product: Option<Product>,
    /// Install into this WSL distribution (from Windows).
    #[arg(long, value_name = "DISTRO", conflicts_with = "windows")]
    wsl: Option<String>,
    /// Install into Windows (from inside WSL).
    #[arg(long)]
    windows: bool,
    /// A release.json URL or a directory holding a release (default:
    /// `ARTISAN_RELEASES`, else the public feed).
    #[arg(long, value_name = "SOURCE")]
    from: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Product {
    Editor,
    Forge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Host {
    Here,
    Wsl(String),
    Windows,
}

pub fn install(args: &InstallArgs, data_dir: Option<&Path>, dry_run: bool) -> Result<()> {
    let sys = Sys { dry_run };
    let targets = match (&args.wsl, args.windows, args.product) {
        (Some(distro), _, product) => {
            vec![(product.unwrap_or(Product::Forge), Host::Wsl(distro.clone()))]
        }
        (None, true, product) => vec![(product.unwrap_or(Product::Editor), Host::Windows)],
        (None, false, Some(product)) => vec![(product, Host::Here)],
        (None, false, None) => pick_targets()?,
    };
    let source = Source::new(args.from.as_deref());
    runtime()?.block_on(async {
        let here: Vec<Product> = targets
            .iter()
            .filter(|(_, host)| *host == Host::Here)
            .map(|(product, _)| *product)
            .collect();
        if !here.is_empty() {
            install_here(&here, source.clone(), data_dir, sys).await?;
        }
        for (product, host) in &targets {
            match host {
                Host::Here => {}
                Host::Wsl(distro) => install_in_wsl(*product, distro, source.clone(), sys).await?,
                Host::Windows => install_in_windows(*product, source.clone(), sys).await?,
            }
        }
        Ok(())
    })
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Runtime::new().context("start the async runtime")
}

/// Downloads (unless present), verifies, unpacks and activates the release
/// here, then wires up the products.
async fn install_here(
    products: &[Product],
    source: Source,
    data_dir: Option<&Path>,
    sys: Sys,
) -> Result<()> {
    let release = release::resolve(source).await?;
    install_release(products, release, data_dir, sys).await
}

async fn install_release(
    products: &[Product],
    release: Release,
    data_dir: Option<&Path>,
    sys: Sys,
) -> Result<()> {
    let root = Root::locate()?;
    let version = release.version().to_owned();
    let (os, arch) = release::this_platform();
    let artifact = release.artifact(os, arch)?;
    if !root
        .version_dir(&version)
        .join(format!("ae{}", std::env::consts::EXE_SUFFIX))
        .is_file()
    {
        let archive = release.download(artifact).await?;
        root.unpack(&version, &artifact.file, &archive)?;
    }
    let editor = products.contains(&Product::Editor) || root.current_editor().is_some();
    if editor && !root.editor(&version).is_file() {
        bail!("release {version} has no Editor for {os} {arch}");
    }
    let data = match data_dir {
        Some(dir) => dir.to_owned(),
        None => crate::paths::forge_data()?,
    };
    let forge_was_running = service::is_running(&data);

    platform::retire_editors(sys, &root)?;
    root.activate(&version)
        .with_context(|| format!("activate {version} in {}", root.dir.display()))?;
    platform::add_to_path(sys, &root)?;
    if editor {
        platform::integrate_editor(sys, &root.editor(&version))?;
    }
    if products.contains(&Product::Forge) {
        let systemd = service::install(sys, &root.ae(), &data)?;
        if !systemd && cfg!(target_os = "linux") {
            println!(
                "systemd isn't running here, so Forge won't start on its own; run `ae forge start` when needed."
            );
        }
    } else if forge_was_running {
        service::restart(&data, sys.dry_run)?;
    }
    let installed: Vec<&str> = products
        .iter()
        .map(|product| {
            if *product == Product::Editor {
                "the Editor"
            } else {
                "Forge"
            }
        })
        .collect();
    println!(
        "Installed {} {version} in {}.",
        installed.join(" and "),
        root.dir.display()
    );
    Ok(())
}

/// Stages the Linux release on Windows' disk and runs its `ae install`
/// inside the distribution.
async fn install_in_wsl(product: Product, distro: &str, source: Source, sys: Sys) -> Result<()> {
    if product == Product::Editor {
        bail!("the Editor runs on Windows; install Forge into WSL instead");
    }
    let release = release::resolve(source).await?;
    let stage = std::env::temp_dir().join(format!("artisan-wsl-{}", release.version()));
    let ae = stage_ae(&release, "linux", &stage).await?;
    let mut command = std::process::Command::new("wsl.exe");
    command
        .args(["-d", distro, "--exec"])
        .arg(to_wsl_path(&ae, Some(distro))?)
        .args(["install", "forge", "--from"])
        .arg(to_wsl_path(&stage, Some(distro))?);
    run_remote(command, sys, &stage, &format!("WSL ({distro})"))
}

/// From inside WSL: stages the Windows release in Windows' temp directory
/// and runs its `ae.exe install` through interop.
async fn install_in_windows(product: Product, source: Source, sys: Sys) -> Result<()> {
    let release = release::resolve(source).await?;
    let temp = sys::output("cmd.exe", &["/d", "/c", "echo %TEMP%"])
        .context("find Windows' temp directory")?;
    let windows_stage = format!("{}\\artisan-windows-{}", temp.trim(), release.version());
    let stage = PathBuf::from(to_wsl_path(Path::new(&windows_stage), None)?);
    let ae = stage_ae(&release, "windows", &stage).await?;
    let mut command = std::process::Command::new(ae);
    let product = if product == Product::Editor {
        "editor"
    } else {
        "forge"
    };
    command.args(["install", product, "--from", &windows_stage]);
    run_remote(command, sys, &stage, "Windows")
}

async fn stage_ae(release: &Release, os: &str, stage: &Path) -> Result<PathBuf> {
    let artifact = release.artifact(os, std::env::consts::ARCH)?;
    let archive = release.stage(artifact, stage).await?;
    let unpacked = stage.join("unpacked");
    let _ = std::fs::remove_dir_all(&unpacked);
    release::extract(&artifact.file, &archive, &unpacked)?;
    Ok(unpacked.join(if os == "windows" { "ae.exe" } else { "ae" }))
}

fn run_remote(
    mut command: std::process::Command,
    sys: Sys,
    stage: &Path,
    host: &str,
) -> Result<()> {
    if sys.dry_run {
        command.arg("--dry-run");
    }
    let status = command
        .status()
        .with_context(|| format!("run the installer in {host}"))?;
    let _ = std::fs::remove_dir_all(stage);
    if !status.success() {
        bail!("installing in {host} failed ({status})");
    }
    Ok(())
}

/// Let the selected WSL distribution resolve its configured mount layout.
fn to_wsl_path(path: &Path, distro: Option<&str>) -> Result<String> {
    let mut command = if let Some(distro) = distro {
        let mut command = std::process::Command::new("wsl.exe");
        command.args(["-d", distro, "--exec", "wslpath"]);
        command
    } else {
        std::process::Command::new("wslpath")
    };
    let output = command
        .args(["-a", "-u"])
        .arg(path)
        .output()
        .context("convert path through WSL")?;
    anyhow::ensure!(
        output.status.success(),
        "WSL path conversion failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let converted = String::from_utf8(output.stdout).context("WSL path is not UTF-8")?;
    anyhow::ensure!(
        !converted.trim().is_empty(),
        "WSL path conversion returned an empty path"
    );
    Ok(converted.trim().to_owned())
}

fn in_wsl() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WSL_DISTRO_NAME").is_some()
}

/// Asks what to install where. Without a terminal, picks the usual choice.
fn pick_targets() -> Result<Vec<(Product, Host)>> {
    let mut options = Vec::new();
    if cfg!(windows) {
        options.push(("The Editor on this PC", (Product::Editor, Host::Here)));
        for distro in platform::wsl_distros().unwrap_or_default() {
            options.push(("Forge in WSL", (Product::Forge, Host::Wsl(distro))));
        }
        options.push(("Forge on this PC (Windows)", (Product::Forge, Host::Here)));
    } else {
        options.push(("Forge on this machine", (Product::Forge, Host::Here)));
        if in_wsl() {
            options.push(("The Editor on Windows", (Product::Editor, Host::Windows)));
        } else {
            options.push(("The Editor on this machine", (Product::Editor, Host::Here)));
        }
    }
    // The usual setup: the first two (Editor + WSL Forge on Windows, Forge
    // + Windows Editor inside WSL), else just the first.
    let default: Vec<usize> = if options.len() > 1 && (cfg!(windows) || in_wsl()) {
        vec![1, 2]
    } else {
        vec![1]
    };
    if !std::io::stdin().is_terminal() {
        return Ok(default
            .iter()
            .map(|index| options[index - 1].1.clone())
            .collect());
    }
    println!("What should be installed?");
    for (index, (label, (_, host))) in options.iter().enumerate() {
        match host {
            Host::Wsl(distro) => println!("  {}) {label}: {distro}", index + 1),
            _ => println!("  {}) {label}", index + 1),
        }
    }
    let default_text = default
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    loop {
        print!("Choose one or more, e.g. 1,2 [{default_text}]: ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line)? == 0 {
            bail!("nothing chosen");
        }
        let line = line.trim();
        if line.is_empty() {
            return Ok(default
                .iter()
                .map(|index| options[index - 1].1.clone())
                .collect());
        }
        let chosen: Option<Vec<_>> = line
            .split([',', ' '])
            .filter(|part| !part.is_empty())
            .map(|part| {
                part.parse::<usize>()
                    .ok()
                    .and_then(|index| options.get(index.checked_sub(1)?))
                    .map(|option| option.1.clone())
            })
            .collect();
        match chosen {
            Some(chosen) if !chosen.is_empty() => return Ok(chosen),
            _ => println!("Type the numbers of the options, separated by commas."),
        }
    }
}

/// Installs the newest release when it is newer than the current one,
/// keeping what is installed (and restarting a running Forge).
pub fn update(data_dir: Option<&Path>, from: Option<&str>, dry_run: bool) -> Result<()> {
    let root = Root::locate()?;
    let current = root
        .current()
        .context("Artisan isn't installed here; run `ae install`")?;
    runtime()?.block_on(async {
        let release = release::resolve(Source::new(from)).await?;
        if !release::newer(release.version(), &current) {
            println!("Artisan {current} is up to date.");
            return Ok(());
        }
        let products: Vec<Product> = root
            .current_editor()
            .map(|_| Product::Editor)
            .into_iter()
            .collect();
        install_release(&products, release, data_dir, Sys { dry_run }).await
    })
}

/// Removes the programs, shortcuts, PATH entry and service. Forge's data
/// (threads, paired devices, engines) stays.
pub fn uninstall(data_dir: &Path, dry_run: bool) -> Result<()> {
    let sys = Sys { dry_run };
    let root = Root::locate()?;
    service::uninstall(sys, data_dir)?;
    platform::retire_editors(sys, &root)?;
    platform::remove_editor_integration(sys)?;
    platform::remove_from_path(sys, &root)?;
    if cfg!(windows) && !dry_run {
        // This ae.exe may be the one running; Windows deletes it once it exits.
        let bin = root.bin();
        let script = format!(
            "ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"",
            bin.display()
        );
        let _ = platform::spawn_detached(
            std::process::Command::new("cmd.exe").args(["/d", "/c", &script]),
        );
    }
    for path in root.program_paths() {
        if let Err(error) = sys.remove(&path) {
            println!("Couldn't remove {} ({error:#}).", path.display());
        }
    }
    println!(
        "Artisan is uninstalled. Forge's data is still in {}.",
        data_dir.display()
    );
    Ok(())
}
