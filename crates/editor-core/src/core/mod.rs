pub mod access;
pub mod account;
pub mod ae;
mod agent_calls;
pub mod client;
pub mod clock;
pub mod completion;
pub mod config_file;
pub mod connection;
pub mod delivery;
pub mod host;
pub mod hosts;
pub mod mcp_sign_in;
pub mod media_http;
pub mod problem;
pub mod range_window;
pub mod resident;
pub mod runtime;
pub mod screen;
pub mod services;
pub mod settings;
mod settings_writer;
pub mod store;

pub use access::EntryGate;
pub use account::{
    AUTH_ORIGIN, Account, AccountEvent, AccountLink, AccountMachine, AccountPhase, LocalForge,
};
pub use ae::{Ae, AeError, RunAe};
pub use client::ForgeClient;
pub use clock::{Clock, TimeSource};
pub use completion::{CompletionKey, CompletionScope, CompletionSlot};
pub use connection::ConnectionStatus;
pub use host::{
    Attention, BlobImage, DEFAULT_ENGINE_CONFIG_KEY, HostData, HostEvent, HostInstance,
    HostSettingError, HostSettings, HostSwitch, LiveSnapshot, LoadThreadError, MCP_SERVERS_KEY,
    McpServersError, ProjectionLoad, ThreadPage, ThreadPageDirection, ThreadSummary,
    ThreadUnavailable, WslStatus,
};
pub use hosts::{
    DataDirError, EditorFiles, EditorSettings, HostConfig, HostId, HostKind, IdentityError,
};
pub use mcp_sign_in::{McpSignIn, McpSignInPhase};
pub use problem::Problem;
pub use range_window::{CollectionWindow, ContentWindow, RangePage, RangeState, RangeWindow};
pub(crate) use resident::CollectionBytes;
pub use resident::ResidentBytes;
pub use screen::{DeepLinkError, DraggedThread, Screen, SettingsPage, SettingsSection};
pub use services::{DialRelay, RelayDialRequest, RelayRoute, Services};
pub use settings::{
    Browser, EditorPreferences, Fonts, SetSettingError, Settings, StoredSettingError, Streaming,
};
pub use settings_writer::{SettingsSave, SettingsShutdown, drain_settings};
pub use store::{Core, CoreEvent, OpenCoreError};
pub mod diagnostics;
