use super::payload::{self, Staged};
use anyhow::{Context as _, Result, bail};
use std::io::{BufRead as _, BufReader};
use std::process::{Command, Stdio};

fn command(staged: &Staged, arguments: &[String]) -> Result<Command> {
    let mut command = Command::new(&staged.ae);
    command
        .args(arguments)
        .arg("--bundled-editor")
        .arg(std::env::current_exe().context("find this program")?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(linux_ae) = &staged.linux_ae {
        command.arg("--bundled-linux-ae").arg(linux_ae);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(command)
}

#[must_use]
pub fn failure(lines: &[String]) -> String {
    lines
        .iter()
        .rev()
        .find_map(|line| line.strip_prefix("error: "))
        .or_else(|| lines.last().map(String::as_str))
        .unwrap_or("the installer stopped without saying why")
        .to_owned()
}

pub fn run(arguments: &[String], mut progress: impl FnMut(&str)) -> Result<()> {
    let staged = payload::stage()?;
    let mut child = command(&staged, arguments)?
        .spawn()
        .with_context(|| format!("start {}", staged.ae.display()))?;
    let mut lines = Vec::new();
    if let Some(stderr) = child.stderr.take() {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let line = line.trim().to_owned();
            if !line.is_empty() {
                progress(&line);
                lines.push(line);
            }
        }
    }
    let status = child.wait().context("wait for the installer")?;
    if !status.success() {
        bail!(failure(&lines));
    }
    payload::write_notices()
}

pub fn headless(arguments: &[String]) -> Result<()> {
    let mut all = vec!["install".to_owned(), "--apply".to_owned()];
    all.extend_from_slice(arguments);
    run(&all, |line| eprintln!("{line}"))
}
