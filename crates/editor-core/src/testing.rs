//! Explicit opt-in test state (dev-only, never production).
//!
//! `Core::for_tests` builds the real [`Core::new`] (no legacy auto-connect,
//! no production bypass), installs a fixture authenticated account plus its
//! proven onboarding record, then connects the saved test hosts. The entry
//! gate reads a genuine `Ready` from that mock state, so component
//! navigation and hosts work. Native entry/gate tests keep using the real
//! `Core::new` plus an HTTP session fixture.

use crate::{Core, CoreEvent, EditorFiles, ForgeClient, HostConfig};
use anyhow::{Context as _, Result};
use gpui::Context;

impl Core {
    pub fn for_tests(files: EditorFiles, cx: &mut Context<Self>) -> Result<Self> {
        use artisan_account::network::{AccountProfile, Jwk, MachineEntry, SessionToken};

        let mut core = Self::new(files, cx)?;
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
        let base_url = core
            .account
            .read_with(cx, |account, _| account.base_url().to_owned());
        let record = crate::access::OnboardingRecord {
            account_id: profile.account_id.clone(),
            base_url: base_url.clone(),
            machine_id: machine.id.clone(),
            completed_at: "0".to_owned(),
        };
        if crate::access::save_onboarding(&core.files.dir, &record).is_ok() {
            core.onboarding_cache = Some(record);
            core.onboarding_scope = Some((base_url, profile.account_id.clone()));
        }
        // Dev-only: skip the restart auto-dial (no real Relay in component
        // tests); the record above already proves the binding.
        core.relay_autoconnected = Some(machine.id);
        core.account.update(cx, |account, cx| {
            account.test_authenticate(profile, SessionToken::new("test-session".to_owned()), cx);
        });
        let identity = core.identity.clone().context("test identity")?;
        for config in core.hosts_file.hosts.clone() {
            if let Ok(client) = ForgeClient::new(&config.address, &config.fingerprint, &identity)
            {
                core.connect(&config, client, cx);
            }
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
        if self.files.save_hosts(&file).is_ok() {
            self.hosts_file = file;
        }
        self.connect(config, client, cx);
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
    }
}
