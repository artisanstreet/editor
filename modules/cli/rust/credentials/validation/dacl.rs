#[cfg(all(test, windows))]
use std::fs::OpenOptions;
#[cfg(all(test, windows))]
use std::io::Write;
#[cfg(windows)]
use std::time::Duration;
use std::{fs, path::Path};

#[cfg(all(test, windows))]
use serde::Serialize;

use super::super::ForgeCredentialError;
use super::super::storage::acl_diagnostic;
#[cfg(all(test, windows))]
use super::super::storage::encode_nonce_hex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CurrentIdentity {
    pub(crate) sid: String,
    pub(crate) account: String,
}

#[cfg(all(test, windows))]
pub(crate) mod acl_diagnostic {
    use super::*;

    pub(super) const MAX_STREAM_BYTES: usize = 4096;
    pub(super) const MAX_EVENTS: usize = 16;
    const MAX_ARTIFACT_BYTES: usize = 16 * 1024;
    pub(super) const CANARIES: [&str; 7] = [
        "S-1-5-21-1-2-3-1000",
        "DOMAIN\\account-canary",
        "C:\\sensitive\\path",
        "whoami/icacls-output-canary",
        "credential-bytes-canary",
        "private-key-bytes-canary",
        "bootstrap-capability-bytes-canary",
    ];
    #[derive(Debug, Serialize)]
    struct AclDiagnosticEvent {
        stage: &'static str,
        kind: &'static str,
        value: &'static str,
    }

    #[derive(Debug, Serialize)]
    pub(super) struct AclDiagnosticRecord {
        schema_version: u8,
        outcome: &'static str,
        stage: &'static str,
        event_count: u8,
        overflow: bool,
        events: Vec<AclDiagnosticEvent>,
    }

    impl AclDiagnosticRecord {
        fn new() -> Self {
            Self {
                schema_version: 1,
                outcome: "Other",
                stage: "Unreached",
                event_count: 0,
                overflow: false,
                events: Vec::with_capacity(MAX_EVENTS),
            }
        }

        fn push(&mut self, stage: &'static str, kind: &'static str, value: &'static str) {
            if self.events.len() < MAX_EVENTS {
                self.events.push(AclDiagnosticEvent { stage, kind, value });
            } else {
                self.overflow = true;
            }
        }

        pub(super) fn finish(mut self, outcome: &'static str) -> Self {
            self.outcome = outcome;
            if outcome == "Success" {
                self.stage = "Completed";
            }
            self.event_count =
                u8::try_from(self.events.len()).expect("diagnostic event count fits in u8");
            self
        }
    }

    #[derive(Clone, Copy)]
    pub(super) enum PlannerClassification {
        InvalidValidatedIdentity,
        InheritedAce,
        CurrentIdentityMatch,
        DuplicateCurrentIdentity,
        SafeRemovableExtra,
        UnsafeNonmatchingExtra,
        DuplicateExtra,
        PlanComplete,
    }

    impl PlannerClassification {
        const fn as_str(self) -> &'static str {
            match self {
                Self::InvalidValidatedIdentity => "InvalidValidatedIdentity",
                Self::InheritedAce => "InheritedAce",
                Self::CurrentIdentityMatch => "CurrentIdentityMatch",
                Self::DuplicateCurrentIdentity => "DuplicateCurrentIdentity",
                Self::SafeRemovableExtra => "SafeRemovableExtra",
                Self::UnsafeNonmatchingExtra => "UnsafeNonmatchingExtra",
                Self::DuplicateExtra => "DuplicateExtra",
                Self::PlanComplete => "PlanComplete",
            }
        }
    }

    #[derive(Clone, Copy)]
    pub(super) enum ParserClassification {
        MalformedSuccessSummary,
        MissingSeparator,
        MissingOpeningToken,
        NonTokenContent,
        UnterminatedToken,
        EmptyToken,
        NoTokens,
        AcceptedAce,
        ParserComplete,
    }

    impl ParserClassification {
        const fn as_str(self) -> &'static str {
            match self {
                Self::MalformedSuccessSummary => "MalformedSuccessSummary",
                Self::MissingSeparator => "MissingSeparator",
                Self::MissingOpeningToken => "MissingOpeningToken",
                Self::NonTokenContent => "NonTokenContent",
                Self::UnterminatedToken => "UnterminatedToken",
                Self::EmptyToken => "EmptyToken",
                Self::NoTokens => "NoTokens",
                Self::AcceptedAce => "AcceptedAce",
                Self::ParserComplete => "ParserComplete",
            }
        }
    }

    thread_local! {
        static ACTIVE: std::cell::RefCell<Option<AclDiagnosticRecord>> = const {
            std::cell::RefCell::new(None)
        };
    }

    pub(super) fn capture<T>(operation: impl FnOnce() -> T) -> (T, AclDiagnosticRecord) {
        ACTIVE.with(|active| *active.borrow_mut() = Some(AclDiagnosticRecord::new()));
        let result = operation();
        let record = ACTIVE
            .with(|active| active.borrow_mut().take())
            .unwrap_or_else(AclDiagnosticRecord::new);
        (result, record)
    }

    pub(crate) fn stage(stage: &'static str) {
        ACTIVE.with(|active| {
            if let Some(record) = active.borrow_mut().as_mut() {
                record.stage = stage;
            }
        });
    }

    fn current_stage() -> &'static str {
        ACTIVE.with(|active| active.borrow().as_ref().map_or("Unreached", |r| r.stage))
    }

    fn event_at(stage: &'static str, kind: &'static str, value: &'static str) {
        ACTIVE.with(|active| {
            if let Some(record) = active.borrow_mut().as_mut() {
                record.push(stage, kind, value);
            }
        });
    }

    pub(super) fn event(kind: &'static str, value: &'static str) {
        event_at(current_stage(), kind, value);
    }

    pub(super) fn planner(classification: PlannerClassification) {
        event("planner", classification.as_str());
    }

    pub(super) fn parser(classification: ParserClassification) {
        event("parser", classification.as_str());
    }

    fn bounded(text: &str) -> &str {
        let mut end = text.len().min(MAX_STREAM_BYTES);
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    }

    pub(super) fn stream_shape(bytes: &[u8]) -> &'static str {
        if bytes.len() > MAX_STREAM_BYTES {
            "Oversized"
        } else if bytes.is_empty() {
            "Empty"
        } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
            "Bom"
        } else if std::str::from_utf8(bytes).is_ok() {
            "Utf8"
        } else {
            "InvalidUtf8"
        }
    }

    fn identity_shape(fields: usize, sid: Option<&str>, account: Option<&str>) -> &'static str {
        if fields != 2 {
            return "FieldCount";
        }
        let (Some(sid), Some(account)) = (sid, account) else {
            return "Missing";
        };
        if sid.len() > MAX_STREAM_BYTES || account.len() > MAX_STREAM_BYTES {
            return "Oversized";
        }
        if sid.is_empty() || account.is_empty() {
            return "Empty";
        }
        let valid_account = account.matches('\\').count() == 1
            && !account.starts_with('\\')
            && !account.ends_with('\\')
            && !account
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | ':' | '"' | ','));
        if is_valid_sid(sid) && valid_account {
            "Valid"
        } else {
            "Malformed"
        }
    }

    pub(super) fn record_identity(
        stdout: &[u8],
        stderr: &[u8],
        fields: usize,
        sid: Option<&str>,
        account: Option<&str>,
    ) {
        for (kind, bytes) in [("stdout", stdout), ("stderr", stderr)] {
            event_at("IdentityResolution", kind, stream_shape(bytes));
        }
        event_at(
            "IdentityResolution",
            "identity",
            identity_shape(fields, sid, account),
        );
    }

    fn summary(output: &str) -> &'static str {
        let output = bounded(output).to_ascii_lowercase();
        if let Some(line) = output
            .lines()
            .find(|line| line.trim_start().starts_with("successfully processed"))
        {
            if line.contains("failed processing") && line.bytes().any(|b| b.is_ascii_digit()) {
                "Recognized"
            } else {
                "Malformed"
            }
        } else if output.contains("successfully")
            || output.contains("processed")
            || output.contains("processing")
            || output.contains("failed")
            || output.contains("files")
        {
            "LocalizedOrOther"
        } else {
            "Missing"
        }
    }

    pub(super) fn record_acl(output: &str, count: usize) {
        event(
            "acl_count",
            match count {
                0 => "Zero",
                1 => "One",
                2 => "Two",
                _ => "Many",
            },
        );
        event("acl_summary", summary(output));
    }

    pub(super) fn probe(exe: &str, args: &[&str]) -> Option<&'static str> {
        if exe.eq_ignore_ascii_case("whoami.exe") {
            Some("Whoami")
        } else if exe.eq_ignore_ascii_case("icacls.exe") {
            Some(if args.contains(&"/grant:r") || args.contains(&"/remove") {
                "IcaclsMutation"
            } else {
                "IcaclsQuery"
            })
        } else {
            None
        }
    }

    pub(super) fn subprocess(probe: Option<&'static str>, outcome: &'static str) {
        let Some(probe) = probe else {
            return;
        };
        event_at(
            if probe == "Whoami" {
                "IdentityResolution"
            } else {
                current_stage()
            },
            probe,
            outcome,
        );
    }

    pub(super) fn ace_reason(
        ace: &str,
        identity: &CurrentIdentity,
        expect_dir: bool,
    ) -> &'static str {
        let ace = bounded(ace);
        let lower = ace.to_ascii_lowercase();
        if lower.contains("deny") {
            return "Deny";
        }
        if lower.contains("(i)") {
            return "Inherited";
        }
        let Some(colon) = ace.find(':') else {
            return "MalformedAce";
        };
        let principal = ace[..colon].trim();
        let sid = bounded(&identity.sid);
        let account = bounded(&identity.account);
        let matches = (identity.sid.len() <= MAX_STREAM_BYTES
            && !sid.is_empty()
            && principal.eq_ignore_ascii_case(sid))
            || (identity.account.len() <= MAX_STREAM_BYTES
                && !account.is_empty()
                && principal.eq_ignore_ascii_case(account));
        if principal.is_empty() || !matches {
            return "IdentityMismatch";
        }
        let flags = &lower[colon + 1..];
        if !flags.contains("(f)") {
            return "MissingFullControl";
        }
        if flags.matches("(f)").count() != 1
            || flags.matches("(oi)").count() > 1
            || flags.matches("(ci)").count() > 1
            || flags
                .chars()
                .any(|c| c.is_ascii_alphabetic() && !matches!(c, 'f' | 'o' | 'i' | 'c'))
        {
            return "InvalidFlags";
        }
        if (expect_dir && (!flags.contains("(oi)") || !flags.contains("(ci)")))
            || (!expect_dir && (flags.contains("(oi)") || flags.contains("(ci)")))
        {
            return "InheritanceMismatch";
        }
        if ["everyone", "builtin", "nt authority", "authenticated users"]
            .iter()
            .any(|forbidden| lower.contains(forbidden))
        {
            "BroadPrincipal"
        } else {
            "Accepted"
        }
    }

    pub(super) fn write(path: &Path, record: &AclDiagnosticRecord) -> Option<()> {
        if !path.is_absolute() {
            return None;
        }
        let bytes = serde_json::to_vec(record).ok()?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return None;
        }
        let parent = path.parent()?;
        let file_name = path.file_name()?;
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).ok()?;
        let temporary_path = parent.join(format!(
            ".{}.{}.tmp",
            file_name.to_string_lossy(),
            encode_nonce_hex(&nonce)
        ));
        let result = (|| -> Option<()> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary_path)
                .ok()?;
            file.write_all(&bytes).ok()?;
            file.sync_all().ok()?;
            drop(file);
            fs::rename(&temporary_path, path).ok()?;
            Some(())
        })();
        if result.is_none() {
            let _ = fs::remove_file(&temporary_path);
        }
        result
    }

    pub(super) fn assert_redacted(record: &AclDiagnosticRecord) {
        let debug = format!("{record:?}");
        let bytes = serde_json::to_vec(record).expect("structural diagnostic serialization");
        assert!(bytes.len() <= MAX_ARTIFACT_BYTES);
        let text = String::from_utf8_lossy(&bytes);
        for canary in CANARIES {
            assert!(!debug.contains(canary));
            assert!(!text.contains(canary));
        }
    }
}

#[cfg(windows)]
pub(crate) fn hidden_output(
    exe: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, ForgeCredentialError> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    #[cfg(all(test, windows))]
    let probe = acl_diagnostic::probe(exe, args);
    let mut command = std::process::Command::new(exe);
    for arg in args {
        command.arg(arg);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = command.spawn().map_err(|_| {
        acl_diagnostic!(acl_diagnostic::subprocess(probe, "SpawnFailed"));
        ForgeCredentialError::Provisioning
    })?;
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            let _ = child.kill();
            let reap_start = std::time::Instant::now();
            let reap_deadline = Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) => {
                        if reap_start.elapsed() > reap_deadline {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
            let _ = child.try_wait();
            acl_diagnostic!(acl_diagnostic::subprocess(probe, "TimedOut"));
            return Err(ForgeCredentialError::Provisioning);
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                acl_diagnostic!(acl_diagnostic::subprocess(probe, "WaitFailed"));
                return Err(ForgeCredentialError::Provisioning);
            }
        }
    }
    let output = child.wait_with_output().map_err(|_| {
        acl_diagnostic!(acl_diagnostic::subprocess(probe, "WaitFailed"));
        ForgeCredentialError::Provisioning
    })?;
    acl_diagnostic!(acl_diagnostic::subprocess(
        probe,
        if output.status.success() {
            "ExitedSuccess"
        } else {
            "ExitedNonZero"
        },
    ));
    Ok(output)
}

#[cfg(windows)]
pub(crate) fn resolve_current_identity() -> Result<CurrentIdentity, ForgeCredentialError> {
    let output = hidden_output(
        "whoami.exe",
        &["/user", "/fo", "csv", "/nh"],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        acl_diagnostic!(acl_diagnostic::record_identity(
            &output.stdout,
            &output.stderr,
            0,
            None,
            None
        ));
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let Some(line) = text.lines().next() else {
        acl_diagnostic!(acl_diagnostic::record_identity(
            &output.stdout,
            &output.stderr,
            0,
            None,
            None
        ));
        return Err(ForgeCredentialError::WindowsAcl);
    };
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == ',' && !in_quotes {
            parts.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(ch);
        }
    }
    parts.push(current.trim().to_string());
    if parts.len() != 2 {
        acl_diagnostic!(acl_diagnostic::record_identity(
            &output.stdout,
            &output.stderr,
            parts.len(),
            None,
            None
        ));
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let account = parts[0].trim().trim_matches('"').trim().to_string();
    let sid = parts[1].trim().trim_matches('"').trim().to_string();
    acl_diagnostic!(acl_diagnostic::record_identity(
        &output.stdout,
        &output.stderr,
        parts.len(),
        Some(&sid),
        Some(&account),
    ));
    if !is_valid_sid(&sid) {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    if !is_valid_account(&account) {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    Ok(CurrentIdentity { sid, account })
}

fn is_valid_sid(sid: &str) -> bool {
    if !sid.starts_with("S-1-") {
        return false;
    }
    if sid.contains(' ') || sid.contains('/') || sid.contains('\\') {
        return false;
    }
    let parts: Vec<&str> = sid.split('-').collect();
    if parts.len() < 3 {
        return false;
    }
    if parts[0] != "S" || parts[1] != "1" {
        return false;
    }
    for part in &parts[2..] {
        if part.is_empty() || part.parse::<u64>().is_err() {
            return false;
        }
    }
    true
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
    if account.matches('\\').count() != 1 {
        return false;
    }
    let mut split = account.split('\\');
    let domain = split.next().unwrap_or("");
    let user = split.next().unwrap_or("");
    !domain.is_empty() && !user.is_empty() && split.next().is_none()
}

#[cfg(test)]
fn parse_icacls_output_with_path(
    output: &str,
    expected_sid: &str,
    queried_path: &str,
) -> Result<(), ForgeCredentialError> {
    let identity = CurrentIdentity {
        sid: expected_sid.to_string(),
        account: String::new(),
    };
    parse_icacls_strict_with_identity(output, &identity, true, queried_path)
}

#[cfg(test)]
fn parse_icacls_strict_with_path(
    output: &str,
    expected_sid: &str,
    expect_dir: bool,
    queried_path: &str,
) -> Result<(), ForgeCredentialError> {
    let identity = CurrentIdentity {
        sid: expected_sid.to_string(),
        account: String::new(),
    };
    parse_icacls_strict_with_identity(output, &identity, expect_dir, queried_path)
}

fn parse_icacls_strict_with_identity(
    output: &str,
    identity: &CurrentIdentity,
    expect_dir: bool,
    queried_path: &str,
) -> Result<(), ForgeCredentialError> {
    let ace_lines = collect_icacls_ace_lines(output, queried_path)?;
    match ace_lines.as_slice() {
        [ace] => validate_icacls_ace(ace, identity, expect_dir),
        _ => Err(ForgeCredentialError::WindowsAcl),
    }
}

fn validate_icacls_flag_tokens(
    flags_part: &str,
    output: &str,
    ace_count: usize,
) -> Result<(), ForgeCredentialError> {
    let _ = (output, ace_count);
    // Flags must be exactly a sequence of (...) tokens with optional whitespace, nothing else
    let mut idx = 0;
    let chars: Vec<char> = flags_part.chars().collect();
    let mut has_content = false;
    while idx < chars.len() {
        while idx < chars.len() && chars[idx].is_whitespace() {
            idx += 1;
        }
        if idx >= chars.len() {
            break;
        }
        if chars[idx] != '(' {
            acl_diagnostic!(acl_diagnostic::parser(
                acl_diagnostic::ParserClassification::NonTokenContent
            ));
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_count));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        let mut tok = String::new();
        idx += 1;
        while idx < chars.len() && chars[idx] != ')' {
            tok.push(chars[idx]);
            idx += 1;
        }
        if idx >= chars.len() || chars[idx] != ')' {
            acl_diagnostic!(acl_diagnostic::parser(
                acl_diagnostic::ParserClassification::UnterminatedToken
            ));
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_count));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        idx += 1;
        has_content = true;
        if tok.is_empty() {
            acl_diagnostic!(acl_diagnostic::parser(
                acl_diagnostic::ParserClassification::EmptyToken
            ));
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_count));
            return Err(ForgeCredentialError::WindowsAcl);
        }
    }
    if !has_content {
        acl_diagnostic!(acl_diagnostic::parser(
            acl_diagnostic::ParserClassification::NoTokens
        ));
        acl_diagnostic!(acl_diagnostic::record_acl(output, ace_count));
        return Err(ForgeCredentialError::WindowsAcl);
    }
    Ok(())
}

fn collect_icacls_ace_lines(
    output: &str,
    queried_path: &str,
) -> Result<Vec<String>, ForgeCredentialError> {
    let mut ace_lines: Vec<String> = Vec::new();
    let queried_lower = queried_path.to_ascii_lowercase();
    let mut first_line = true;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("successfully") {
            if is_icacls_success_summary(&lower) {
                continue;
            }
            acl_diagnostic!(acl_diagnostic::parser(
                acl_diagnostic::ParserClassification::MalformedSuccessSummary
            ));
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        // Exact queried path prefix handling: strip only the complete queried path
        // (case-insensitive) from the first ACE line if present.
        let mut candidate = trimmed.to_string();
        if !queried_path.is_empty() && lower.starts_with(&queried_lower) {
            let remainder = trimmed[queried_path.len()..].trim();
            if remainder.is_empty() {
                // Header line containing only the path, no ACE
                first_line = false;
                continue;
            }
            candidate = remainder.to_string();
        } else if first_line && trimmed.contains(":\\") && !trimmed.contains('(') {
            // Header without queried_path provided (parser test without path)
            first_line = false;
            continue;
        }
        first_line = false;
        if !candidate.contains(':') {
            acl_diagnostic!(acl_diagnostic::parser(
                acl_diagnostic::ParserClassification::MissingSeparator
            ));
            // Any non-summary, non-ACE line is a failure.
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        if !candidate.contains('(') {
            if candidate
                .split_once(':')
                .is_some_and(|(_, flags)| flags.trim().is_empty())
            {
                acl_diagnostic!(acl_diagnostic::parser(
                    acl_diagnostic::ParserClassification::NoTokens
                ));
            } else {
                acl_diagnostic!(acl_diagnostic::parser(
                    acl_diagnostic::ParserClassification::MissingOpeningToken
                ));
            }
            // Any non-summary, non-ACE line is a failure.
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        // Ensure no trailing junk after the parenthesized flags
        let Some(colon) = candidate.find(':') else {
            acl_diagnostic!(acl_diagnostic::parser(
                acl_diagnostic::ParserClassification::MissingSeparator
            ));
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
            return Err(ForgeCredentialError::WindowsAcl);
        };
        let flags_part = &candidate[colon + 1..];
        validate_icacls_flag_tokens(flags_part, output, ace_lines.len())?;
        ace_lines.push(candidate);
        acl_diagnostic!(acl_diagnostic::parser(
            acl_diagnostic::ParserClassification::AcceptedAce
        ));
    }
    acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
    acl_diagnostic!(acl_diagnostic::parser(
        acl_diagnostic::ParserClassification::ParserComplete
    ));
    Ok(ace_lines)
}

fn is_icacls_success_summary(line: &str) -> bool {
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

fn plan_icacls_removals(
    output: &str,
    identity: &CurrentIdentity,
    queried_path: &str,
) -> Result<Vec<String>, ForgeCredentialError> {
    if !is_valid_sid(&identity.sid)
        || !is_valid_account(&identity.account)
        || identity.sid.eq_ignore_ascii_case(&identity.account)
    {
        acl_diagnostic!(acl_diagnostic::planner(
            acl_diagnostic::PlannerClassification::InvalidValidatedIdentity
        ));
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let ace_lines = collect_icacls_ace_lines(output, queried_path)?;
    let mut removals = Vec::new();
    let mut current_identity_count = 0;
    for ace in ace_lines {
        let Some(colon) = ace.find(':') else {
            return Err(ForgeCredentialError::WindowsAcl);
        };
        let principal = ace[..colon].trim();
        let flags = ace[colon + 1..].to_ascii_lowercase();
        if flags.contains("(i)") {
            acl_diagnostic!(acl_diagnostic::planner(
                acl_diagnostic::PlannerClassification::InheritedAce
            ));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        let is_deny = flags.contains("(deny)");
        let is_current_identity = principal.eq_ignore_ascii_case(&identity.sid)
            || principal.eq_ignore_ascii_case(&identity.account);
        if is_current_identity {
            current_identity_count += 1;
            if current_identity_count > 1 {
                acl_diagnostic!(acl_diagnostic::planner(
                    acl_diagnostic::PlannerClassification::DuplicateCurrentIdentity
                ));
                return Err(ForgeCredentialError::WindowsAcl);
            }
            acl_diagnostic!(acl_diagnostic::planner(
                acl_diagnostic::PlannerClassification::CurrentIdentityMatch
            ));
            if is_deny {
                removals.push(principal.to_owned());
            }
            continue;
        }
        if !is_safe_acl_principal(principal) {
            acl_diagnostic!(acl_diagnostic::planner(
                acl_diagnostic::PlannerClassification::UnsafeNonmatchingExtra
            ));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        if removals
            .iter()
            .any(|candidate: &String| candidate.eq_ignore_ascii_case(principal))
        {
            acl_diagnostic!(acl_diagnostic::planner(
                acl_diagnostic::PlannerClassification::DuplicateExtra
            ));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        acl_diagnostic!(acl_diagnostic::planner(
            acl_diagnostic::PlannerClassification::SafeRemovableExtra
        ));
        removals.push(principal.to_owned());
    }
    acl_diagnostic!(acl_diagnostic::planner(
        acl_diagnostic::PlannerClassification::PlanComplete
    ));
    Ok(removals)
}

fn is_safe_acl_principal(principal: &str) -> bool {
    if principal
        .chars()
        .any(|character| matches!(character, '*' | '(' | ')'))
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

fn validate_icacls_ace(
    ace: &str,
    identity: &CurrentIdentity,
    expect_dir: bool,
) -> Result<(), ForgeCredentialError> {
    acl_diagnostic!(acl_diagnostic::event(
        "ace",
        acl_diagnostic::ace_reason(ace, identity, expect_dir)
    ));
    let lower = ace.to_ascii_lowercase();
    if lower.contains("deny") {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    if lower.contains("(i)") {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let colon = ace.find(':').ok_or(ForgeCredentialError::WindowsAcl)?;
    let principal = ace[..colon].trim();
    let matches_sid =
        !identity.sid.is_empty() && principal.eq_ignore_ascii_case(identity.sid.as_str());
    let matches_account =
        !identity.account.is_empty() && principal.eq_ignore_ascii_case(identity.account.as_str());
    if !matches_sid && !matches_account {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let flags_part = &ace[colon + 1..];
    let flags_lower = flags_part.to_ascii_lowercase();
    if !flags_lower.contains("(f)") {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let mut tokens: Vec<String> = Vec::new();
    let mut chars = flags_lower.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '(' {
            let mut tok = String::new();
            for c in chars.by_ref() {
                if c == ')' {
                    break;
                }
                tok.push(c);
            }
            tokens.push(tok);
        }
    }
    let mut seen_f = 0;
    let mut object_inherit_count = 0;
    let mut container_inherit_count = 0;
    for tok in &tokens {
        match tok.as_str() {
            "f" => seen_f += 1,
            "oi" => object_inherit_count += 1,
            "ci" => container_inherit_count += 1,
            _ => return Err(ForgeCredentialError::WindowsAcl),
        }
    }
    if seen_f != 1 {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    if expect_dir {
        if object_inherit_count != 1 || container_inherit_count != 1 {
            return Err(ForgeCredentialError::WindowsAcl);
        }
    } else if object_inherit_count != 0 || container_inherit_count != 0 {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let mut stripped = flags_lower.clone();
    stripped = stripped.replace("(f)", "");
    stripped = stripped.replace("(oi)", "");
    stripped = stripped.replace("(ci)", "");
    stripped = stripped.replace([' ', '\t', ','], "");
    if !stripped.trim().is_empty() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    for forbidden in ["everyone", "builtin", "nt authority", "authenticated users"] {
        if lower.contains(forbidden) {
            return Err(ForgeCredentialError::WindowsAcl);
        }
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn verify_windows_dacl(
    path: &Path,
    expected_sid: &str,
) -> Result<(), ForgeCredentialError> {
    let identity = resolve_current_identity()?;
    if !identity.sid.eq_ignore_ascii_case(expected_sid) {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let path_str = path.to_string_lossy().to_string();
    let output = hidden_output("icacls.exe", &[&path_str], Duration::from_secs(5))?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let is_dir = fs::metadata(path).is_ok_and(|metadata| metadata.is_dir());
    parse_icacls_strict_with_identity(&text, &identity, is_dir, &path_str)
}

#[cfg(windows)]
pub(crate) fn restrict_directory_windows(dir: &Path) -> Result<(), ForgeCredentialError> {
    acl_diagnostic!(acl_diagnostic::stage("DirectoryAclMutation"));
    let identity = resolve_current_identity()?;
    let dir_str = dir.to_string_lossy().to_string();
    let grant = format!("*{}:(OI)(CI)F", identity.sid);
    let output = hidden_output(
        "icacls.exe",
        &[&dir_str, "/inheritance:r", "/grant:r", &grant],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    acl_diagnostic!(acl_diagnostic::stage("DirectoryAclVerification"));
    let query = hidden_output("icacls.exe", &[&dir_str], Duration::from_secs(5))?;
    if !query.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let text = String::from_utf8_lossy(&query.stdout).to_string();
    let removals = plan_icacls_removals(&text, &identity, &dir_str)?;
    acl_diagnostic!(acl_diagnostic::stage("DirectoryAclMutation"));
    for principal in &removals {
        let removal = icacls_remove_argument(principal);
        let output = hidden_output(
            "icacls.exe",
            &[&dir_str, "/remove", removal.as_str()],
            Duration::from_secs(5),
        )?;
        if !output.status.success() {
            return Err(ForgeCredentialError::WindowsAcl);
        }
    }
    let output = hidden_output(
        "icacls.exe",
        &[&dir_str, "/grant:r", &grant],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    acl_diagnostic!(acl_diagnostic::stage("DirectoryAclVerification"));
    verify_windows_dacl(dir, &identity.sid)?;
    Ok(())
}

#[cfg(windows)]
fn icacls_remove_argument(principal: &str) -> String {
    if is_valid_sid(principal) {
        format!("*{principal}")
    } else {
        principal.to_owned()
    }
}

#[cfg(windows)]
pub(crate) fn restrict_file_windows(path: &Path, sid: &str) -> Result<(), ForgeCredentialError> {
    acl_diagnostic!(acl_diagnostic::stage("BundleAclMutation"));
    let identity = resolve_current_identity()?;
    if !identity.sid.eq_ignore_ascii_case(sid) {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let path_str = path.to_string_lossy().to_string();
    let grant = format!("*{sid}:F");
    let output = hidden_output(
        "icacls.exe",
        &[&path_str, "/inheritance:r", "/grant:r", &grant],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    acl_diagnostic!(acl_diagnostic::stage("BundleAclVerification"));
    verify_windows_dacl(path, sid)
}

// Tiny private parser test kept inside production file because DACL string shape cannot be
// exercised through the public credential facade without Windows `icacls` execution.
#[cfg(test)]
mod parser_tests {
    use super::{
        CurrentIdentity, is_icacls_success_summary, parse_icacls_output_with_path,
        parse_icacls_strict_with_identity, parse_icacls_strict_with_path, plan_icacls_removals,
    };

    #[test]
    fn windows_dacl_parser_strict() {
        let sid = "S-1-5-21-1-2-3-1000";
        let account = "TEST\\User";
        let identity = CurrentIdentity {
            sid: sid.to_string(),
            account: account.to_string(),
        };
        let dir_path = "C:\\creds";
        let file_path = "C:\\creds\\file";
        let good_dir_sid = format!("{dir_path} {sid}:(OI)(CI)(F)");
        assert!(parse_icacls_output_with_path(&good_dir_sid, sid, dir_path).is_ok());
        assert!(parse_icacls_strict_with_path(&good_dir_sid, sid, true, dir_path).is_ok());
        let good_dir_account = format!("{dir_path} {account}:(OI)(CI)(F)");
        assert!(
            parse_icacls_strict_with_identity(&good_dir_account, &identity, true, dir_path).is_ok()
        );
        // A drive-letter path must be stripped only when the complete queried path matches.
        let drive_file = format!("{file_path} {sid}:(F)");
        assert!(parse_icacls_strict_with_path(&drive_file, sid, false, file_path).is_ok());
        assert!(
            parse_icacls_strict_with_path(&drive_file, sid, false, "C:\\creds\\other").is_err()
        );
        let sid_prefix = format!("{file_path} {sid}00:(F)");
        assert!(parse_icacls_strict_with_path(&sid_prefix, sid, false, file_path).is_err());
        let with_inherited = format!("{dir_path} {sid}:(I)(OI)(CI)(F)");
        assert!(parse_icacls_output_with_path(&with_inherited, sid, dir_path).is_err());
        let two_sids = format!("{dir_path} {sid}:(F)\nS-1-5-21-1-2-3-1001:(F)");
        assert!(parse_icacls_output_with_path(&two_sids, sid, dir_path).is_err());
        let deny = format!("{dir_path} {sid}:(DENY)(F)");
        assert!(parse_icacls_output_with_path(&deny, sid, dir_path).is_err());
        let everyone = format!("{dir_path} BUILTIN\\Users:(F) {sid}:(F)");
        assert!(parse_icacls_output_with_path(&everyone, sid, dir_path).is_err());
        let named_extra = format!("{dir_path} {sid}:(F)\nDOMAIN\\OtherUser:(F)");
        assert!(parse_icacls_output_with_path(&named_extra, sid, dir_path).is_err());
        assert!(parse_icacls_strict_with_path(&named_extra, sid, false, dir_path).is_err());
        let extra_flags = format!("{dir_path} {sid}:(OI)(CI)(F)(M)");
        assert!(parse_icacls_strict_with_path(&extra_flags, sid, true, dir_path).is_err());
        let broad = format!("{dir_path} {sid}:(OI)(CI)(M)");
        assert!(parse_icacls_strict_with_path(&broad, sid, true, dir_path).is_err());
        // Exact queried path containing spaces
        let spaced_path = "C:\\My Documents\\Artisan creds";
        let spaced_good = format!("{spaced_path} {sid}:(OI)(CI)(F)");
        assert!(parse_icacls_strict_with_path(&spaced_good, sid, true, spaced_path).is_ok());
        assert!(
            parse_icacls_strict_with_identity(&spaced_good, &identity, true, spaced_path).is_ok()
        );
        let spaced_wrong_prefix = format!("C:\\My Documents\\Other {sid}:(OI)(CI)(F)");
        assert!(
            parse_icacls_strict_with_path(&spaced_wrong_prefix, sid, true, spaced_path).is_err()
        );
        // Duplicate tokens
        let dup_f_file = format!("{file_path} {sid}:(F)(F)");
        assert!(parse_icacls_strict_with_path(&dup_f_file, sid, false, file_path).is_err());
        let dup_oi_dir = format!("{dir_path} {sid}:(OI)(OI)(CI)(F)");
        assert!(parse_icacls_strict_with_path(&dup_oi_dir, sid, true, dir_path).is_err());
        // Trailing junk
        let trailing = format!("{file_path} {sid}:(F) extra");
        assert!(parse_icacls_strict_with_path(&trailing, sid, false, file_path).is_err());
        let localized = format!("{dir_path} {sid}:(OI)(CI)(F)\nDacl access");
        assert!(parse_icacls_output_with_path(&localized, sid, dir_path).is_err());
    }

    #[test]
    fn icacls_success_summary_requires_one_successful_target() {
        for summary in [
            "successfully processed 1 files; failed processing 0 files",
            "successfully processed 1 files; failed processing 0 files.",
        ] {
            assert!(is_icacls_success_summary(summary));
        }
        for summary in [
            "successfully processed 2 files; failed processing 0 files",
            "successfully processed 1 files; failed processing 1 files",
            "successfully processed one files; failed processing 0 files",
            "successfully processed 1 files; failed processing 0 files. trailing",
            "successfully processed 1 files; failed processing 0 files..",
            "erfolgreich verarbeitet 1 files; failed processing 0 files",
        ] {
            assert!(!is_icacls_success_summary(summary));
        }
    }

    #[test]
    fn dacl_convergence_plans_all_extra_explicit_principals() {
        let sid = "S-1-5-21-1-2-3-1000";
        let account = "TEST\\User";
        let identity = CurrentIdentity {
            sid: sid.to_string(),
            account: account.to_string(),
        };
        let path = "C:\\creds";
        let output = format!(
            "{path} {sid}:(OI)(CI)(F)\nDOMAIN\\Runner:(OI)(CI)(F)\nBUILTIN\\Administrators:(F)\nEveryone:(F)"
        );
        assert_eq!(
            plan_icacls_removals(&output, &identity, path).unwrap(),
            vec![
                "DOMAIN\\Runner".to_string(),
                "BUILTIN\\Administrators".to_string(),
                "Everyone".to_string(),
            ]
        );
        assert!(parse_icacls_strict_with_identity(&output, &identity, true, path).is_err());

        let account_output = format!("{path} {account}:(OI)(CI)(F)\nDOMAIN\\Runner:(OI)(CI)(F)");
        assert_eq!(
            plan_icacls_removals(&account_output, &identity, path).unwrap(),
            vec!["DOMAIN\\Runner".to_string()]
        );

        let lowercase_sid = sid.to_ascii_lowercase();
        let converged_output = format!("{path} {lowercase_sid}:(OI)(CI)(F)");
        assert_eq!(
            plan_icacls_removals(&converged_output, &identity, path).unwrap(),
            Vec::<String>::new()
        );
        assert!(
            parse_icacls_strict_with_identity(&converged_output, &identity, true, path).is_ok()
        );
    }

    #[test]
    fn dacl_convergence_rejects_malformed_duplicate_and_ambiguous_identities() {
        let sid = "S-1-5-21-1-2-3-1000";
        let account = "TEST\\User";
        let identity = CurrentIdentity {
            sid: sid.to_string(),
            account: account.to_string(),
        };
        let path = "C:\\creds";
        for output in [
            format!("{path} DOMAIN/Runner:(F)"),
            format!("{path} DOMAIN\\Runner:(F)\nDOMAIN\\runner:(M)"),
            format!("{path} {sid}:(F)\n{account}:(F)"),
            format!("{path} {sid}:(I)(OI)(CI)(F)"),
        ] {
            assert!(plan_icacls_removals(&output, &identity, path).is_err());
        }

        let malformed_identity = CurrentIdentity {
            sid: "not-a-sid".into(),
            account: account.into(),
        };
        let exact = format!("{path} {sid}:(OI)(CI)(F)");
        assert!(plan_icacls_removals(&exact, &malformed_identity, path).is_err());
    }
}

#[cfg(all(test, windows))]
mod diagnostic_tests {
    use super::*;
    use crate::credentials::provision_or_load;
    use std::path::PathBuf;

    fn assert_artifact(bytes: &[u8]) {
        assert!(bytes.len() <= 16 * 1024);
        let value: serde_json::Value = serde_json::from_slice(bytes).expect("diagnostic JSON");
        let object = value.as_object().expect("diagnostic object");
        assert_eq!(
            object
                .get("schema_version")
                .and_then(serde_json::Value::as_u64),
            Some(1)
        );
        let events = object
            .get("events")
            .and_then(serde_json::Value::as_array)
            .expect("diagnostic events");
        assert!(events.len() <= acl_diagnostic::MAX_EVENTS);
        assert_eq!(
            object
                .get("event_count")
                .and_then(serde_json::Value::as_u64),
            Some(events.len() as u64)
        );
        let text = String::from_utf8_lossy(bytes);
        for canary in acl_diagnostic::CANARIES {
            assert!(!text.contains(canary));
        }
    }

    fn assert_classification(
        kind: &'static str,
        expected: &'static str,
        expect_success: bool,
        allowed: &[&str],
        operation: impl FnOnce() -> Result<Vec<String>, ForgeCredentialError>,
    ) {
        let (result, captured) = acl_diagnostic::capture(operation);
        assert_eq!(result.is_ok(), expect_success);
        let record = captured.finish(if expect_success {
            "Success"
        } else {
            "WindowsAcl"
        });
        acl_diagnostic::assert_redacted(&record);
        let bytes = serde_json::to_vec(&record).expect("bounded classification diagnostic");
        assert_artifact(&bytes);
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("classification JSON");
        let events = value
            .get("events")
            .and_then(serde_json::Value::as_array)
            .expect("classification events");
        let classification_values: Vec<&str> = events
            .iter()
            .filter_map(|event| {
                let object = event.as_object()?;
                if object.get("kind").and_then(serde_json::Value::as_str) != Some(kind) {
                    return None;
                }
                object.get("value").and_then(serde_json::Value::as_str)
            })
            .collect();
        assert!(
            classification_values.contains(&expected),
            "ACL diagnostic classification mismatch: kind={kind}, expected={expected}, actual={classification_values:?}"
        );
        assert!(
            classification_values
                .iter()
                .all(|value| allowed.contains(value))
        );
    }

    fn assert_planner_classification(
        expected: &'static str,
        expect_success: bool,
        operation: impl FnOnce() -> Result<Vec<String>, ForgeCredentialError>,
    ) {
        let allowed = [
            "InvalidValidatedIdentity",
            "InheritedAce",
            "CurrentIdentityMatch",
            "DuplicateCurrentIdentity",
            "SafeRemovableExtra",
            "UnsafeNonmatchingExtra",
            "DuplicateExtra",
            "PlanComplete",
        ];
        assert_classification("planner", expected, expect_success, &allowed, operation);
    }

    fn assert_parser_classification(
        expected: &'static str,
        expect_success: bool,
        operation: impl FnOnce() -> Result<Vec<String>, ForgeCredentialError>,
    ) {
        let allowed = [
            "MalformedSuccessSummary",
            "MissingSeparator",
            "MissingOpeningToken",
            "NonTokenContent",
            "UnterminatedToken",
            "EmptyToken",
            "NoTokens",
            "AcceptedAce",
            "ParserComplete",
        ];
        assert_classification("parser", expected, expect_success, &allowed, operation);
    }

    #[test]
    fn parser_diagnostic_classifications_are_bounded_and_redacted() {
        let sid = acl_diagnostic::CANARIES[0];
        let path = acl_diagnostic::CANARIES[2];
        let accepted = format!("{path} {sid}:(OI)(CI)(F)");

        assert_parser_classification("MalformedSuccessSummary", false, || {
            collect_icacls_ace_lines(
                &format!(
                    "{accepted}\nSuccessfully processed many files; Failed processing 0 files."
                ),
                path,
            )
        });
        assert_parser_classification("MissingSeparator", false, || {
            collect_icacls_ace_lines(&format!("{path} non-ace output"), path)
        });
        assert_parser_classification("MissingOpeningToken", false, || {
            collect_icacls_ace_lines(&format!("{path} {sid}:F"), path)
        });
        assert_parser_classification("NonTokenContent", false, || {
            collect_icacls_ace_lines(&format!("{path} {sid}:(F) trailing"), path)
        });
        assert_parser_classification("UnterminatedToken", false, || {
            collect_icacls_ace_lines(&format!("{path} {sid}:(F"), path)
        });
        assert_parser_classification("EmptyToken", false, || {
            collect_icacls_ace_lines(&format!("{path} {sid}:()"), path)
        });
        assert_parser_classification("NoTokens", false, || {
            collect_icacls_ace_lines(&format!("{path} ({sid}:"), path)
        });
        assert_parser_classification("AcceptedAce", true, || {
            collect_icacls_ace_lines(&accepted, path)
        });
        assert_parser_classification("ParserComplete", true, || {
            collect_icacls_ace_lines(&accepted, path)
        });
    }

    #[test]
    fn planner_diagnostic_classifications_are_bounded_and_redacted() {
        let sid = acl_diagnostic::CANARIES[0];
        let account = acl_diagnostic::CANARIES[1];
        let path = acl_diagnostic::CANARIES[2];
        let identity = CurrentIdentity {
            sid: sid.to_string(),
            account: account.to_string(),
        };

        let invalid_identity = CurrentIdentity {
            sid: "not-a-sid".to_string(),
            account: account.to_string(),
        };
        assert_planner_classification("InvalidValidatedIdentity", false, || {
            plan_icacls_removals("", &invalid_identity, path)
        });
        assert_planner_classification("InheritedAce", false, || {
            plan_icacls_removals(&format!("{path} {sid}:(I)(OI)(CI)(F)"), &identity, path)
        });
        assert_planner_classification("CurrentIdentityMatch", true, || {
            plan_icacls_removals(&format!("{path} {sid}:(OI)(CI)(F)"), &identity, path)
        });
        assert_planner_classification("DuplicateCurrentIdentity", false, || {
            plan_icacls_removals(
                &format!("{path} {sid}:(OI)(CI)(F)\n{account}:(OI)(CI)(F)"),
                &identity,
                path,
            )
        });
        assert_planner_classification("SafeRemovableExtra", true, || {
            plan_icacls_removals(
                &format!("{path} {sid}:(OI)(CI)(F)\nEveryone:(F)"),
                &identity,
                path,
            )
        });
        assert_planner_classification("UnsafeNonmatchingExtra", false, || {
            plan_icacls_removals(
                &format!("{path} {sid}:(OI)(CI)(F)\nmalformed/principal:(F)"),
                &identity,
                path,
            )
        });
        assert_planner_classification("DuplicateExtra", false, || {
            plan_icacls_removals(
                &format!("{path} {sid}:(OI)(CI)(F)\nEveryone:(F)\nEVERYONE:(F)"),
                &identity,
                path,
            )
        });
        assert_planner_classification("PlanComplete", true, || {
            plan_icacls_removals(&format!("{path} {sid}:(OI)(CI)(F)"), &identity, path)
        });
    }

    #[test]
    fn diagnostic_capture_is_bounded_redacted_and_noninterfering() {
        for (bytes, shape) in [
            (&[][..], "Empty"),
            (&[0xef, 0xbb, 0xbf][..], "Bom"),
            (&[0xff][..], "InvalidUtf8"),
        ] {
            assert_eq!(acl_diagnostic::stream_shape(bytes), shape);
        }
        assert_eq!(
            acl_diagnostic::stream_shape(&[b'x'; acl_diagnostic::MAX_STREAM_BYTES]),
            "Utf8"
        );
        assert_eq!(
            acl_diagnostic::stream_shape(&[b'x'; acl_diagnostic::MAX_STREAM_BYTES + 1]),
            "Oversized"
        );

        let sid = "S-1-5-21-1-2-3-1000";
        let identity = CurrentIdentity {
            sid: sid.to_string(),
            account: "DOMAIN\\account-canary".to_string(),
        };
        let raw_path = "C:\\sensitive\\path";
        let raw_output = format!(
            "{raw_path} {sid}:(OI)(CI)(F)\nSuccessfully processed 1 files; Failed processing 0 files."
        );
        let raw_stderr = acl_diagnostic::CANARIES.join("\n");
        let (result, captured) = acl_diagnostic::capture(|| {
            acl_diagnostic::stage("DirectoryAclVerification");
            acl_diagnostic::record_identity(
                raw_output.as_bytes(),
                raw_stderr.as_bytes(),
                2,
                Some(sid),
                Some(identity.account.as_str()),
            );
            parse_icacls_strict_with_identity(&raw_output, &identity, true, raw_path)
        });
        assert!(result.is_ok());
        let record = captured.finish("Success");
        acl_diagnostic::assert_redacted(&record);
        let json = serde_json::to_string(&record).expect("structural diagnostic serialization");
        assert!(
            ["Completed", "identity", "acl_count", "ace", "Accepted"]
                .iter()
                .all(|value| json.contains(value))
        );

        for output in [
            format!("C:\\creds {sid}:(OI)(CI)(F)"),
            format!("C:\\creds {sid}:(DENY)(F)"),
            format!("C:\\creds {sid}:(F) extra"),
        ] {
            let expected = parse_icacls_strict_with_identity(&output, &identity, true, "C:\\creds");
            let (actual, captured) = acl_diagnostic::capture(|| {
                parse_icacls_strict_with_identity(&output, &identity, true, "C:\\creds")
            });
            let record = captured.finish("Other");
            if output.contains("DENY") {
                assert!(serde_json::to_string(&record).unwrap().contains("Deny"));
            }
            assert_eq!(actual, expected);
        }

        let ((), captured) = acl_diagnostic::capture(|| {
            for _ in 0..=acl_diagnostic::MAX_EVENTS {
                acl_diagnostic::event("probe", "ExitedNonZero");
            }
        });
        let overflow = captured.finish("WindowsAcl");
        let bytes = serde_json::to_vec(&overflow).expect("bounded diagnostic serialization");
        assert!(String::from_utf8_lossy(&bytes).contains("\"overflow\":true"));
        assert_artifact(&bytes);
    }

    #[test]
    fn retained_windows_acl_diagnostic() {
        if std::env::var_os("ARTISAN_NATIVE_ACL_DIAGNOSTIC").as_deref()
            != Some(std::ffi::OsStr::new("1"))
        {
            return;
        }
        let artifact_path = std::env::var_os("ARTISAN_NATIVE_ACL_DIAGNOSTIC_FILE").map_or_else(
            || panic!("ARTISAN_NATIVE_ACL_DIAGNOSTIC_FILE is required"),
            PathBuf::from,
        );
        assert!(artifact_path.is_absolute());
        let home = tempfile::tempdir().expect("temporary diagnostic home");
        let (result, captured) = acl_diagnostic::capture(|| provision_or_load(home.path()));
        let outcome = match &result {
            Ok(_) => "Success",
            Err(ForgeCredentialError::WindowsAcl) => "WindowsAcl",
            Err(ForgeCredentialError::Provisioning) => "Provisioning",
            Err(_) => "Other",
        };
        let record = captured.finish(outcome);
        acl_diagnostic::assert_redacted(&record);
        acl_diagnostic::write(&artifact_path, &record)
            .expect("unable to write retained ACL diagnostic artifact");
        let artifact = fs::read(&artifact_path).expect("retained ACL diagnostic artifact missing");
        assert_artifact(&artifact);
    }
}
