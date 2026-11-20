#![cfg(test)]

#[path = "shell/action_failure.rs"]
mod action_failure;
#[path = "shell/application.rs"]
mod application;
#[path = "shell/browser.rs"]
mod browser;
#[path = "shell/connection.rs"]
mod connection;
#[path = "shell/dialogs.rs"]
mod dialogs;
#[path = "shell/gating.rs"]
mod gating;
#[path = "shell/host.rs"]
mod host;
#[path = "shell/lifetime.rs"]
mod lifetime;
#[path = "shell/live_throttle.rs"]
mod live_throttle;
#[path = "shell/menu_keys.rs"]
mod menu_keys;
#[path = "shell/navigation_viewport.rs"]
mod navigation_viewport;
#[path = "shell/navigator_panel.rs"]
mod navigator_panel;
#[path = "shell/notifications.rs"]
mod notifications;
#[path = "shell/read.rs"]
mod read;
#[path = "shell/refresh.rs"]
mod refresh;
#[path = "shell/reveal.rs"]
mod reveal;
#[path = "shell/root.rs"]
mod root;
#[path = "shell/scale.rs"]
mod scale;
#[path = "shell/scroll_edge.rs"]
mod scroll_edge;
#[path = "shell/settings_scroll.rs"]
mod settings_scroll;
#[path = "shell/stopwatch.rs"]
mod stopwatch;
#[expect(
    clippy::disallowed_methods,
    reason = "test fixture driving the raw aqua text and element APIs, not view code"
)]
#[path = "shell/wheel.rs"]
mod wheel;
#[path = "shell/workspace.rs"]
mod workspace;
