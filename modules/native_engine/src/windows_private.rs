#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::{
    path::Path,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::io::NativeFileError;

const ACL_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ACL_OUTPUT_BYTES: usize = 64 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Debug, Eq, PartialEq)]
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
            grant.clone(),
        ],
    )?;
    validate_icacls_mutation_output(&output)?;

    let query = query_directory_acl(&path_text)?;
    let removals = plan_acl_removals(&query, &identity, &path_text)?;
    if !removals.is_empty() {
        for principal in removals {
            let removal = icacls_remove_argument(&principal)?;
            let output = run_command(
                "icacls.exe",
                &[path_text.clone(), "/remove".to_owned(), removal],
            )?;
            validate_icacls_mutation_output(&output)?;
        }

        let output = run_command(
            "icacls.exe",
            &[path_text.clone(), "/grant:r".to_owned(), grant],
        )?;
        validate_icacls_mutation_output(&output)?;
    }

    let final_identity = resolve_current_identity()?;
    if !same_identity(&identity, &final_identity) {
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
    let text = query_directory_acl(path_text)?;
    let is_directory = std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir());
    validate_acl(&text, path_text, identity, is_directory)
}

fn query_directory_acl(path_text: &str) -> Result<String, NativeFileError> {
    let output = run_command("icacls.exe", &[path_text.to_owned()])?;
    if !output.status.success() {
        return Err(NativeFileError::PrivatePermissions);
    }
    String::from_utf8(output.stdout).map_err(|_| NativeFileError::PrivatePermissions)
}

fn validate_icacls_mutation_output(output: &Output) -> Result<(), NativeFileError> {
    if !output.status.success() {
        return Err(NativeFileError::PrivatePermissions);
    }
    let text =
        std::str::from_utf8(&output.stdout).map_err(|_| NativeFileError::PrivatePermissions)?;
    let mut saw_summary = false;
    for line in text.lines() {
        if line.chars().any(char::is_control) {
            return Err(NativeFileError::PrivatePermissions);
        }
        let lower = line.trim().to_ascii_lowercase();
        if lower.starts_with("successfully") {
            if !is_success_summary(&lower) || saw_summary {
                return Err(NativeFileError::PrivatePermissions);
            }
            saw_summary = true;
        }
    }
    Ok(())
}

fn same_identity(left: &WindowsIdentity, right: &WindowsIdentity) -> bool {
    left.sid.eq_ignore_ascii_case(&right.sid) && left.account.eq_ignore_ascii_case(&right.account)
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
        || account.contains('*')
        || account.contains('(')
        || account.contains(')')
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
    let principal = ace.principal.as_str();
    if !principal.eq_ignore_ascii_case(&identity.sid)
        && !principal.eq_ignore_ascii_case(&identity.account)
    {
        return Err(NativeFileError::PrivatePermissions);
    }
    let required: &[&str] = if expect_directory {
        &["f", "oi", "ci"]
    } else {
        &["f"]
    };
    if ace.tokens.len() != required.len()
        || required.iter().any(|required| {
            ace.tokens
                .iter()
                .filter(|candidate| candidate.as_str() == *required)
                .count()
                != 1
        })
        || ace
            .tokens
            .iter()
            .any(|token| !required.contains(&token.as_str()))
    {
        return Err(NativeFileError::PrivatePermissions);
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct ParsedAce {
    principal: String,
    tokens: Vec<String>,
}

fn collect_ace_lines(output: &str, queried_path: &str) -> Result<Vec<ParsedAce>, NativeFileError> {
    if output.len() > MAX_ACL_OUTPUT_BYTES {
        return Err(NativeFileError::TooLarge);
    }
    let queried_lower = queried_path.to_ascii_lowercase();
    let mut first_line = true;
    let mut saw_summary = false;
    let mut ace_lines = Vec::new();
    for line in output.lines() {
        if line.chars().any(char::is_control) {
            return Err(NativeFileError::PrivatePermissions);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("successfully") {
            if !is_success_summary(&lower) || saw_summary {
                return Err(NativeFileError::PrivatePermissions);
            }
            saw_summary = true;
            continue;
        }
        let mut candidate = trimmed.to_owned();
        if !queried_path.is_empty() && lower.starts_with(&queried_lower) {
            let remainder = &trimmed[queried_path.len()..];
            if remainder.is_empty() {
                first_line = false;
                continue;
            }
            if !remainder.chars().next().is_some_and(char::is_whitespace) {
                return Err(NativeFileError::PrivatePermissions);
            }
            candidate = remainder.trim().to_owned();
        } else if first_line && is_windows_path_header(trimmed) && !trimmed.contains('(') {
            if !queried_path.is_empty() {
                return Err(NativeFileError::PrivatePermissions);
            }
            first_line = false;
            continue;
        }
        first_line = false;
        ace_lines.push(parse_ace_line(&candidate)?);
    }
    Ok(ace_lines)
}

fn is_windows_path_header(line: &str) -> bool {
    let bytes = line.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
        || line.starts_with(r"\\")
}

fn parse_ace_line(line: &str) -> Result<ParsedAce, NativeFileError> {
    let colon = line.find(':').ok_or(NativeFileError::PrivatePermissions)?;
    let principal = line[..colon].trim();
    if principal.is_empty() || principal.chars().any(char::is_control) {
        return Err(NativeFileError::PrivatePermissions);
    }
    let tokens = parse_flag_tokens(&line[colon + 1..])?;
    Ok(ParsedAce {
        principal: principal.to_owned(),
        tokens,
    })
}

fn parse_flag_tokens(flags: &str) -> Result<Vec<String>, NativeFileError> {
    if flags.chars().any(char::is_control) {
        return Err(NativeFileError::PrivatePermissions);
    }
    let chars = flags.chars().collect::<Vec<_>>();
    let mut index = 0;
    let mut tokens = Vec::new();
    while index < chars.len() {
        while index < chars.len() && chars[index] == ' ' {
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
        let token = chars[start..index].iter().collect::<String>();
        if !token.bytes().all(|byte| byte.is_ascii_alphanumeric())
            || tokens
                .iter()
                .any(|candidate: &String| candidate.eq_ignore_ascii_case(&token))
        {
            return Err(NativeFileError::PrivatePermissions);
        }
        tokens.push(token.to_ascii_lowercase());
        index += 1;
    }
    if tokens.is_empty() {
        Err(NativeFileError::PrivatePermissions)
    } else {
        Ok(tokens)
    }
}

fn plan_acl_removals(
    output: &str,
    identity: &WindowsIdentity,
    queried_path: &str,
) -> Result<Vec<String>, NativeFileError> {
    if !is_valid_sid(&identity.sid)
        || !is_valid_account(&identity.account)
        || identity.sid.eq_ignore_ascii_case(&identity.account)
    {
        return Err(NativeFileError::PrivatePermissions);
    }
    let ace_lines = collect_ace_lines(output, queried_path)?;
    if ace_lines.is_empty() {
        return Err(NativeFileError::PrivatePermissions);
    }

    let mut removals = Vec::new();
    let mut current_identity_count = 0;
    for ace in ace_lines {
        if ace
            .tokens
            .iter()
            .any(|token| matches!(token.as_str(), "i" | "deny" | "allow"))
        {
            return Err(NativeFileError::PrivatePermissions);
        }

        let is_current_identity = ace.principal.eq_ignore_ascii_case(&identity.sid)
            || ace.principal.eq_ignore_ascii_case(&identity.account);
        if is_current_identity {
            current_identity_count += 1;
            if current_identity_count > 1 {
                return Err(NativeFileError::PrivatePermissions);
            }
            continue;
        }

        if !is_safe_acl_principal(&ace.principal)
            || removals
                .iter()
                .any(|candidate: &String| candidate.eq_ignore_ascii_case(&ace.principal))
        {
            return Err(NativeFileError::PrivatePermissions);
        }
        removals.push(ace.principal);
    }

    if current_identity_count != 1 {
        return Err(NativeFileError::PrivatePermissions);
    }
    Ok(removals)
}

fn is_safe_acl_principal(principal: &str) -> bool {
    if principal.is_empty()
        || principal.chars().any(|character| {
            character.is_control() || matches!(character, '*' | '(' | ')' | ':' | ',' | '"')
        })
    {
        return false;
    }
    if is_valid_sid(principal) || is_valid_account(principal) {
        return true;
    }
    matches!(
        principal.to_ascii_lowercase().as_str(),
        "everyone"
            | "creator owner"
            | "owner rights"
            | "all application packages"
            | "all restricted application packages"
            | "authenticated users"
            | "anonymous logon"
            | "interactive"
            | "local service"
            | "network service"
            | "administrators"
            | "users"
            | "system"
    )
}

fn icacls_remove_argument(principal: &str) -> Result<String, NativeFileError> {
    if !is_safe_acl_principal(principal) {
        return Err(NativeFileError::PrivatePermissions);
    }
    if is_valid_sid(principal) {
        Ok(format!("*{principal}"))
    } else {
        Ok(principal.to_owned())
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
        let accepted = r"C:\\private ARTISAN\runner:(OI)(CI)(F)
Successfully processed 1 files; Failed processing 0 files.";
        assert!(validate_acl(accepted, r"C:\\private", &identity, true).is_ok());
        for invalid in [
            r"C:\\private Everyone:(OI)(CI)(F)
Successfully processed 1 files; Failed processing 0 files.",
            r"C:\\private ARTISAN\runner:(OI)(CI)(F)(I)
Successfully processed 1 files; Failed processing 0 files.",
            r"C:\\private ARTISAN\runner:(F)
Successfully processed 1 files; Failed processing 0 files.",
        ] {
            assert!(validate_acl(invalid, r"C:\\private", &identity, true).is_err());
        }
    }

    #[test]
    fn acl_planner_converges_multiple_safe_explicit_principals() {
        let identity = identity();
        let path = r"C:\\private";
        let output = format!(
            "{path} {}:(OI)(CI)(F)\nDOMAIN\\Runner:(OI)(CI)(F)\nBUILTIN\\Administrators:(F)\nEveryone:(F)",
            identity.sid
        );
        assert_eq!(
            plan_acl_removals(&output, &identity, path).unwrap(),
            vec![
                "DOMAIN\\Runner".to_owned(),
                "BUILTIN\\Administrators".to_owned(),
                "Everyone".to_owned(),
            ]
        );
        let converged = format!("{path} {}:(OI)(CI)(F)", identity.sid);
        assert!(validate_acl(&converged, path, &identity, true).is_ok());
    }

    #[test]
    fn exact_single_owner_acl_is_a_noop() {
        let identity = identity();
        let path = r"C:\\private";
        let output = format!("{path} {}:(OI)(CI)(F)", identity.sid);
        assert_eq!(
            plan_acl_removals(&output, &identity, path).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn acl_planner_rejects_inherited_duplicate_malformed_and_unsafe_shapes() {
        let identity = identity();
        let path = r"C:\\private";
        let sid = identity.sid.as_str();
        let account = identity.account.as_str();
        for output in [
            format!("{path} {sid}:(I)(OI)(CI)(F)"),
            format!("{path} {sid}:(OI)(CI)(F)\n{account}:(OI)(CI)(F)"),
            format!("{path} {sid}:(OI)(CI)(F)\nDOMAIN/Runner:(F)"),
            format!("{path} {sid}:(OI)(CI)(F)\nDOMAIN\\Runner:(F)\nDOMAIN\\runner:(M)"),
            format!("{path} {sid}:(OI)(CI)(F)\nEveryone:(DENY)(F)"),
            format!("{path} {sid}:(OI)(CI)(F)\n*:(F)"),
            format!("{path} {sid}:(OI)(CI)(F)\nDOMAIN\\Other\u{0007}:(F)"),
            format!("{path} {sid}:(OI)(CI)(F)(F)"),
            format!("{path} {sid}:(OI)(CI)(F)\u{0007}"),
            format!("{path} {sid}:(OI)(CI)(F) trailing"),
        ] {
            assert!(plan_acl_removals(&output, &identity, path).is_err());
        }
    }

    #[test]
    fn acl_parser_rejects_ambiguous_path_and_summary_output() {
        let identity = identity();
        let path = r"C:\\private";
        let wrong_path = r"C:\\other";
        let wrong_header = format!("{wrong_path} {}:(OI)(CI)(F)", identity.sid);
        assert!(plan_acl_removals(&wrong_header, &identity, path).is_err());

        let duplicate_summary = format!(
            "{path} {}:(OI)(CI)(F)\nSuccessfully processed 1 files; Failed processing 0 files.\nSuccessfully processed 1 files; Failed processing 0 files.",
            identity.sid
        );
        assert!(plan_acl_removals(&duplicate_summary, &identity, path).is_err());

        let malformed_summary = format!(
            "{path} {}:(OI)(CI)(F)\nSuccessfully processed 2 files; Failed processing 0 files.",
            identity.sid
        );
        assert!(plan_acl_removals(&malformed_summary, &identity, path).is_err());
        let oversized = "x".repeat(MAX_ACL_OUTPUT_BYTES + 1);
        assert_eq!(
            collect_ace_lines(&oversized, path),
            Err(NativeFileError::TooLarge)
        );
    }
}
