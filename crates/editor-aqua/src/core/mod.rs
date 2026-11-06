pub mod access;
pub mod account;
pub mod ae;
mod agent_calls;
pub mod client;
pub mod connection;
pub mod delivery;
pub mod host;
pub mod hosts;
pub mod mcp_sign_in;
pub mod media_http;
pub mod range_window;
pub mod resident;
pub mod runtime;
pub mod screen;
pub mod services;
pub mod store;
#[cfg(feature = "test-support")]
mod testing;
#[cfg(feature = "test-support")]
pub use testing::save_onboarding_for_tests;

pub use access::EntryGate;
pub use account::{Account, AccountEvent, AccountMachine, AccountPhase, LocalForge};
#[cfg(feature = "test-support")]
pub use ae::AeScript;
pub use client::ForgeClient;
pub use connection::ConnectionStatus;
pub use host::{
    Attention, BlobImage, HostData, HostEvent, HostInstance, LiveSnapshot, ProjectionLoad,
    ThreadPage, ThreadPageDirection, ThreadSummary, WslStatus,
};
pub use hosts::{EditorFiles, EditorSettings, HostConfig, HostId, HostKind};
pub use mcp_sign_in::{McpSignIn, McpSignInPhase};
#[cfg(feature = "test-support")]
pub use range_window::RangeCompletion;
pub use range_window::{CollectionWindow, ContentWindow, RangePage, RangeState, RangeWindow};
pub use resident::{CollectionBytes, ResidentBytes};
pub use screen::{DraggedThread, Screen, SettingsPage, SettingsSection};
#[cfg(feature = "test-support")]
pub use services::RelayScript;
pub use services::{RelayDialRequest, Services};
pub use store::{Core, CoreEvent};
