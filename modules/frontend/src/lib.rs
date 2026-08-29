//! Native Artisan Editor application assembly boundary.
//!
//! The binary launches the minimal GPUI proof window, now embedding the
//! product-specific [`project_picker`] leaf. Complete product assembly,
//! navigation, and screens remain later work. Beyond that window, the
//! library hosts narrow product-state models without
//! rendering them: [`attention`], [`composer`], [`transcript`], and
//! [`thread_list_selection`].

pub mod active_thread_light_policy;
pub mod activity_status_labels;
pub mod approval_presentation;
pub mod artisan_error_code;
pub mod attention;
pub mod attention_reconnect;
pub mod attention_title_policy;
pub mod authoritative_subscription_policy;
pub mod browser_dom_boundary;
pub mod command_ranking;
pub mod component_gallery_policy;
pub mod composer;
pub mod composer_draft_session_policy;
pub mod context_usage_description;
pub mod context_usage_gauge_policy;
pub mod context_usage_model_name;
pub mod context_usage_tone;
pub mod conversation_diff_stat;
pub mod conversation_presentation;
pub mod conversation_projection;
pub mod conversation_relative_age;
pub mod conversation_scroll_position;
pub mod conversation_turn_footer_policy;
pub mod conversation_turn_navigator;
pub mod dev_instance_policy;
pub mod editor_route_gate_policy;
pub mod engine_section_indicator_policy;
pub mod engine_usage_cache;
pub mod file_icon;
pub mod forge_recovery_health;
pub mod harness_setup_policy;
pub mod hover_pill_group_policy;
pub mod image_inspection_store;
pub mod image_policy;
pub mod image_viewer;
pub mod latest_request_gate;
pub mod markdown_fence_policy;
pub mod markdown_test_parser_policy;
pub mod markdown_warmup_policy;
pub mod marketplace_fixture_policy;
pub mod math_rendering_policy;
pub mod mobile_breakpoint_policy;
pub mod model_favorites_presentation;
pub mod model_policy_controller;
pub mod model_policy_controls_presentation;
pub mod model_selection_presentation;
pub mod motion_spring;
pub mod new_thread_interaction;
pub mod new_thread_sentence_policy;
pub mod notification_events;
pub mod notification_preferences;
pub mod notification_web_presenter_policy;
pub mod onboarding_harness_presentation;
pub mod onboarding_route;
pub mod project_identity_policy;
pub mod project_picker;
pub mod proof;
pub mod reader_attention;
pub mod reasoning_display;
pub mod route_navigation;
pub mod run_usage_policy;
pub mod runtime_fixture_policy;
pub mod runtime_fixture_support;
pub mod runtime_surface;
pub mod session_tool_policy;
pub mod setup_label_transition_policy;
pub mod shell;
pub mod subscription_projection;
pub mod telemetry_bootstrap_policy;
pub mod telemetry_preferences;
pub mod thread_environment_presentation;
pub mod thread_hover_rail_policy;
pub mod thread_list_selection;
pub mod thread_panel_policy;
pub mod thread_retention_settings_policy;
pub mod thread_route_gate_policy;
pub mod thread_title_policy;
pub mod thread_title_settings_policy;
pub mod transcript;
pub mod workspace_header_presentation;

use std::process::ExitCode;

/// Runs the currently implemented native editor boundary.
#[must_use]
pub fn run() -> ExitCode {
    proof::run()
}
