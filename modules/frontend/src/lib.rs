//! Native Artisan Editor application assembly boundary.
//!
//! The binary launches the minimal GPUI proof window, now embedding the
//! product-specific [`project_picker`] leaf. Complete product assembly,
//! navigation, and screens remain later work. Beyond that window, the
//! library hosts narrow product-state models without
//! rendering them: [`attention`], [`composer`], [`transcript`], and
//! [`thread_list_selection`].

pub mod artisan_error_code;
pub mod attention;
pub mod attention_reconnect;
pub mod command_ranking;
pub mod composer;
pub mod context_usage_description;
pub mod context_usage_model_name;
pub mod context_usage_tone;
pub mod conversation_diff_stat;
pub mod conversation_presentation;
pub mod conversation_projection;
pub mod conversation_scroll_position;
pub mod conversation_turn_navigator;
pub mod engine_usage_cache;
pub mod file_icon;
pub mod forge_recovery_health;
pub mod image_inspection_store;
pub mod image_policy;
pub mod image_viewer;
pub mod latest_request_gate;
pub mod markdown_fence_policy;
pub mod marketplace_fixture_policy;
pub mod math_rendering_policy;
pub mod motion_spring;
pub mod new_thread_interaction;
pub mod notification_events;
pub mod notification_preferences;
pub mod onboarding_route;
pub mod project_identity_policy;
pub mod project_picker;
pub mod proof;
pub mod reader_attention;
pub mod reasoning_display;
pub mod route_navigation;
pub mod runtime_fixture_policy;
pub mod runtime_fixture_support;
pub mod runtime_surface;
pub mod shell;
pub mod subscription_projection;
pub mod telemetry_bootstrap_policy;
pub mod telemetry_preferences;
pub mod thread_list_selection;
pub mod transcript;

use std::process::ExitCode;

/// Runs the currently implemented native editor boundary.
#[must_use]
pub fn run() -> ExitCode {
    proof::run()
}
