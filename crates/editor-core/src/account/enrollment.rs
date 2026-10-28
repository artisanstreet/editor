//! Local Forge binding and `ae` subprocess calls for account enrollment.
//!
//! The binding comes from `ae account status --json` through the ordinary
//! `ae` binary (which owns Windows→WSL forwarding): never the data file
//! directly, never human CLI text. Every `ae` invocation pins the explicit
//! Core origin.

use super::{Account, AccountEvent, valid_code_shape};
#[cfg(any(test, feature = "test-support"))]
use super::AccountPhase;
#[cfg(any(test, feature = "test-support"))]
use artisan_account::network::{AccountProfile, SessionToken};
use anyhow::{Context as _, Result, bail};
use artisan_account::normalise_code;
use gpui::Context;

impl Account {
    /// Cached local Forge binding. Refreshed only by explicit
    /// `refresh_local_binding`, never per render.
    #[must_use]
    pub fn local_binding(&self) -> Option<crate::access::LocalForgeBinding> {
        self.cached_binding.clone()
    }

    /// The authoritative account inventory can revoke a binding that `ae`
    /// still has on disk. Only an explicit revocation permits replacement.
    pub fn local_binding_revoked(&self) -> bool {
        let Some(binding) = self.cached_binding.as_ref().filter(|binding| binding.enrolled) else {
            return false;
        };
        self.profile().is_some_and(|profile| {
            binding.account_id.as_deref() == Some(profile.account_id.as_str())
                && profile.machines.iter().any(|machine| {
                    Some(machine.id.as_str()) == binding.machine_id.as_deref()
                        && machine.revoked_at.is_some()
                })
        })
    }

    /// Re-reads the local Forge binding through `ae` (explicit user or
    /// onboarding refresh) and caches it. The hot path (`local_binding`)
    /// never hits disk or subprocess. Completions from a superseded refresh
    /// or a rotated session never overwrite: the binding epoch plus the
    /// session generation both gate.
    pub fn refresh_local_binding(&mut self, cx: &mut Context<Self>) {
        let base_url = self.base_url.clone();
        let generation = self.generation;
        self.binding_epoch = self.binding_epoch.wrapping_add(1);
        let binding_epoch = self.binding_epoch;
        let runner = self.ae_runner.clone();
        cx.spawn(async move |this, cx| {
            let binding = read_ae_status_json(runner, &base_url).await;
            this.update(cx, |this, cx| {
                if this.generation != generation || this.binding_epoch != binding_epoch {
                    return;
                }
                match binding {
                    Ok(binding) => {
                        this.cached_binding = binding;
                    }
                    Err(error) => {
                        tracing::warn!("local Forge binding unreadable: {error:#}");
                    }
                }
                cx.emit(AccountEvent::Changed);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Explicit mock sign-in for unrelated component tests only. Installs a
    /// session plus an authenticated fixture profile with no network, so the
    /// entry gate reads a genuine `Ready` once the fixture onboarding record
    /// is present. Never compiled into production; native entry/gate tests
    /// use the real HTTP fixture.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn test_authenticate(
        &mut self,
        profile: AccountProfile,
        session: SessionToken,
        cx: &mut Context<Self>,
    ) {
        self.session = Some(session);
        self.bump_generation();
        self.install_relay_dialer();
        self.set_phase(AccountPhase::Authenticated { profile }, cx);
    }
}

/// Canned `ae` result for tests: exit status plus captured streams.
/// Portable across OSes: no fixture binaries, no real user files.
#[derive(Clone, Debug)]
pub struct AeOutcome {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Test seam replacing the `ae` subprocess: receives argv, returns the
/// canned outcome. Injected per-Account in tests; production always runs
/// the ordinary `ae` binary.
pub type AeRunner = std::sync::Arc<dyn Fn(Vec<String>) -> AeOutcome + Send + Sync>;

/// Runs `ae …`: exit status plus trimmed stdout. Test runners short-circuit
/// before any subprocess; without one, the ordinary `ae` binary runs in
/// every build. Every test fixture injects a runner, so tests never shell
/// out to a real `ae`.
pub(super) async fn run_ae(runner: Option<AeRunner>, args: &[&str]) -> Result<String> {
    let args: Vec<String> = args.iter().map(ToString::to_string).collect();
    let outcome = match runner {
        Some(run) => {
            // Injected runners are synchronous; run them off-thread like the
            // real subprocess so a slow fake can never wedge the caller.
            crate::runtime::runtime()
                .spawn_blocking(move || run(args))
                .await
                .map_err(|error| anyhow::anyhow!("account task did not run: {error}"))?
        }
        None => run_ae_subprocess(args).await?,
    };
    if !outcome.success {
        let detail = outcome
            .stderr
            .trim()
            .lines()
            .next_back()
            .filter(|line| !line.trim().is_empty())
            .unwrap_or("the `ae` command failed with no message");
        bail!("`ae` failed: {detail}");
    }
    let stdout = outcome.stdout.trim().to_owned();
    if stdout.is_empty() {
        bail!("`ae` printed nothing");
    }
    Ok(stdout)
}

/// Real `ae` subprocess.
async fn run_ae_subprocess(args: Vec<String>) -> Result<AeOutcome> {
    use crate::runtime::runtime;
    let output = runtime()
        .spawn(async move {
            let mut command = tokio::process::Command::new("ae");
            // No console window: the Editor is a GUI app, so a console
            // child would otherwise flash one.
            #[cfg(windows)]
            {
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                command.creation_flags(CREATE_NO_WINDOW);
            }
            command.args(&args).output().await
        })
        .await
        .map_err(|error| anyhow::anyhow!("account task did not run: {error}"))?;
    let output = output.map_err(|error| anyhow::anyhow!("could not run `ae`: {error:#}"))?;
    Ok(AeOutcome {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Runs `ae account --core <base> enroll --json` on this machine and returns
/// the actual Core-issued code. The explicit origin keeps a saved custom
/// Core from diverging via environment inheritance. Machine output is the
/// only contract: human text is never parsed (its first alphanumerics are
/// sentence words, not the code).
pub(super) async fn run_ae_enroll(
    runner: Option<AeRunner>,
    base_url: &str,
    replace: bool,
) -> Result<String> {
    let mut args = vec!["account", "--core", base_url, "enroll", "--json"];
    if replace {
        args.push("--replace");
    }
    let stdout = run_ae(runner, &args).await?;
    let parsed: serde_json::Value = serde_json::from_slice(stdout.as_bytes())
        .context("read the Forge enrollment answer")?;
    let code = normalise_code(
        parsed
            .get("code")
            .and_then(|code| code.as_str())
            .unwrap_or_default(),
    );
    if !valid_code_shape(&code) {
        bail!("started the enrollment but Core did not return a usable code");
    }
    Ok(code)
}

/// Parses `ae account status --json` into the binding cache shape. `None`
/// means unbound (nothing started, pending-unclaimed, expired, voided):
/// pair or retry. `enrolled` must be a real boolean and a false state must
/// name a recognized status: `{}`, null, a missing flag, or an unknown
/// status is a read error, never silent unbound success. `enrolled: true`
/// requires the confirmed machine and account ids; anything less is
/// incomplete, never assumed.
pub(super) fn parse_ae_status(stdout: &str) -> Result<Option<crate::access::LocalForgeBinding>> {
    let parsed: serde_json::Value =
        serde_json::from_slice(stdout.as_bytes()).context("read the Forge enrollment state")?;
    let Some(enrolled) = parsed
        .get("enrolled")
        .and_then(serde_json::Value::as_bool)
    else {
        bail!("the Forge enrollment state is incomplete");
    };
    if enrolled {
        let machine = parsed
            .get("machine_id")
            .and_then(|id| id.as_str())
            .unwrap_or_default();
        let account = parsed
            .get("account_id")
            .and_then(|id| id.as_str())
            .unwrap_or_default();
        if machine.is_empty() || account.is_empty() {
            bail!("the Forge enrollment state is incomplete");
        }
        return Ok(Some(crate::access::LocalForgeBinding {
            machine_id: Some(machine.to_owned()),
            account_id: Some(account.to_owned()),
            enrolled: true,
            claimed_by: None,
            claimed_email: None,
        }));
    }
    match parsed.get("status").and_then(|status| status.as_str()) {
        Some("claimed" | "confirmed") => {
            let account = parsed
                .get("account_id")
                .and_then(|id| id.as_str())
                .unwrap_or_default();
            if account.is_empty() {
                bail!("the Forge enrollment state is incomplete");
            }
            let email = parsed
                .get("email")
                .and_then(|email| email.as_str())
                .map(str::to_owned);
            Ok(Some(crate::access::LocalForgeBinding {
                machine_id: None,
                account_id: None,
                enrolled: false,
                claimed_by: Some(account.to_owned()),
                claimed_email: email,
            }))
        }
        Some("none" | "pending" | "expired" | "voided") => Ok(None),
        _ => bail!("the Forge enrollment state is incomplete"),
    }
}

/// Reads the local Forge binding through the ordinary `ae` binary (which
/// owns Windows→WSL forwarding): never the data file directly, so the
/// Editor sees the same identity `ae` acts on.
pub(super) async fn read_ae_status_json(
    runner: Option<AeRunner>,
    base_url: &str,
) -> Result<Option<crate::access::LocalForgeBinding>> {
    let stdout = run_ae(
        runner,
        &["account", "--core", base_url, "status", "--json"],
    )
    .await?;
    parse_ae_status(&stdout)
}

/// Renders a parsed binding as friendly operator text: what state the
/// Forge is in, never raw CLI output and never raw ids (the verified email
/// is the useful identity; account ids stay out of display).
pub(super) fn render_binding_status(
    binding: Option<&crate::access::LocalForgeBinding>,
) -> String {
    match binding {
        Some(crate::access::LocalForgeBinding { enrolled: true, .. }) => {
            "This Forge is enrolled.".to_owned()
        }
        Some(crate::access::LocalForgeBinding {
            claimed_email: Some(email),
            ..
        }) if !email.trim().is_empty() => {
            format!("Enrollment claimed. Verified email: {email}.")
        }
        Some(_) => "Enrollment claimed.".to_owned(),
        None => "No claimed Forge enrollment on this machine yet.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<Option<crate::access::LocalForgeBinding>> {
        parse_ae_status(json)
    }

    #[test]
    fn status_shapes_map_to_bindings() {
        let enrolled = parse(r#"{"enrolled":true,"machine_id":"mch_1","account_id":"acct_1"}"#)
            .unwrap()
            .unwrap();
        assert!(enrolled.enrolled);
        assert_eq!(enrolled.machine_id.as_deref(), Some("mch_1"));
        assert_eq!(enrolled.account_id.as_deref(), Some("acct_1"));
        assert_eq!(
            render_binding_status(Some(&enrolled)),
            "This Forge is enrolled."
        );
        let claimed = parse(
            r#"{"enrolled":false,"enrollment_id":"enr_1","status":"claimed","account_id":"acct_9","email":"op@example.invalid"}"#,
        )
        .unwrap()
        .unwrap();
        assert!(!claimed.enrolled);
        assert_eq!(claimed.claimed_by.as_deref(), Some("acct_9"));
        assert_eq!(
            claimed.claimed_email.as_deref(),
            Some("op@example.invalid")
        );
        assert_eq!(
            render_binding_status(Some(&claimed)),
            "Enrollment claimed. Verified email: op@example.invalid."
        );
        for pending in [
            r#"{"enrolled":false,"enrollment_id":"enr_1","status":"pending"}"#,
            r#"{"enrolled":false,"enrollment_id":"enr_1","status":"none"}"#,
            r#"{"enrolled":false,"enrollment_id":"enr_1","status":"expired"}"#,
            r#"{"enrolled":false,"enrollment_id":"enr_1","status":"voided"}"#,
        ] {
            assert!(
                parse(pending).unwrap().is_none(),
                "unbound state must read as no binding: {pending}"
            );
        }
        assert_eq!(
            render_binding_status(None),
            "No claimed Forge enrollment on this machine yet."
        );
    }

    #[test]
    fn malformed_status_is_a_read_error_not_silent_unbound() {
        for malformed in [
            "{}",
            "null",
            "not json at all",
            r#"{"status":"claimed"}"#,
            r#"{"enrolled":"yes"}"#,
            r#"{"enrolled":true}"#,
            r#"{"enrolled":true,"machine_id":"mch_1"}"#,
            r#"{"enrolled":false,"enrollment_id":"enr_1","status":"bogus"}"#,
            r#"{"enrolled":false,"enrollment_id":"enr_1","status":"claimed"}"#,
        ] {
            assert!(
                parse(malformed).is_err(),
                "malformed output must fail loudly: {malformed}"
            );
        }
    }
}
