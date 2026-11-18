#![expect(
    clippy::cast_possible_truncation,
    clippy::clone_on_ref_ptr,
    clippy::default_trait_access,
    clippy::float_cmp,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::needless_pass_by_value,
    clippy::redundant_clone,
    clippy::redundant_closure_for_method_calls,
    clippy::renamed_function_params,
    clippy::too_many_lines,
    clippy::unchecked_time_subtraction,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![cfg(test)]

#[path = "shell/browser.rs"]
mod browser;
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
#[cfg(windows)]
#[path = "shell/uia.rs"]
mod uia;
#[expect(
    clippy::disallowed_methods,
    reason = "test fixture driving the raw aqua text and element APIs, not view code"
)]
#[path = "shell/wheel.rs"]
mod wheel;
#[path = "shell/workspace.rs"]
mod workspace;
