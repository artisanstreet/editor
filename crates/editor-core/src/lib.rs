//! The Editor's connection to its Forges, and navigation.
//!
//! [`Core`] owns the paired hosts and the current [`Screen`]. Each host's
//! [`HostData`] is what that Forge has sent, updated only by its event
//! stream. Sections read from these and call Forge through
//! [`ForgeClient`]; nothing else is shared between sections.

mod agent_calls;
mod account;
mod client;
mod connection;
mod host;
pub mod hosts;
pub mod media_http;
mod mcp_sign_in;
mod pairing_targets;
mod range_window;
mod resident;
pub mod runtime;
mod screen;
pub use pairing_targets::PairingTargets;
pub use resident::{CollectionBytes, ResidentBytes};

pub use account::{
    Account, AccountEvent, AccountPhase, RelayDialRequest, RelayDialer, RelayDialerFactory,
    CORE_BASE_URL_DEFAULT,
};
pub use artisan_account::network::SessionToken;

pub use client::{ForgeClient, relay_dialer_factory};
pub use mcp_sign_in::{McpSignIn, McpSignInPhase};
pub use connection::ConnectionStatus;
pub use host::{
    Attention, BlobImage, HostData, HostEvent, ImageDecoder, LiveSnapshot, ProjectionLoad,
    ThreadPage, ThreadPageDirection, ThreadSummary,
};
pub use hosts::{EditorFiles, EditorSettings, HostConfig, HostId, HostKind, LocalTarget};
pub use range_window::{
    CollectionWindow, ContentWindow, RangePage, RangeState, RangeWindow, RangeWindowEvent,
};
pub use ropey::Rope;
pub use screen::{DraggedThread, Screen, SettingsPage};

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
    /// An MCP sign-in started, moved on, failed or finished.
    McpSignInChanged,
}

impl EventEmitter<CoreEvent> for Core {}

impl Core {
    /// Loads this device's files and connects to every paired host.
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
            identity: Some(identity.clone()),
            settings,
            hosts: HashMap::new(),
            hosts_file,
            pairing_targets: PairingTargets::default(),
            screen: Screen::Home,
            back: Vec::new(),
            device_name: device_name(),
            mcp_sign_ins: mcp_sign_in::McpSignIns::default(),
            account: cx.new(|cx| Account::new(EditorFiles { dir: files_dir }, cx)),
            account_hosts: HashMap::new(),
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
        // SignedOut (or a replaced session identity) must immediately drop
        // account Relay inventory: cached threads and streams must not
        // outlive the session, and grant expiry is not the cleanup path.
        cx.subscribe(&core.account, |core, account, event, cx| {
            if matches!(event, AccountEvent::Changed) {
                core.reconcile_account_hosts(&account, cx);
            }
        })
        .detach();
        for config in core.hosts_file.hosts.clone() {
            let client = ForgeClient::new(&config.address, &config.fingerprint, &identity);
            if let Err(error) = client.map(|client| core.connect(&config, client, cx)) {
                tracing::error!(host = %config.id, "can't connect to {}: {error:#}", config.name);
            }
        }
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

    pub fn hosts(&self) -> impl Iterator<Item = (&HostConfig, &Entity<HostData>)> {
        self.hosts_file
            .hosts
            .iter()
            .filter_map(|config| self.hosts.get(&config.id).map(|host| (config, host)))
    }

    #[must_use]
    pub fn host(&self, id: &HostId) -> Option<&Entity<HostData>> {
        self.hosts.get(id)
    }

    /// True until the user explicitly removes this pairing, including connection failures.
    pub fn is_paired(&self, id: &HostId) -> bool {
        self.hosts_file.hosts.iter().any(|host| &host.id == id)
    }

    /// The current screen's host, then the saved last host, then the first paired host.
    #[must_use]
    pub fn active_host(&self) -> Option<&Entity<HostData>> {
        self.screen
            .host()
            .and_then(|id| self.host(id))
            .or_else(|| self.hosts_file.last.as_ref().and_then(|id| self.host(id)))
            .or_else(|| self.hosts().next().map(|(_, host)| host))
    }

    /// Saves a newly paired host and connects to it.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "callers hand over a freshly paired host; the saved copy is cloned from it"
    )]
    pub fn add_host(&mut self, config: HostConfig, cx: &mut Context<Self>) -> Result<()> {
        let identity = self.identity.as_ref().context(
            "this device's identity could not be loaded; repair its configuration before pairing",
        )?;
        let client = ForgeClient::new(&config.address, &config.fingerprint, identity)?;
        let mut file = self.hosts_file.clone();
        file.hosts.retain(|known| known.id != config.id);
        file.hosts.push(config.clone());
        file.last = Some(config.id.clone());
        self.files.save_hosts(&file)?;
        self.hosts_file = file;
        self.connect(&config, client, cx);
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
        Ok(())
    }

    /// The AST account controller: sign-in state, sessions, machines.
    #[must_use]
    pub fn account(&self) -> &Entity<Account> {
        &self.account
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
    pub fn connect_with_client(
        &mut self,
        config: HostConfig,
        client: ForgeClient,
        cx: &mut Context<Self>,
    ) {
        if let HostKind::Account { account, .. } = &config.kind {
            self.account_hosts.insert(config.id.clone(), account.clone());
        }
        self.hosts.remove(&config.id);
        self.connect(&config, client, cx);
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
    }

    /// Drops account Relay inventory that no longer belongs to the current
    /// session: entries whose recorded owner differs from the signed-in
    /// account, every entry once signed out, and — even under the same
    /// account id — every entry when the session token was replaced (old
    /// grants and the old dialer are dead). Direct hosts are untouched.
    /// Each removed host's Relay route is closed explicitly first
    /// (`ForgeClient::close_relay`: unary plus every event stream stop at
    /// once, even for views briefly holding client clones); dropping the
    /// entity then aborts its connection task and GPUI follow task. Screens
    /// pointing at removed hosts navigate away and the pruned back stack
    /// keeps `close_settings`/`go_back` from landing on them; `hosts.json`
    /// never held these entries, but a saved `last` pointing at one is
    /// cleared. No grant-expiry wait: this runs on the sign-out transition.
    fn reconcile_account_hosts(&mut self, account: &Entity<Account>, cx: &mut Context<Self>) {
        let (current, token) = account.read_with(cx, |account, _| {
            (
                account.profile().map(|profile| profile.account_id.clone()),
                account.session_token().map(|token| token.as_str().to_owned()),
            )
        });
        // A replaced token retires every channel opened under the old one.
        let rotated = self.account_session.as_deref() != token.as_deref();
        self.account_session = token;
        let stale: Vec<HostId> = self
            .account_hosts
            .iter()
            .filter_map(|(id, owner)| {
                (rotated || Some(owner) != current.as_ref()).then(|| id.clone())
            })
            .collect();
        if stale.is_empty() {
            return;
        }
        for id in &stale {
            // Close the route first so clones held by briefly open views
            // stop at once; entity drop then aborts `_connection`
            // (AbortOnDropHandle) and the GPUI follow task, and the Relay
            // channel (no background tasks of its own) closes its sockets
            // on drop with it.
            if let Some(host) = self.hosts.get(id) {
                host.read_with(cx, |host, _| host.client.close_relay());
            }
            self.hosts.remove(id);
            self.account_hosts.remove(id);
        }
        if self.screen.host().is_some_and(|host| stale.contains(host)) {
            self.navigate(Screen::Home, cx);
        }
        // Prune after navigating: `navigate` pushes the retired current
        // screen onto `back`, so pruning only beforehand would leave the
        // stale route in history (and in the settings workspace behind).
        self.back
            .retain(|screen| screen.host().is_none_or(|host| !stale.contains(host)));
        if self.hosts_file.last.as_ref().is_some_and(|last| stale.contains(last)) {
            let mut file = self.hosts_file.clone();
            file.last = None;
            if self.files.save_hosts(&file).is_ok() {
                self.hosts_file = file;
            }
        }
        cx.emit(CoreEvent::HostsChanged);
        cx.notify();
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
    fn landing(&self, screen: Screen, cx: &App) -> Screen {
        match (screen, self.active_host()) {
            (Screen::Home, Some(host)) => Screen::NewThread {
                host: host.read(cx).id.clone(),
                project: None,
            },
            (screen, _) => screen,
        }
    }

    pub fn navigate(&mut self, screen: Screen, cx: &mut Context<Self>) {
        let screen = self.landing(screen, cx);
        if let Screen::Thread { host, thread } = &screen
            && let Some(host) = self.host(host).cloned()
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
                && let Some(host) = self.host(host).cloned()
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
}

/// `"spline"` or `"system"`: the families text is drawn in.
pub const FONT_SET_KEY: &str = "font_set";
/// Draw text from unhinted outlines in grayscale (`TextRenderingMode::Outline`).
pub const OUTLINE_TEXT_KEY: &str = "outline_text";

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
                device_name: device_name(),
                mcp_sign_ins: mcp_sign_in::McpSignIns::default(),
                account: cx.new(|cx| Account::new(
                    EditorFiles {
                        dir: files.dir.clone(),
                    },
                    cx,
                )),
                account_hosts: HashMap::new(),
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
