//! Native Artisan Editor application assembly boundary.
//!
//! The binary launches the shipping GPUI application, which owns the native
//! window and application entities while its transport service starts a
//! newly owned Forge and loads real project, thread, and bounded conversation
//! state. The remaining modules continue to provide the narrow product
//! policies and conversation composition used by later read-only and
//! interactive workflow slices.

pub mod active_thread_light_policy;
pub mod activity_status_labels;
pub mod approval_presentation;
pub mod artisan_error_code;
pub mod attachment_tray_policy;
pub mod attention;
pub mod attention_reconnect;
pub mod attention_title_policy;
pub mod authoritative_subscription_policy;
pub mod browser_dom_boundary;
pub mod clipboard_write_boundary;
pub mod command_ranking;
pub mod component_gallery_policy;
pub mod composer;
pub mod composer_action_failure;
pub mod composer_draft_session_policy;
pub mod composer_gesture;
pub mod context_auto_compaction;
pub mod context_usage_description;
pub mod context_usage_details_policy;
pub mod context_usage_gauge_policy;
pub mod context_usage_model_name;
pub mod context_usage_tone;
pub mod conversation_checklist;
pub mod conversation_delivery_machine;
pub mod conversation_diff_stat;
pub mod conversation_error_card_policy;
pub mod conversation_host;
pub mod conversation_presentation;
pub mod conversation_projection;
pub mod conversation_relative_age;
pub mod conversation_scene;
pub mod conversation_scroll_position;
pub mod conversation_state_machine;
pub mod conversation_status_labels;
pub mod conversation_steering;
pub mod conversation_steering_machine;
pub mod conversation_surface;
pub mod conversation_turn_footer_policy;
pub mod conversation_turn_machine;
pub mod conversation_turn_navigator;
pub mod conversation_view_machine;
pub mod dev_instance_policy;
pub mod dropdown_highlight_settle;
pub mod editor_diagnostic_mapping;
pub mod editor_language;
pub mod editor_route_gate_policy;
pub mod editor_view_state_policy;
pub mod editor_workspace_identity;
pub mod engine_section_indicator_policy;
pub mod engine_usage_cache;
pub mod file_icon;
pub mod forge_endpoint_policy;
pub mod forge_recovery_health;
pub mod forge_repair_request;
pub mod gradient_avatar;
pub mod harness_setup_policy;
pub mod host_identity_controller;
pub mod host_resume_recovery_policy;
pub mod hover_pill_geometry_policy;
pub mod hover_pill_group_policy;
pub mod image_inspection_store;
pub mod image_policy;
pub mod image_viewer;
pub mod latest_request_gate;
pub mod machine_switch;
pub mod markdown_fence_policy;
pub mod markdown_language_registry_policy;
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
pub mod native_application;
pub mod native_transport_service;
pub mod new_thread_draft;
pub mod new_thread_interaction;
pub mod new_thread_sentence_policy;
pub mod notification_contract;
pub mod notification_events;
pub mod notification_preferences;
pub mod notification_web_presenter_policy;
pub mod object_url_boundary;
pub mod onboarding_harness_presentation;
pub mod onboarding_route;
pub mod project_catalog;
pub mod project_identity_policy;
pub mod project_path_policy;
pub mod project_picker;
pub mod proof;
pub mod reader_attention;
pub mod reasoning_display;
pub mod repository_mark;
pub mod rich_link_url;
pub mod route_navigation;
pub mod route_navigation_adapter;
pub mod run_usage_policy;
pub mod runtime_fixture_policy;
pub mod runtime_fixture_support;
pub mod runtime_surface;
pub mod scoped_attachment_queue;
pub mod scoped_attachment_runner_policy;
pub mod session_projection;
pub mod session_tool_policy;
pub mod setup_label_transition_policy;
pub mod shell;
pub mod shell_command;
pub mod shell_layout;
pub mod shell_presentation_state;
pub mod speed_presentation;
pub mod steering_pending_lip;
pub mod subscription_projection;
pub mod tab_derivations;
pub mod telemetry_bootstrap_policy;
pub mod telemetry_preferences;
pub mod terminal_presentation;
pub mod thread_environment_presentation;
pub mod thread_hover_rail_policy;
pub mod thread_list_selection;
pub mod thread_navigation_core;
pub mod thread_panel_policy;
pub mod thread_read_tracker;
pub mod thread_retention_settings_policy;
pub mod thread_route_gate_policy;
pub mod thread_title_policy;
pub mod thread_title_settings_policy;
pub mod transcript;
pub mod usage_meter;
pub mod usage_recovery_settings_policy;
pub mod usage_refresh_claims;
pub mod usage_reset_duration;
pub mod usage_window_motion;
pub mod vcs_diff_presentation;
pub mod vcs_labels;
pub mod workspace_header_presentation;
pub mod workspace_tab_state;

/// Runs the currently implemented native editor boundary.
#[must_use]
pub fn run() -> std::process::ExitCode {
    native_application::run()
}
