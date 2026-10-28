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
#[cfg(any(test, feature = "test-support"))]
mod testing;
pub use pairing_targets::PairingTargets;
pub use resident::{CollectionBytes, ResidentBytes};

pub use access::{EntryGate, LocalForgeBinding, OnboardingRecord};
pub use account::{
    Account, AccountEvent, AccountPhase, AeOutcome, AeRunner, RelayDialRequest, RelayDialer,
    RelayDialerFactory, CORE_BASE_URL_DEFAULT, RELAY_BASE_URL_DEFAULT,
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

use anyhow::Result;
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
        let mut was_live = false;
        cx.subscribe(&host, move |core, host, event, cx| match event {
            HostEvent::WslDistro { distro } => {
                let id = host.read(cx).id.clone();
                core.remember_wsl_distro(&id, distro);
            }
            HostEvent::EditorCall { id, call } => {
                core.editor_call(&host, id.clone(), call.clone(), cx);
            }
            HostEvent::Changed => {
                let live = host.read(cx).is_live();
                if live != was_live {
                    was_live = live;
                    cx.emit(CoreEvent::HostsChanged);
                    cx.notify();
                }
            }
            HostEvent::Reset
            | HostEvent::ImagesChanged
            | HostEvent::EngineInstall { .. }
            | HostEvent::ThreadSummaryChanged { .. }
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
        self.gate_ready.as_ref()?;
        self.hosts.get(id)
    }

    /// True until the user explicitly removes this pairing, including connection failures.
    pub fn is_paired(&self, id: &HostId) -> bool {
        self.account_configs.contains_key(id)
            || self.hosts_file.hosts.iter().any(|host| &host.id == id)
    }

    /// The current screen's host, then the saved last host, then the first paired host.
    /// `None` unless `Ready`; locked states never leak a host. Gated on the
    /// synchronously maintained Ready identity: no `cx` on this hot path.
    #[must_use]
    pub fn active_host(&self) -> Option<&Entity<HostData>> {
        self.gate_ready.as_ref()?;
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

    pub fn set_relay_dialer_factory(&mut self, factory: RelayDialerFactory, cx: &mut Context<Self>) {
        self.account.update(cx, |account, _| {
            account.set_relay_dialer_factory(factory);
        });
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
                Screen::Thread { .. }
                | Screen::NewThread { .. }
                | Screen::Settings(_)
                | Screen::Marketplace
                | Screen::Usage => Screen::Home,
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
