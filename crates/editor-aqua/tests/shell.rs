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
#[path = "shell/navigator_menu.rs"]
mod navigator_menu;
#[path = "shell/root.rs"]
mod root;
#[path = "shell/scale.rs"]
mod scale;
#[path = "shell/scroll_edge.rs"]
mod scroll_edge;
#[cfg(windows)]
#[path = "shell/uia.rs"]
mod uia;
#[path = "shell/wheel.rs"]
mod wheel;
#[path = "shell/workspace.rs"]
mod workspace;
