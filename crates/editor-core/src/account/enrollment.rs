//! Local Forge binding and `ae` subprocess calls for account enrollment.
//!
//! The binding file is read once per explicit refresh, never on the
//! hot path. Every `ae` invocation pins the explicit Core origin.

use super::{Account, AccountEvent, valid_code_shape};
#[cfg(any(test, feature = "test-support"))]
use super::AccountPhase;
#[cfg(any(test, feature = "test-support"))]
use artisan_account::network::{AccountProfile, SessionToken};
use anyhow::{Result, bail};
use artisan_account::normalise_code;
use gpui::Context;

impl Account {
    /// Cached local Forge binding. Refreshed only by explicit
    /// `refresh_local_binding`, never per render.
    #[must_use]
    pub fn local_binding(&self) -> Option<crate::access::LocalForgeBinding> {
        self.cached_binding.clone()
    }

    /// Reads the binding file once for the cached value.
    fn read_local_binding_file() -> Option<crate::access::LocalForgeBinding> {
        let path = artisan_paths::App::Forge.data().ok()?.join("ast_account.json");
        let bytes = std::fs::read(path).ok()?;
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let machine_id = parsed
            .get("machine_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let account_id = parsed
            .get("account_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        Some(crate::access::LocalForgeBinding {
            enrolled: machine_id.is_some(),
            claimed_by: None,
            machine_id,
            account_id,
        })
    }

    /// Re-reads the local Forge binding once (explicit user/onboarding
    /// action) and caches it. The hot path (`local_binding`) never hits disk.
    pub fn refresh_local_binding(&mut self, cx: &mut Context<Self>) {
        self.cached_binding = Self::read_local_binding_file();
        cx.emit(AccountEvent::Changed);
        cx.notify();
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

/// Runs `ae …` and returns human-readable output (stdout, else the last
/// stderr line). Used only for the local Forge's account commands.
pub(super) async fn run_ae(args: &[&str]) -> String {
    use crate::runtime::runtime;
    let args: Vec<String> = args.iter().map(ToString::to_string).collect();
    let result = runtime()
        .spawn(async move {
            tokio::process::Command::new("ae")
                .args(&args)
                .output()
                .await
        })
        .await;
    match result {
        Err(error) => format!("Could not run `ae`: {error:#}"),
        Ok(Err(error)) => format!("Could not run `ae`: {error:#}"),
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if output.status.success() && !stdout.is_empty() {
                return stdout;
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            stderr
                .trim()
                .lines()
                .next_back()
                .unwrap_or("The `ae` command failed with no message.")
                .to_owned()
        }
    }
}

/// Runs `ae account --core <base> enroll` on this machine and returns the
/// printed code. The explicit origin keeps a saved custom Core from
/// diverging via environment inheritance.
///
/// The human output is parsed defensively; unparseable output surfaces
/// verbatim so nothing is silently misread.
///
/// (Vilde: an `ae account enroll --json` flag would remove this parsing;
/// requested via root.)
pub(super) async fn run_ae_enroll(base_url: &str) -> Result<String> {
    use crate::runtime::runtime;
    let base_url = base_url.to_owned();
    let output = runtime()
        .spawn(async move {
            tokio::process::Command::new("ae")
                .args(["account", "--core", &base_url, "enroll"])
                .output()
                .await
        })
        .await?;
    let output = output?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`ae account enroll` failed: {}",
            stderr.trim().lines().next().unwrap_or("unknown error")
        );
    }
    for line in stdout.lines() {
        if let Some(rest) = line.split("Claim this code").nth(1) {
            let code: String = rest
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .take(16)
                .collect();
            let code = normalise_code(&code);
            if valid_code_shape(&code) {
                return Ok(code);
            }
        }
    }
    bail!(
        "started the enrollment but could not read its code; the Forge printed: {}",
        stdout.trim()
    )
}
