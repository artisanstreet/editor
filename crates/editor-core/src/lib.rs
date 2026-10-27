//! The Editor's connection to its Forges, and navigation.
//!
//! [`Core`] owns the paired hosts and the current [`Screen`]. Each host's
//! [`HostData`] is what that Forge has sent, updated only by its event
//! stream. Sections read from these and call Forge through
//! [`ForgeClient`]; nothing else is shared between sections.

mod access;
mod agent_calls;
mod account;
mod client;
mod connection;
mod host;
pub mod hosts;
mod mcp_sign_in;
pub mod media_http;
mod pairing_targets;
mod range_window;
mod resident;
pub mod runtime;
mod screen;
pub use pairing_targets::PairingTargets;
pub use resident::{CollectionBytes, ResidentBytes};

pub use access::{EntryGate, LocalForgeBinding, OnboardingMethod, OnboardingRecord};
pub use account::{
    Account, AccountEvent, AccountPhase, RelayDialRequest, RelayDialer, RelayDialerFactory,
    CORE_BASE_URL_DEFAULT, RELAY_BASE_URL_DEFAULT,
};
pub use artisan_account::network::SessionToken;

pub use client::{ForgeClient, relay_dialer_factory};
pub use connection::ConnectionStatus;
pub use host::{
    Attention, BlobImage, HostData, HostEvent, ImageDecoder, LiveSnapshot, ProjectionLoad,
    ThreadPage, ThreadPageDirection, ThreadSummary,
};
pub use hosts::{EditorFiles, EditorSettings, HostConfig, HostId, HostKind, LocalTarget};
pub use mcp_sign_in::{McpSignIn, McpSignInPhase};
pub use range_window::{
    CollectionWindow, ContentWindow, RangePage, RangeState, RangeWindow, RangeWindowEvent,
};
pub use ropey::Rope;
pub use screen::{DraggedThread, Screen, SettingsPage, SettingsSection};

use anyhow::{Context as _, Result};
use artisan_api::tls::Identity;
use gpui::{App, AppContext as _, Context, Entity, EventEmitter};
use hosts::HostsFile;
use std::collections::HashMap;

pub struct Core {
    pub files: EditorFiles,
    pub identity: Option<Identity>,
    pub settings: EditorSettings,
    hosts: HashMap<HostId, Entity<HostData>>,
    hosts_file: HostsFile,
    pairing_targets: PairingTargets,
    screen: Screen,
    back: Vec<Screen>,
    /// Deferred workspace intent while locked. Set by gated `navigate`
    /// when a workspace screen arrives before `Ready`; applied once on
    /// `Ready` if the host still exists, dropped on account switch.
    pending_screen: Option<Screen>,
    /// Relay auto-connect attempted for the current Ready machine. Guards
    /// the restart path (Ready + empty hosts) against duplicate dials from
    /// repeated account events; cleared on lock/switch. Manual Connect
    /// remains available after a failed attempt.
    relay_autoconnected: Option<String>,
    /// Cached Ready identity `(base_url, account_id)` maintained
    /// synchronously by `reconcile_access`. `hosts()`/`host()`/
    /// `active_host()` gate on this with no `cx`, no disk IO, and no
    /// account read; `entry_gate(cx)` stays the UI-facing API.
    gate_ready: Option<(String, String)>,
    /// Cached onboarding for the current origin+account. Loaded once per
    /// account/base transition (never per-frame disk IO); cleared on
    /// sign-out/switch. `entry_gate` reads this, not the file.
    onboarding_cache: Option<crate::access::OnboardingRecord>,
    /// The origin+account the cache was loaded for.
    onboarding_scope: Option<(String, String)>,
    /// The device name sent when pairing.
    pub device_name: String,
    /// MCP sign-ins this Editor catches the redirect for.
    mcp_sign_ins: mcp_sign_in::McpSignIns,
    /// The AST account controller: sign-in state, sessions, machines.
    account: Entity<Account>,
    /// Live account Relay entries (`connect_with_client`), host id to the
    /// owning account id. Never persisted: the reconciliation below drops
    /// whatever no longer belongs to the session on sign-out or account
    /// switch, so cached account views and streams can't outlive it.
    account_hosts: HashMap<HostId, String>,
    /// Ephemeral Relay host configs, keyed by id. `connect_with_client`
    /// registers here; public `hosts()`/`active_host()` include these so a
    /// connected account Forge is usable (sidebar, composer) once `Ready`.
    /// Never written to `hosts.json`; dropped with the route on lock.
    account_configs: HashMap<HostId, HostConfig>,
    /// Session token those entries were opened under. A replaced token
    /// closes every account channel too, even when the account id is
    /// unchanged: old grants and the old dialer are dead.
    account_session: Option<String>,
}

#[derive(Clone, Debug)]
pub enum CoreEvent {
    ScreenChanged,
    HostsChanged,
    SettingsChanged,
    /// Account-only gate moved: mount/drop the workspace.
    AccessChanged,
    /// An MCP sign-in started, moved on, failed or finished.
    McpSignInChanged,
}

impl EventEmitter<CoreEvent> for Core {}

/// Guidance shared by the production pairing rejection and the Forge
/// agent-call rejection: legacy direct pairing has no exact binding proof,
/// so enrollment is account-side until proven adoption exists.
pub(crate) const DIRECT_PAIRING_DISABLED: &str =
    "direct pairing is disabled: enroll this Forge into your account from the Account page, then connect it there";

/// Explicit opt-in test state for unrelated component tests (dev-only,
/// never production).
///
/// `for_tests` builds the real [`Core::new`] (no legacy auto-connect, no
/// production bypass), installs a fixture authenticated account plus its
/// proven onboarding record, then connects the saved test hosts. The entry
/// gate reads a genuine `Ready` from that mock state, so component
/// navigation and hosts work. Native entry/gate tests must keep using the
/// real `Core::new` plus an HTTP session fixture.
#[cfg(any(test, feature = "test-support"))]
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
            method: crate::access::OnboardingMethod::Relay,
            completed_at: "0".to_owned(),
            host_id: Some(machine.id.clone()),
            fingerprint: None,
        };
        if crate::access::save_onboarding(&core.files.dir, &record).is_ok() {
            core.onboarding_cache = Some(record);
            core.onboarding_scope = Some((base_url, profile.account_id.clone()));
        }
        // Dev-only: skip the restart auto-dial (no real Relay in component
        // tests); the record above already proves the binding.
        core.relay_autoconnected = Some(machine.id.clone());
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
        config: HostConfig,
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
        self.connect(&config, client, cx);
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
    }
}

impl Core {
    /// Loads this device's files. Hosts stay disconnected until the
    /// account-only gate reaches `Ready`: no legacy auto-connect prior to
    /// account readiness, no workspace flash while `Restoring`.
    pub fn new(files: EditorFiles, cx: &mut Context<Self>) -> Result<Self> {
        let identity = files.identity()?;
        let hosts_file = files.hosts()?;
        let settings = files.settings().unwrap_or_else(|error| {
            tracing::warn!("ignored unreadable settings: {error:#}");
            EditorSettings::new()
        });
        let files_dir = files.dir.clone();
        let mut core = Self {
            files,
            identity: Some(identity),
            settings,
            hosts: HashMap::new(),
            hosts_file,
            pairing_targets: PairingTargets::default(),
            screen: Screen::Home,
            back: Vec::new(),
            pending_screen: None,
            relay_autoconnected: None,
            gate_ready: None,
            onboarding_cache: None,
            onboarding_scope: None,
            device_name: device_name(),
            mcp_sign_ins: mcp_sign_in::McpSignIns::default(),
            account: cx.new(|cx| Account::new(EditorFiles { dir: files_dir }, cx)),
            account_hosts: HashMap::new(),
            account_configs: HashMap::new(),
            account_session: None,
        };
        // Exact startup install Vilde requests: build his transport factory
        // from the account's session-authed Core client and register it on
        // Core (which forwards to the account controller). `Account::new`
        // installs the same factory as a default so standalone controller
        // tests still exercise login/resume; this call makes the app
        // bootstrap path explicit and keeps `set_relay_dialer_factory` wired.
        let factory = crate::client::relay_dialer_factory(
            core.account.read(cx).session_client(),
            cx.background_executor().clone(),
        );
        core.set_relay_dialer_factory(factory, cx);
        // Account transitions drive the gate: lock hosts on sign-out,
        // refresh the onboarding cache on auth, connect deferred hosts on
        // `Ready`. The subscription is installed before any async resume
        // completes, so startup has no flash or missed event.
        cx.subscribe(&core.account, |core, account, event, cx| {
            if matches!(event, AccountEvent::Changed) {
                core.reconcile_access(&account, cx);
            }
        })
        .detach();
        if let Some(reduced) = core
            .settings
            .get("reduced_motion")
            .and_then(serde_json::Value::as_bool)
        {
            cx.set_reduce_motion(reduced);
        }
        editor_ui::theme::set_mode(theme_mode(&core.settings), cx);
        editor_ui::theme::set_fonts(font_set(&core.settings), cx);
        set_outline_text(outline_text(&core.settings), cx);
        editor_ui::streaming::set_mode(streaming_mode(&core.settings), cx);
        Ok(core)
    }

    fn connect(
        &mut self,
        config: &HostConfig,
        client: ForgeClient,
        cx: &mut Context<Self>,
    ) -> Entity<HostData> {
        let (id, name) = (config.id.clone(), config.name.clone());
        let host = cx.new(|cx| {
            let mut host = HostData::new(id, name, client, cx);
            host.set_pairing(&config.kind, &config.address);
            host
        });
        cx.subscribe(&host, |core, host, event, cx| match event {
            HostEvent::WslDistro { distro } => {
                let id = host.read(cx).id.clone();
                core.remember_wsl_distro(&id, distro);
            }
            HostEvent::EditorCall { id, call } => {
                core.editor_call(&host, id.clone(), call.clone(), cx);
            }
            HostEvent::Reset
            | HostEvent::ImagesChanged
            | HostEvent::EngineInstall { .. }
            | HostEvent::ThreadSummaryChanged { .. }
            | HostEvent::Changed
            | HostEvent::ThreadChanged { .. }
            | HostEvent::Item(_)
            | HostEvent::Live { .. } => {}
        })
        .detach();
        self.hosts.insert(config.id.clone(), host.clone());
        host
    }

    /// Saves that a host's Forge runs in WSL `distro`, so the Editor can
    /// start it there when it is unreachable.
    fn remember_wsl_distro(&mut self, id: &HostId, distro: &str) {
        let mut file = self.hosts_file.clone();
        let Some(config) = file.hosts.iter_mut().find(|config| &config.id == id) else {
            return;
        };
        config.kind = HostKind::Wsl {
            distro: distro.to_owned(),
        };
        match self.files.save_hosts(&file) {
            Ok(()) => self.hosts_file = file,
            Err(error) => tracing::warn!("couldn't save the host's WSL distribution: {error:#}"),
        }
    }

    /// Visible paired hosts. Empty unless the gate is `Ready`: setup uses
    /// private internals, public UI never sees content while locked.
    /// Gated on the synchronously maintained Ready identity: no `cx`,
    /// no disk IO, no account read on this hot path.
    pub fn hosts(&self) -> impl Iterator<Item = (&HostConfig, &Entity<HostData>)> {
        let ready = self.gate_ready.is_some();
        self.hosts_file
            .hosts
            .iter()
            .filter_map(|config| self.hosts.get(&config.id).map(|host| (config, host)))
            .chain(
                self.account_configs
                    .values()
                    .filter_map(|config| self.hosts.get(&config.id).map(|host| (config, host))),
            )
            .filter(move |_| ready)
    }

    #[must_use]
    pub fn host(&self, id: &HostId) -> Option<&Entity<HostData>> {
        if self.gate_ready.is_none() {
            return None;
        }
        self.hosts.get(id)
    }

    /// True until the user explicitly removes this pairing, including connection failures.
    pub fn is_paired(&self, id: &HostId) -> bool {
        self.hosts_file.hosts.iter().any(|host| &host.id == id)
    }

    /// The current screen's host, then the saved last host, then the first paired host.
    /// `None` unless `Ready`; locked states never leak a host. Gated on the
    /// synchronously maintained Ready identity: no `cx` on this hot path.
    #[must_use]
    pub fn active_host(&self) -> Option<&Entity<HostData>> {
        if self.gate_ready.is_none() {
            return None;
        }
        self.screen
            .host()
            .and_then(|id| self.hosts.get(id))
            .or_else(|| {
                self.hosts_file
                    .last
                    .as_ref()
                    .and_then(|id| self.hosts.get(id))
            })
            .or_else(|| {
                self.hosts_file
                    .hosts
                    .iter()
                    .filter_map(|config| self.hosts.get(&config.id))
                    .chain(
                        self.account_configs
                            .values()
                            .filter_map(|config| self.hosts.get(&config.id)),
                    )
                    .next()
            })
    }

    /// Saves a newly paired host and connects to it.
    ///
    /// Forbidden legacy access: unverified direct configs (`Local`/`Wsl`/
    /// `Remote`) are rejected in every gate state until exact direct binding
    /// proof exists. Relay routes enter via `connect_with_client` with a
    /// live client; new Forges enroll through the account (claim/confirm)
    /// then connect from the Account page.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "callers hand over a freshly paired host; the saved copy is kept on success"
    )]
    pub fn add_host(&mut self, config: HostConfig, _cx: &mut Context<Self>) -> Result<()> {
        if !matches!(config.kind, HostKind::Account { .. }) {
            anyhow::bail!(crate::DIRECT_PAIRING_DISABLED);
        }
        anyhow::bail!("account Forges connect from the Account page, not by pairing")
    }

    /// The AST account controller: sign-in state, sessions, machines.
    #[must_use]
    pub const fn account(&self) -> &Entity<Account> {
        &self.account
    }

    /// Account-only entry gate for `EntryRoot`. Cached per origin+account
    /// transition (no per-frame disk IO); `Restoring` holds the workspace
    /// unmounted with no host connects; only `Ready` exposes content.
    #[must_use]
    pub fn entry_gate(&self, cx: &gpui::App) -> EntryGate {
        let account = self.account.read(cx);
        match account.phase() {
            AccountPhase::Restoring => EntryGate::Restoring,
            AccountPhase::Authenticated { profile } => {
                let account_id = profile.account_id.clone();
                let email = profile.email.clone();
                let onboarded = self.onboarding_cache.as_ref().is_some_and(|record| {
                    Some(&record.account_id) == Some(&account_id)
                        && Some(&record.base_url) == Some(&account.base_url().to_owned())
                        && profile.machines.iter().any(|machine| {
                            machine.id == record.machine_id && machine.revoked_at.is_none()
                        })
                }) && self.onboarding_scope.as_ref().is_some_and(|(base, aid)| {
                    base == account.base_url() && aid == &account_id
                });
                if onboarded {
                    EntryGate::Ready { account_id }
                } else {
                    EntryGate::NeedsSetup { account_id, email }
                }
            }
            AccountPhase::SignedOut
            | AccountPhase::Requesting
            | AccountPhase::Pending { .. }
            | AccountPhase::EmailApproved { .. }
            | AccountPhase::Finalizing
            | AccountPhase::Error { .. } => EntryGate::SignedOut,
        }
    }

    /// Guarded host access: `Some` only when the gate is `Ready` and the
    /// host is connected. Setup flows use private internals; public UI must
    /// go through here so `NeedsSetup`/locked states never leak content.
    #[must_use]
    pub fn require_host(
        &self,
        id: &HostId,
        cx: &gpui::App,
    ) -> Option<gpui::Entity<HostData>> {
        if !matches!(self.entry_gate(cx), EntryGate::Ready { .. }) {
            return None;
        }
        self.hosts.get(id).cloned()
    }

    /// Whether `machine_id` has a live account route under the current
    /// session. Used at `complete_setup` time; metadata alone is not enough.
    #[must_use]
    pub fn connected_account_host(&self, machine_id: &str, cx: &gpui::App) -> bool {
        let account = self.account.read(cx);
        let Some(profile) = account.profile() else {
            return false;
        };
        let id = HostId(machine_id.to_owned());
        self.hosts.contains_key(&id)
            && self.account_hosts.get(&id).is_some_and(|owner| *owner == profile.account_id)
            && self.hosts.get(&id).is_some_and(|host| {
                let host = host.read(cx);
                host.client.is_relay() && host.is_live()
            })
    }

    /// Registers the Relay dialer factory (Vilde's transport, once at
    /// startup via the account controller; the controller installs
    /// a session-bound dialer on every login and resumed session).
    /// (Kept on Core so the app bootstrap has one stable call site even
    /// before any session exists; the live dialer itself lives on Account.)
    pub fn set_relay_dialer_factory(&mut self, factory: RelayDialerFactory, cx: &mut Context<Self>) {
        self.account.update(cx, |account, _| {
            account.set_relay_dialer_factory(factory);
        });
    }

    /// Adds an in-memory host entry for a connected Relay Forge and connects
    /// to it. This is the shared inventory path the account controller uses
    /// with Vilde's relay-backed client: entries are deliberately NOT
    /// persisted to hosts.json and are NOT ordinary direct hosts. On restart
    /// the machine inventory is re-discovered on login and the route
    /// re-establishes on Connect, so no stale relay address is ever
    /// direct-dialed. Direct pairing still uses `add_host`.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "callers hand over an owned host entry; the saved inventory keeps its own clone"
    )]
    /// Adds an in-memory host entry for a connected Relay Forge. Validates
    /// the current account/session generation at insertion: stale
    /// `connect_machine` callbacks after sign-out/switch are dropped, not
    /// just unsubscribed. Allowed in `Ready` and `NeedsSetup` (Relay
    /// onboarding); rejected while locked.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "callers hand over an owned host entry; the saved inventory keeps its own clone"
    )]
    pub fn connect_with_client(
        &mut self,
        config: HostConfig,
        client: ForgeClient,
        cx: &mut Context<Self>,
    ) {
        let (phase_ok, owner_ok) = self.account.read_with(cx, |account, _| {
            let phase_ok = matches!(account.phase(), AccountPhase::Authenticated { .. });
            let owner_ok = match &config.kind {
                HostKind::Account { account: owner, machine } => account
                    .profile()
                    .is_some_and(|profile| &profile.account_id == owner)
                    && account.can_complete_setup(machine)
                    && account.session_token().is_some(),
                _ => false,
            };
            (phase_ok, owner_ok)
        });
        if !phase_ok || !owner_ok {
            return;
        }
        if let HostKind::Account { account, .. } = &config.kind {
            self.account_hosts.insert(config.id.clone(), account.clone());
            // Ephemeral inventory so `hosts()`/`active_host()` expose the
            // Relay route once `Ready`. Never persisted to `hosts.json`.
            self.account_configs.insert(config.id.clone(), config.clone());
        }
        self.hosts.remove(&config.id);
        self.connect(&config, client, cx);
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
    }

    /// Completes onboarding against a real connected account-owned Forge.
    /// Validates the live profile (non-revoked machine), the Core origin
    /// scope, and an actual live connection: Relay requires a live
    /// account route for `machine_id`; Direct requires a locally enrolled
    /// binding plus a live direct host. Metadata alone never completes.
    /// Persists per-origin+account, connects deferred hosts, applies any
    /// deferred navigation. Setup internals may read private host state
    /// while public `hosts()` stays hidden until `Ready`.
    pub fn complete_setup(
        &mut self,
        machine_id: String,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let (profile, base_url) = self.account.read_with(cx, |account, _| {
            (
                account.profile().cloned(),
                account.base_url().to_owned(),
            )
        });
        let Some(profile) = profile else {
            anyhow::bail!("sign in first");
        };
        let machine = profile
            .machines
            .iter()
            .find(|machine| machine.id == machine_id)
            .context("that Forge is no longer on the account")?;
        if machine.revoked_at.is_some() {
            anyhow::bail!("that Forge was removed from the account");
        }
        let account_id = profile.account_id.clone();
        // Live connection proof: the exact mapped Relay route must be live
        // (disconnected placeholders don't count). Direct adoption has no
        // exact local-route proof in the current API (a same-device live
        // candidate and matching key do not tie the candidate TLS identity
        // to the enrolled machine, especially across WSL/data dirs), so it
        // is rejected honestly instead of persisting an assumed binding.
        // Same Forge Hub data is preserved: onboarding goes through
        // verified Relay; direct pairings stay on disk awaiting a proven
        // adoption flow.
        let relay_live = self.hosts.contains_key(&HostId(machine_id.clone()))
            && self
                .account_hosts
                .get(&HostId(machine_id.clone()))
                .is_some_and(|owner| *owner == account_id)
            && self
                .hosts
                .get(&HostId(machine_id.clone()))
                .is_some_and(|host| {
                    host.read_with(cx, |host, _| host.client.is_relay() && host.is_live())
                });
        if !relay_live {
            anyhow::bail!(
                "connect the Forge through Relay first, then finish setup (direct adoption is not available yet; your direct pairings stay on disk)"
            );
        }
        let (method, host_id, fingerprint) = (
            crate::access::OnboardingMethod::Relay,
            Some(machine_id.clone()),
            None,
        );
        let completed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs().to_string())
            .unwrap_or_default();
        let record = crate::access::OnboardingRecord {
            account_id: account_id.clone(),
            base_url: base_url.clone(),
            machine_id: machine_id.clone(),
            method,
            completed_at,
            host_id,
            fingerprint,
        };
        crate::access::save_onboarding(&self.files.dir, &record)?;
        self.onboarding_cache = Some(record);
        self.onboarding_scope = Some((base_url.clone(), account_id.clone()));
        self.gate_ready = Some((base_url, account_id));
        // The Relay route proving setup is already live; no legacy
        // direct dial here. Unknown hosts stay disconnected.
        cx.emit(CoreEvent::AccessChanged);
        cx.notify();
        let pending = self.pending_screen.take();
        if let Some(screen) = pending {
            self.navigate(screen, cx);
        }
        Ok(())
    }

    /// Restarts with a cached Relay onboarding must not dead-end on the
    /// legacy pair UI: `Ready` + empty hosts auto-reconnects the recorded
    /// machine through the account after borrow release. Guarded to one
    /// attempt per Ready scope; manual Connect stays available afterwards.
    /// Unknown legacy direct hosts are never dialed here.
    fn maybe_autoconnect_relay(
        &mut self,
        account: &Entity<Account>,
        machine_id: String,
        cx: &mut Context<Self>,
    ) {
        if self.relay_autoconnected.as_deref() == Some(machine_id.as_str()) {
            return;
        }
        if self.hosts.contains_key(&HostId(machine_id.clone())) {
            return;
        }
        self.relay_autoconnected = Some(machine_id.clone());
        let core = cx.entity();
        account.update(cx, |account, cx| {
            account.connect_machine(machine_id, &core, cx);
        });
    }

    /// Drops host entities without touching persisted pairings or Forge disk
    /// data. Closes Relay routes first so view-held clones stop at once.
    fn drop_hosts(&mut self, ids: &[HostId], cx: &mut Context<Self>) {
        for id in ids {
            if let Some(host) = self.hosts.get(id) {
                host.read_with(cx, |host, _| host.client.close_relay());
            }
            self.hosts.remove(id);
            self.account_hosts.remove(id);
            self.account_configs.remove(id);
        }
        if self.screen.host().is_some_and(|host| ids.contains(host)) {
            self.screen = Screen::Home;
            cx.emit(CoreEvent::ScreenChanged);
        }
        self.back
            .retain(|screen| screen.host().is_none_or(|host| !ids.contains(host)));
        if self.hosts_file.last.as_ref().is_some_and(|last| ids.contains(last)) {
            let mut file = self.hosts_file.clone();
            file.last = None;
            if self.files.save_hosts(&file).is_ok() {
                self.hosts_file = file;
            }
        }
        // Deferred intent pointing at a dropped host is stale.
        if self
            .pending_screen
            .as_ref()
            .and_then(|screen| screen.host())
            .is_some_and(|host| ids.contains(host))
        {
            self.pending_screen = None;
        }
    }

    /// Locks all host access: closes every route, drops every entity,
    /// navigates home, prunes host back-stack. Retains disk data
    /// (`hosts.json`, Forge DBs); backend runs on Forges keep running.
    fn lock_all_hosts(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<HostId> = self.hosts.keys().cloned().collect();
        self.drop_hosts(&ids, cx);
        self.pending_screen = self.pending_screen.take().filter(|screen| screen.host().is_none());
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
    }

    /// Account transitions drive the gate: cached onboarding loads once per
    /// origin+account, hosts lock on sign-out, deferred hosts connect on
    /// `Ready`. Stale previous-account back-stack is pruned; deferred
    /// deeplinks survive `Restoring` but never load guarded content locked.
    fn reconcile_access(&mut self, account: &Entity<Account>, cx: &mut Context<Self>) {
        let (phase, profile, token, base_url) = account.read_with(cx, |account, _| {
            (
                account.phase().clone(),
                account.profile().cloned(),
                account.session_token().map(|token| token.as_str().to_owned()),
                account.base_url().to_owned(),
            )
        });
        let rotated = self.account_session.as_deref() != token.as_deref();
        self.account_session = token.clone();
        match (&phase, profile) {
            (AccountPhase::Restoring, _) => {
                self.onboarding_cache = None;
                self.onboarding_scope = None;
                self.relay_autoconnected = None;
                self.gate_ready = None;
                self.lock_all_hosts(cx);
                cx.emit(CoreEvent::AccessChanged);
                cx.notify();
            }
            (AccountPhase::Authenticated { .. }, Some(profile)) => {
                let account_id = profile.account_id.clone();
                let scope = (base_url.clone(), account_id.clone());
                if self.onboarding_scope.as_ref() != Some(&scope) {
                    self.onboarding_cache =
                        crate::access::load_onboarding(&self.files.dir, &base_url, &account_id);
                    self.onboarding_scope = Some(scope);
                }
                let ready = self.onboarding_cache.as_ref().is_some_and(|record| {
                    record.account_id == account_id
                        && record.base_url == base_url
                        && profile.machines.iter().any(|machine| {
                            machine.id == record.machine_id && machine.revoked_at.is_none()
                        })
                });
                if ready {
                    self.gate_ready = Some((base_url.clone(), account_id.clone()));
                    // Drop rotated or foreign account routes, keep current.
                    let stale: Vec<HostId> = self
                        .account_hosts
                        .iter()
                        .filter(|&(_, owner)| rotated || *owner != account_id)
                        .map(|(id, _)| id.clone())
                        .collect();
                    if !stale.is_empty() {
                        self.drop_hosts(&stale, cx);
                        self.relay_autoconnected = None;
                    }
                    // Restart path: cached Relay onboarding with empty hosts
                    // auto-reconnects the recorded machine through the
                    // account (after borrow release). Ready Home shows the
                    // account machine inventory, never the legacy pair UI.
                    if let Some(record) = self.onboarding_cache.clone()
                        && record.method == crate::access::OnboardingMethod::Relay
                        && record.account_id == account_id
                        && record.base_url == base_url
                    {
                        let machine = record.machine_id.clone();
                        if !self.hosts.contains_key(&HostId(machine.clone())) {
                            self.maybe_autoconnect_relay(account, machine, cx);
                        }
                    }
                    cx.emit(CoreEvent::AccessChanged);
                    cx.notify();
                    let pending = self.pending_screen.take();
                    if let Some(screen) = pending {
                        self.navigate(screen, cx);
                    }
                } else {
                    // Setup: keep only current account routes for onboarding,
                    // drop every direct host; public inventory stays hidden.
                    self.gate_ready = None;
                    self.relay_autoconnected = None;
                    let mut stale: Vec<HostId> = self
                        .hosts
                        .keys()
                        .filter(|id| !self.account_hosts.contains_key(*id))
                        .cloned()
                        .collect();
                    stale.extend(
                        self.account_hosts
                            .iter()
                            .filter(|&(_, owner)| rotated || *owner != account_id)
                            .map(|(id, _)| id.clone()),
                    );
                    if !stale.is_empty() {
                        self.drop_hosts(&stale, cx);
                    }
                    cx.emit(CoreEvent::AccessChanged);
                    cx.notify();
                }
            }
            _ => {
                // SignedOut and all in-flight sign-in steps: locked.
                self.onboarding_cache = None;
                self.onboarding_scope = None;
                self.relay_autoconnected = None;
                self.gate_ready = None;
                // A replaced token retires channels even under the same id;
                // here there is no valid session at all, so lock everything.
                self.lock_all_hosts(cx);
                cx.emit(CoreEvent::AccessChanged);
                cx.notify();
            }
        }
    }

    pub fn remove_host(&mut self, id: &HostId, cx: &mut Context<Self>) -> Result<()> {
        let mut file = self.hosts_file.clone();
        file.hosts.retain(|known| &known.id != id);
        if file.last.as_ref() == Some(id) {
            file.last = None;
        }
        self.files.save_hosts(&file)?;
        self.hosts_file = file;
        self.account_hosts.remove(id);
        self.account_configs.remove(id);
        self.hosts.remove(id);
        if self.screen.host() == Some(id) {
            self.navigate(Screen::Home, cx);
        }
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
        Ok(())
    }

    #[must_use]
    pub const fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Home is only the pairing entry: with a host it is a new thread there.
    /// Locked states stay on `Home`; only `Ready` auto-advances.
    fn landing(&self, screen: Screen, cx: &App) -> Screen {
        if !matches!(self.entry_gate(cx), EntryGate::Ready { .. }) {
            return match screen {
                Screen::Settings(SettingsPage::Account) | Screen::Home => screen,
                _ => Screen::Home,
            };
        }
        match (screen, self.active_host()) {
            (Screen::Home, Some(host)) => Screen::NewThread {
                host: host.read(cx).id.clone(),
                project: None,
            },
            (screen, _) => screen,
        }
    }

    /// Whether `screen` may load while locked. Only `Home` and the Account
    /// settings page are unguarded; everything else defers until `Ready`
    /// and never loads guarded content while locked.
    fn screen_allowed_while_locked(screen: &Screen) -> bool {
        matches!(screen, Screen::Home | Screen::Settings(SettingsPage::Account))
    }

    pub fn navigate(&mut self, screen: Screen, cx: &mut Context<Self>) {
        // Locked: stash workspace intent, never load guarded content.
        if !matches!(self.entry_gate(cx), EntryGate::Ready { .. })
            && !Self::screen_allowed_while_locked(&screen)
        {
            self.pending_screen = Some(screen);
            return;
        }
        let screen = self.landing(screen, cx);
        if let Screen::Thread { host, thread } = &screen
            && let Some(host) = self.hosts.get(host).cloned()
        {
            host.update(cx, |host, cx| host.ensure_thread(thread, cx));
        }
        if screen == self.screen {
            return;
        }
        if let Some(host) = screen.host()
            && self.hosts_file.last.as_ref() != Some(host)
        {
            let mut file = self.hosts_file.clone();
            file.last = Some(host.clone());
            match self.files.save_hosts(&file) {
                Ok(()) => self.hosts_file = file,
                Err(error) => tracing::warn!("couldn't remember the last host: {error:#}"),
            }
        }
        let previous = std::mem::replace(&mut self.screen, screen);
        self.back.push(previous);
        if self.back.len() > 50 {
            self.back.remove(0);
        }
        cx.emit(CoreEvent::ScreenChanged);
        cx.notify();
    }

    /// The workspace visible behind the settings dialog.
    pub fn screen_behind_settings(&self) -> Option<&Screen> {
        self.back
            .iter()
            .rev()
            .find(|screen| !matches!(screen, Screen::Settings(_)))
    }

    /// The workspace route underneath settings, or the current route otherwise.
    pub fn workspace_screen(&self) -> &Screen {
        if matches!(self.screen, Screen::Settings(_)) {
            self.screen_behind_settings().unwrap_or(&self.screen)
        } else {
            &self.screen
        }
    }

    /// Returns to the screen Settings was opened from, past the settings
    /// pages visited since; else to a new thread on the active host.
    pub fn close_settings(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.screen, Screen::Settings(_)) {
            return;
        }
        if let Some(index) = self
            .back
            .iter()
            .rposition(|screen| !matches!(screen, Screen::Settings(_)))
        {
            self.back.truncate(index + 1);
            self.go_back(cx);
            return;
        }
        self.back.clear();
        self.screen = self.landing(Screen::Home, cx);
        cx.emit(CoreEvent::ScreenChanged);
        cx.notify();
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(screen) = self.back.pop() {
            let screen = self.landing(screen, cx);
            if let Screen::Thread { host, thread } = &screen
                && let Some(host) = self.hosts.get(host).cloned()
            {
                host.update(cx, |host, cx| host.ensure_thread(thread, cx));
            }
            self.screen = screen;
            cx.emit(CoreEvent::ScreenChanged);
            cx.notify();
        }
    }

    /// Stores a device-local preference.
    pub fn set_setting(&mut self, key: &str, value: serde_json::Value, cx: &mut Context<Self>) {
        if self.settings.get(key) == Some(&value) {
            return;
        }
        let previous = self.settings.insert(key.to_owned(), value);
        if let Err(error) = self.files.save_settings(&self.settings) {
            match previous {
                Some(previous) => {
                    self.settings.insert(key.to_owned(), previous);
                }
                None => {
                    self.settings.remove(key);
                }
            }
            tracing::error!("couldn't save settings: {error:#}");
            cx.notify();
            return;
        }
        if key == "reduced_motion" {
            cx.set_reduce_motion(
                self.settings
                    .get(key)
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            );
        }
        if key == "dark_mode" {
            editor_ui::theme::set_mode(theme_mode(&self.settings), cx);
        }
        if key == FONT_SET_KEY {
            editor_ui::theme::set_fonts(font_set(&self.settings), cx);
        }
        if key == OUTLINE_TEXT_KEY {
            set_outline_text(outline_text(&self.settings), cx);
        }
        if key == STREAMING_MODE_KEY {
            editor_ui::streaming::set_mode(streaming_mode(&self.settings), cx);
        }
        cx.emit(CoreEvent::SettingsChanged);
        cx.notify();
    }

    /// The theme mode this device's settings choose.
    #[must_use]
    pub fn theme_mode(&self) -> editor_ui::theme::ThemeMode {
        theme_mode(&self.settings)
    }

    /// The font set this device's settings choose.
    #[must_use]
    pub fn font_set(&self) -> editor_ui::theme::FontSet {
        font_set(&self.settings)
    }

    /// Whether this device draws text from its exact outline.
    #[must_use]
    pub fn outline_text(&self) -> bool {
        outline_text(&self.settings)
    }

    /// How this device reveals a streaming reply.
    #[must_use]
    pub fn streaming_mode(&self) -> editor_ui::streaming::StreamingMode {
        streaming_mode(&self.settings)
    }
}

/// `"spline"` or `"system"`: the families text is drawn in.
pub const FONT_SET_KEY: &str = "font_set";
/// Draw text from unhinted outlines in grayscale (`TextRenderingMode::Outline`).
pub const OUTLINE_TEXT_KEY: &str = "outline_text";
/// `"smooth"`, `"swift"` or `"instant"`: how a streaming reply appears.
pub const STREAMING_MODE_KEY: &str = "streaming_mode";

fn streaming_mode(settings: &EditorSettings) -> editor_ui::streaming::StreamingMode {
    editor_ui::streaming::StreamingMode::from_key(
        settings
            .get(STREAMING_MODE_KEY)
            .and_then(serde_json::Value::as_str),
    )
}

fn font_set(settings: &EditorSettings) -> editor_ui::theme::FontSet {
    editor_ui::theme::FontSet::from_key(
        settings
            .get(FONT_SET_KEY)
            .and_then(serde_json::Value::as_str),
    )
}

fn outline_text(settings: &EditorSettings) -> bool {
    settings
        .get(OUTLINE_TEXT_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
}

/// Glyphs already in the atlas are keyed by mode, so the refresh rasterizes
/// every visible glyph again in the new one.
fn set_outline_text(on: bool, cx: &mut App) {
    let mode = if on {
        gpui::TextRenderingMode::Outline
    } else {
        gpui::TextRenderingMode::PlatformDefault
    };
    if cx.text_rendering_mode() != mode {
        cx.set_text_rendering_mode(mode);
        cx.refresh_windows();
    }
}

fn theme_mode(settings: &EditorSettings) -> editor_ui::theme::ThemeMode {
    let dark = settings
        .get("dark_mode")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    if dark {
        editor_ui::theme::ThemeMode::Dark
    } else {
        editor_ui::theme::ThemeMode::Light
    }
}

/// Creates the [`Core`] entity. If this device's files can't be read it
/// still returns a usable (empty) core, with the error to show.
pub fn init(cx: &mut App) -> (Entity<Core>, Option<anyhow::Error>) {
    let files = EditorFiles::default_location();
    let mut failure = None;
    let core = cx.new(|cx| match Core::new(files.clone(), cx) {
        Ok(core) => core,
        Err(error) => {
            failure = Some(error);
            Core {
                files: files.clone(),
                identity: None,
                settings: EditorSettings::new(),
                hosts: HashMap::new(),
                hosts_file: HostsFile::default(),
                pairing_targets: PairingTargets::default(),
                screen: Screen::Home,
                back: Vec::new(),
                pending_screen: None,
                relay_autoconnected: None,
                gate_ready: None,
                onboarding_cache: None,
                onboarding_scope: None,
                device_name: device_name(),
                mcp_sign_ins: mcp_sign_in::McpSignIns::default(),
                account: cx.new(|cx| Account::new(
                    EditorFiles {
                        dir: files.dir.clone(),
                    },
                    cx,
                )),
                account_hosts: HashMap::new(),
                account_configs: HashMap::new(),
                account_session: None,
                }
        }
    });
    (core, failure)
}

#[cfg(test)]
mod tests;

fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|name| name.trim().to_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Artisan Editor".to_owned())
}
