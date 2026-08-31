#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::{
    path::Path,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::files::NativeFileError;

const ACL_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ACL_OUTPUT_BYTES: usize = 64 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct WindowsIdentity {
    sid: String,
    account: String,
}

pub(crate) fn restrict_directory(path: &Path) -> Result<(), NativeFileError> {
    let identity = resolve_current_identity()?;
    let path_text = path.to_string_lossy().into_owned();
    let grant = format!("*{}:(OI)(CI)F", identity.sid);
    let output = run_command(
        "icacls.exe",
        &[
            path_text.clone(),
            "/inheritance:r".to_owned(),
            "/grant:r".to_owned(),
            grant,
        ],
    )?;
    if !output.status.success() {
        return Err(NativeFileError::PrivatePermissions);
    }
    validate_directory_output(path, &path_text, &identity)
}

pub(crate) fn validate_directory(path: &Path) -> Result<(), NativeFileError> {
    let identity = resolve_current_identity()?;
    let path_text = path.to_string_lossy().into_owned();
    validate_directory_output(path, &path_text, &identity)
}

fn validate_directory_output(
    path: &Path,
    path_text: &str,
    identity: &WindowsIdentity,
) -> Result<(), NativeFileError> {
    let output = run_command("icacls.exe", &[path_text.to_owned()])?;
    if !output.status.success() {
        return Err(NativeFileError::PrivatePermissions);
    }
    let text = String::from_utf8(output.stdout).map_err(|_| NativeFileError::PrivatePermissions)?;
    let is_directory = std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false);
    validate_acl(&text, path_text, identity, is_directory)
}

fn run_command(executable: &str, arguments: &[String]) -> Result<Output, NativeFileError> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = command.spawn().map_err(|_| NativeFileError::Io)?;
    let deadline = Instant::now()
        .checked_add(ACL_COMMAND_TIMEOUT)
        .ok_or(NativeFileError::Io)?;
    loop {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(NativeFileError::Io);
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(NativeFileError::Io);
            }
        }
    }
    let output = child.wait_with_output().map_err(|_| NativeFileError::Io)?;
    if output.stdout.len() > MAX_ACL_OUTPUT_BYTES || output.stderr.len() > MAX_ACL_OUTPUT_BYTES {
        return Err(NativeFileError::TooLarge);
    }
    Ok(output)
}

fn resolve_current_identity() -> Result<WindowsIdentity, NativeFileError> {
    let output = run_command(
        "whoami.exe",
        &[
            "/user".to_owned(),
            "/fo".to_owned(),
            "csv".to_owned(),
            "/nh".to_owned(),
        ],
    )?;
    if !output.status.success() {
        return Err(NativeFileError::PrivatePermissions);
    }
    let text = String::from_utf8(output.stdout).map_err(|_| NativeFileError::PrivatePermissions)?;
    let line = text
        .lines()
        .next()
        .ok_or(NativeFileError::PrivatePermissions)?;
    let parts = parse_csv_pair(line).ok_or(NativeFileError::PrivatePermissions)?;
    let identity = WindowsIdentity {
        account: parts.0,
        sid: parts.1,
    };
    if !is_valid_sid(&identity.sid) || !is_valid_account(&identity.account) {
        return Err(NativeFileError::PrivatePermissions);
    }
    Ok(identity)
}

fn parse_csv_pair(line: &str) -> Option<(String, String)> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for character in line.chars() {
        match character {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                parts.push(current.trim().trim_matches('"').trim().to_owned());
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if in_quotes {
        return None;
    }
    parts.push(current.trim().trim_matches('"').trim().to_owned());
    match parts.as_slice() {
        [account, sid] => Some((account.clone(), sid.clone())),
        _ => None,
    }
}

fn is_valid_sid(sid: &str) -> bool {
    if !sid.starts_with("S-1-") || sid.contains(' ') || sid.contains('/') || sid.contains('\\') {
        return false;
    }
    let parts = sid.split('-').collect::<Vec<_>>();
    parts.len() >= 3
        && parts[0] == "S"
        && parts[1] == "1"
        && parts[2..]
            .iter()
            .all(|part| !part.is_empty() && part.parse::<u64>().is_ok())
}

fn is_valid_account(account: &str) -> bool {
    if account.is_empty()
        || account.contains('\0')
        || account.contains('/')
        || account.contains(':')
        || account.contains('"')
        || account.contains(',')
        || account.chars().any(char::is_control)
    {
        return false;
    }
    account.matches('\\').count() == 1
        && account
            .split_once('\\')
            .is_some_and(|(domain, user)| !domain.is_empty() && !user.is_empty())
}

fn validate_acl(
    output: &str,
    queried_path: &str,
    identity: &WindowsIdentity,
    expect_directory: bool,
) -> Result<(), NativeFileError> {
    let ace_lines = collect_ace_lines(output, queried_path)?;
    let [ace] = ace_lines.as_slice() else {
        return Err(NativeFileError::PrivatePermissions);
    };
    let colon = ace.find(':').ok_or(NativeFileError::PrivatePermissions)?;
    let principal = ace[..colon].trim();
    if !principal.eq_ignore_ascii_case(&identity.sid)
        && !principal.eq_ignore_ascii_case(&identity.account)
    {
        return Err(NativeFileError::PrivatePermissions);
    }
    let tokens = parse_flag_tokens(&ace[colon + 1..])?;
    let required: &[&str] = if expect_directory {
        &["f", "oi", "ci"]
    } else {
        &["f"]
    };
    if tokens.len() != required.len()
        || required.iter().any(|required| {
            tokens
                .iter()
                .filter(|candidate| candidate.as_str() == *required)
                .count()
                != 1
        })
        || tokens
            .iter()
            .any(|token| !required.contains(&token.as_str()))
    {
        return Err(NativeFileError::PrivatePermissions);
    }
    Ok(())
}

fn collect_ace_lines(output: &str, queried_path: &str) -> Result<Vec<String>, NativeFileError> {
    if output.len() > MAX_ACL_OUTPUT_BYTES {
        return Err(NativeFileError::TooLarge);
    }
    let queried_lower = queried_path.to_ascii_lowercase();
    let mut first_line = true;
    let mut ace_lines = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("successfully") {
            if is_success_summary(&lower) {
                continue;
            }
            return Err(NativeFileError::PrivatePermissions);
        }
        let mut candidate = trimmed.to_owned();
        if !queried_path.is_empty() && lower.starts_with(&queried_lower) {
            let remainder = trimmed[queried_path.len()..].trim();
            if remainder.is_empty() {
                first_line = false;
                continue;
            }
            candidate = remainder.to_owned();
        } else if first_line && trimmed.contains(":\\") && !trimmed.contains('(') {
            first_line = false;
            continue;
        }
        first_line = false;
        let colon = candidate
            .find(':')
            .ok_or(NativeFileError::PrivatePermissions)?;
        if candidate[..colon].trim().is_empty() {
            return Err(NativeFileError::PrivatePermissions);
        }
        parse_flag_tokens(&candidate[colon + 1..])?;
        ace_lines.push(candidate);
    }
    Ok(ace_lines)
}

fn parse_flag_tokens(flags: &str) -> Result<Vec<String>, NativeFileError> {
    let chars = flags.chars().collect::<Vec<_>>();
    let mut index = 0;
    let mut tokens = Vec::new();
    while index < chars.len() {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        if index == chars.len() || chars[index] != '(' {
            return Err(NativeFileError::PrivatePermissions);
        }
        index += 1;
        let start = index;
        while index < chars.len() && chars[index] != ')' {
            index += 1;
        }
        if index == chars.len() || index == start {
            return Err(NativeFileError::PrivatePermissions);
        }
        tokens.push(
            chars[start..index]
                .iter()
                .collect::<String>()
                .to_ascii_lowercase(),
        );
        index += 1;
    }
    if tokens.is_empty() {
        Err(NativeFileError::PrivatePermissions)
    } else {
        Ok(tokens)
    }
}

fn is_success_summary(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("successfully processed ") else {
        return false;
    };
    let Some((processed, failed)) = rest.split_once("; failed processing ") else {
        return false;
    };
    let Some(processed) = processed.strip_suffix(" files") else {
        return false;
    };
    let failed = failed.strip_suffix('.').unwrap_or(failed);
    let Some(failed) = failed.strip_suffix(" files") else {
        return false;
    };
    let (Ok(processed), Ok(failed)) = (processed.parse::<u64>(), failed.parse::<u64>()) else {
        return false;
    };
    processed == 1 && failed == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> WindowsIdentity {
        WindowsIdentity {
            sid: "S-1-5-21-42".to_owned(),
            account: "ARTISAN\\runner".to_owned(),
        }
    }

    #[test]
    fn exact_directory_acl_is_accepted_and_everything_else_is_rejected() {
        let identity = identity();
        let accepted = r#"C:\\private ARTISAN\runner:(OI)(CI)(F)
Successfully processed 1 files; Failed processing 0 files."#;
        assert!(validate_acl(accepted, r#"C:\\private"#, &identity, true).is_ok());
        for invalid in [
            r#"C:\\private Everyone:(OI)(CI)(F)
Successfully processed 1 files; Failed processing 0 files."#,
            r#"C:\\private ARTISAN\runner:(OI)(CI)(F)(I)
Successfully processed 1 files; Failed processing 0 files."#,
            r#"C:\\private ARTISAN\runner:(F)
Successfully processed 1 files; Failed processing 0 files."#,
        ] {
            assert!(validate_acl(invalid, r#"C:\\private"#, &identity, true).is_err());
        }
    }
}
