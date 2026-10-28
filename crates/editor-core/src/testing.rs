//! A signed-in, onboarded `Core` for component tests of other crates. It
//! never reaches Core, the Relay or a real `ae`; the hosts saved in its
//! `hosts.json` connect directly.

use crate::{Core, CoreEvent, EditorFiles, ForgeClient, HostConfig, Services};
use anyhow::{Context as _, Result};
use artisan_account::network::{AccountProfile, Jwk, MachineEntry};
use gpui::Context;

impl Core {
    pub fn for_tests(files: EditorFiles, cx: &mut Context<Self>) -> Result<Self> {
        let services = Services::scripted(
            "http://127.0.0.1:1",
            std::sync::Arc::new(|_| anyhow::bail!("no `ae` in component tests")),
            None,
        );
        let mut core = Self::with_services(files, services, cx)?;
        let machine = MachineEntry {
            id: "mch_test0000000000000000000000".to_owned(),
            label: "Test Forge".to_owned(),
            public_key: Jwk {
                kty: "OKP".to_owned(),
                crv: "Ed25519".to_owned(),
                x: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            },
            created_at: "2026-01-01T00:00:00.000Z".to_owned(),
            revoked_at: None,
            last_seen_at: None,
        };
        let profile = AccountProfile {
            account_id: "acct_test00000000000000000000".to_owned(),
            email: "test@example.invalid".to_owned(),
            display_name: "Test".to_owned(),
            totp_enrolled: false,
            machines: vec![machine.clone()],
            latest_config_version: None,
        };
        core.account.update(cx, |account, cx| {
            account.sign_in_for_tests(profile, machine.id, cx);
        });
        core.gate = core.account.read(cx).entry_gate();
        let identity = core.identity.clone().context("test identity")?;
        for config in core.hosts_file.hosts.clone() {
            let client = ForgeClient::new(&config.address, &config.fingerprint, &identity)?;
            core.connect(&config, client, cx);
        }
        Ok(core)
    }

    pub fn add_test_host(
        &mut self,
        config: &HostConfig,
        client: ForgeClient,
        cx: &mut Context<Self>,
    ) {
        let mut file = self.hosts_file.clone();
        file.hosts.retain(|known| known.id != config.id);
        file.hosts.push(config.clone());
        file.last = Some(config.id.clone());
        if let Err(error) = self.files.save_hosts(&file) {
            tracing::warn!("test hosts.json: {error:#}");
        }
        self.hosts_file = file;
        self.connect(config, client, cx);
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
    }
}

/// Saves the record a finished setup leaves, so a test can resume straight
/// into `Ready` without knowing where or how it is stored.
pub fn save_onboarding_for_tests(
    files: &EditorFiles,
    base_url: &str,
    account_id: &str,
    machine_id: &str,
) -> Result<()> {
    crate::access::save_onboarding(
        &files.dir,
        &crate::access::OnboardingRecord {
            account_id: account_id.to_owned(),
            base_url: base_url.to_owned(),
            machine_id: machine_id.to_owned(),
        },
    )
}
