//! Fixture and engine-state tests for the native settings surface.
//!
//! Extracted verbatim from `native_settings.rs` during the module split.

use super::*;

#[test]
fn default_shell_selects_models() {
    let shell = SettingsShell::new();
    assert_eq!(shell.selected(), SettingsSection::Models);
    assert!(shell.is_selected(SettingsSection::Models));
    assert!(!shell.is_selected(SettingsSection::Threads));
    assert_eq!(SettingsShell::default().selected(), SettingsSection::Models);
}

#[test]
fn nav_selection_reports_changes() {
    let mut shell = SettingsShell::new();
    assert!(!shell.select(SettingsSection::Models));
    assert!(shell.select(SettingsSection::Threads));
    assert_eq!(shell.selected(), SettingsSection::Threads);
    assert!(shell.is_selected(SettingsSection::Threads));
    assert!(shell.select(SettingsSection::Appearance));
    assert_eq!(shell.selected(), SettingsSection::Appearance);
}

#[test]
fn section_hrefs_match_legacy_settings_routes() {
    assert_eq!(SettingsSection::Models.href(), "/settings/models");
    assert_eq!(SettingsSection::Appearance.href(), "/settings/appearance");
    assert_eq!(
        SettingsSection::Engines.href(),
        "/settings/engines/fixture-engine"
    );
    assert_eq!(
        SettingsSection::Notifications.href(),
        "/settings/notifications"
    );
    assert_eq!(SettingsSection::Privacy.href(), "/settings/privacy");
    assert_eq!(SettingsSection::Threads.href(), "/settings/threads");
    assert_eq!(SettingsSection::About.href(), "/settings/about");
    assert_eq!(SettingsSection::ALL.len(), 7);
}

#[test]
fn href_resolution_covers_every_section() {
    for section in SettingsSection::ALL {
        assert_eq!(section_for_href(section.href()), Some(section));
    }
    assert_eq!(
        section_for_href("/settings/engines/other-engine"),
        Some(SettingsSection::Engines)
    );
    assert_eq!(
        section_for_href("/settings/models/"),
        Some(SettingsSection::Models)
    );
    assert_eq!(section_for_href("/settings/unknown"), None);
}

#[test]
fn every_section_mounts_with_copy_and_anchors() {
    for section in SettingsSection::ALL {
        let snapshot = section_snapshot(section);
        assert_eq!(snapshot.section, section);
        assert_eq!(snapshot.title, section.title());
        assert_eq!(snapshot.description, section.description());
        assert!(!snapshot.title.is_empty());
        assert!(!snapshot.description.is_empty());
        assert!(!snapshot.anchors.is_empty());
        assert!(!snapshot.primitives.is_empty());
    }
}

#[test]
fn shell_outlet_follows_selection() {
    let mut shell = SettingsShell::new();
    for section in SettingsSection::ALL {
        assert!(shell.select(section) || shell.selected() == section);
        let outlet = shell.outlet();
        assert_eq!(outlet.section, section);
        assert_eq!(outlet, section_snapshot(section));
    }
}

#[test]
fn section_anchors_match_legacy_nav() {
    let hashes = |section: SettingsSection| {
        section
            .anchors()
            .iter()
            .map(|anchor| anchor.hash)
            .collect::<Vec<_>>()
    };
    assert_eq!(hashes(SettingsSection::Models), ["compaction", "favorites"]);
    assert_eq!(
        hashes(SettingsSection::Threads),
        ["retention", "usage-recovery", "agents"]
    );
    assert_eq!(hashes(SettingsSection::Notifications), ["system"]);
    assert_eq!(
        hashes(SettingsSection::Privacy),
        ["telemetry", "never-collected"]
    );
    assert_eq!(hashes(SettingsSection::About), ["build"]);
}

#[test]
fn fixture_policies_resolve_through_existing_helpers() {
    let models = fixture_models();
    assert_eq!(models.len(), 2);
    assert_eq!(models_for_fixture_engine(FIXTURE_ENGINE_ID).len(), 2);
    assert!(models_for_fixture_engine("unknown-engine").is_empty());
    assert_eq!(
        thinking_for_fixture_model(&models[0]),
        Some(ThinkingLevel::Medium)
    );
    assert_eq!(thinking_for_fixture_model(&models[1]), None);

    assert_eq!(resolve_fixture_telemetry(None), fixture_telemetry());
    assert!(fixture_retention_is_valid());
    assert_eq!(
        fixture_retention_state().policy(),
        Some(fixture_retention_policy())
    );

    let recovery = fixture_usage_recovery();
    assert!(!recovery.switch_disabled());

    assert!(fixture_notifications().enabled);
    assert_eq!(fixture_notification_default(), fixture_notifications());
    assert_eq!(fixture_engine_status(), EngineSettingsStatus::Ready);
    assert!(fixture_engine_template().contains("profile_id="));
}

#[test]
fn primitive_recipes_resolve_for_fixture_theme() {
    assert_eq!(nav_tab_specs().len(), 7);
    assert_eq!(nav_tab_specs()[0].label(), "Models");
    // Legacy sticky-nav order (nav.svelte): Models, Threads,
    // Appearance, Notifications, Privacy, then About, Engines group last.
    let specs = nav_tab_specs();
    let labels: Vec<&str> = specs.iter().map(TabSpec::label).collect();
    assert_eq!(
        labels,
        [
            "Models",
            "Threads",
            "Appearance",
            "Notifications",
            "Privacy",
            "About",
            "Engines"
        ]
    );
    let _ = fixture_card_style();
    let _ = fixture_switch_style(true);
    let _ = fixture_switch_style(false);
    let _ = fixture_tabs_style();
    let _ = fixture_toggle_group_style();
    let _ = fixture_tooltip_style();
    assert!(fixture_collapsible_state(true).is_open());
    assert!(!fixture_collapsible_state(false).is_open());
    assert!(
        SettingsSection::Appearance
            .primitives()
            .contains(&SettingsPrimitive::ToggleGroup)
    );
}

mod settings_screen_tests {
    use super::*;
    use crate::native_route::NativeRoute;

    #[test]
    fn route_mapping_covers_every_section() {
        assert_eq!(
            settings_section_for_route(SettingsRoute::Models),
            SettingsSection::Models
        );
        assert_eq!(
            settings_section_for_route(SettingsRoute::Appearance),
            SettingsSection::Appearance
        );
        assert_eq!(
            settings_section_for_route(SettingsRoute::Engines),
            SettingsSection::Engines
        );
        assert_eq!(
            settings_section_for_route(SettingsRoute::Notifications),
            SettingsSection::Notifications
        );
        assert_eq!(
            settings_section_for_route(SettingsRoute::Privacy),
            SettingsSection::Privacy
        );
        assert_eq!(
            settings_section_for_route(SettingsRoute::Threads),
            SettingsSection::Threads
        );
        assert_eq!(
            settings_section_for_route(SettingsRoute::About),
            SettingsSection::About
        );
    }

    #[test]
    fn screen_selector_matches_route_selector_suffix() {
        for route in [
            SettingsRoute::Models,
            SettingsRoute::Appearance,
            SettingsRoute::Engines,
            SettingsRoute::Notifications,
            SettingsRoute::Privacy,
            SettingsRoute::Threads,
            SettingsRoute::About,
        ] {
            assert_eq!(
                settings_screen_selector(route),
                NativeRoute::Settings {
                    section: route,
                    engine: None,
                }
                .selector_suffix()
            );
        }
        assert_eq!(
            settings_screen_selector(SettingsRoute::Models),
            "route-settings-models"
        );
    }

    #[test]
    fn engine_label_prefers_explicit_then_id_then_fixture() {
        assert_eq!(
            resolve_engine_label(Some("Custom"), Some("codex")),
            "Custom"
        );
        assert_eq!(resolve_engine_label(None, Some("codex")), "codex");
        assert_eq!(resolve_engine_label(None, None), FIXTURE_ENGINE_LABEL);
        assert_eq!(
            unknown_engine_description("nope"),
            "No engine with id \"nope\" exists in the catalog."
        );
    }

    #[test]
    fn disabled_engine_hides_account_and_model_anchors() {
        let full = visible_anchors(SettingsSection::Engines, true);
        assert_eq!(full.len(), 4);
        let reduced = visible_anchors(SettingsSection::Engines, false);
        assert_eq!(reduced.len(), 2);
        assert_eq!(reduced[0].hash, "availability");
        assert_eq!(reduced[1].hash, "installation");
        assert_eq!(
            visible_anchors(SettingsSection::Models, false).len(),
            SettingsSection::Models.anchors().len()
        );
    }

    #[test]
    fn gap_notices_match_legacy_desktop_copy() {
        assert!(notification_gap_notice(SystemNotificationGap::None).is_none());
        assert!(notification_gap_notice(SystemNotificationGap::Unsupported).is_none());
        let (blocked_title, _) =
            notification_gap_notice(SystemNotificationGap::Blocked).expect("blocked notice");
        assert_eq!(blocked_title, "Blocked by your system");
        let (unprompted_title, _) =
            notification_gap_notice(SystemNotificationGap::Unprompted).expect("unprompted notice");
        assert_eq!(unprompted_title, "Not allowed yet");
    }

    #[test]
    fn appearance_literals_match_legacy_defaults() {
        assert_eq!(AppearanceTimeFormat::TwelveHour.as_str(), "12-hour");
        assert_eq!(AppearanceTimeFormat::TwentyFourHour.as_str(), "24-hour");
        assert_eq!(AppearancePathSeparator::Backslash.character(), "\\");
        assert_eq!(AppearancePathSeparator::ForwardSlash.character(), "/");
        assert_eq!(AppearancePathSeparator::Backslash.label(), "Backslash");
        assert_eq!(APPEARANCE_DEFAULT_TEXT_FONT, "Spline Sans");
        assert_eq!(APPEARANCE_DEFAULT_CODE_FONT, "Spline Sans Mono");
        assert_eq!(AGENT_NAME_DATASET_DEFAULT, "norwegian");
        assert_eq!(AGENT_NAME_DATASETS.len(), 2);
        assert_eq!(ProseWidth::Balanced.as_str(), "balanced");
    }

    #[test]
    fn telemetry_captions_cover_every_choice() {
        assert_eq!(
            telemetry_choice_caption(TelemetryPreference::Unset),
            "Not decided"
        );
        assert_eq!(telemetry_choice_caption(TelemetryPreference::Enabled), "On");
        assert_eq!(
            telemetry_choice_caption(TelemetryPreference::Disabled),
            "Off"
        );
    }

    fn live_snapshot(
        readiness: crate::native_profile_usage::EngineReadinessVerdict,
    ) -> SettingsEngineSnapshot {
        SettingsEngineSnapshot {
            engine_id: "codex".to_owned(),
            readiness,
            account_email: None,
            refresh_failure: None,
            refreshing: false,
            catalog: SettingsEngineCatalogState::Loading,
            catalog_error: None,
            registry: SettingsEngineRegistryState::Missing,
            selected_thread: None,
            saved_model: None,
            saved_profile: None,
            displayed_model: None,
            displayed_authoritative: false,
            can_save_displayed: false,
            pending_save: false,
            save_failed: false,
            choice_notice: None,
            models: Vec::new(),
            install: None,
        }
    }

    #[test]
    fn live_engine_states_name_the_probed_verdict() {
        use crate::native_profile_usage::EngineReadinessVerdict;

        let ready = SettingsEngineSnapshot {
            account_email: Some("owner@example.test".to_owned()),
            ..live_snapshot(EngineReadinessVerdict::Ready)
        };
        assert_eq!(ready.availability_badge(), "Available");
        assert!(ready.installation_state().contains("owner@example.test"));
        assert!(ready.account_state().contains("owner@example.test"));

        let signin = live_snapshot(EngineReadinessVerdict::NeedsSignIn);
        assert_eq!(signin.availability_badge(), "Sign-in required");
        // A responding executable proves installation even without sign-in.
        assert!(signin.installation_state().contains("Installed"));
        assert!(signin.account_state().contains("No account"));

        let checking = live_snapshot(EngineReadinessVerdict::Checking);
        assert_eq!(checking.availability_badge(), "Checking");

        // An unchecked engine never claims a missing installation.
        let unknown = live_snapshot(EngineReadinessVerdict::NotReady);
        assert_eq!(unknown.availability_badge(), "Unavailable");
        assert!(!unknown.installation_state().contains("install"));
        assert!(!unknown.installation_state().contains("repair"));

        let failed = SettingsEngineSnapshot {
            refresh_failure: Some("provider usage read timed out".to_owned()),
            ..live_snapshot(EngineReadinessVerdict::NotReady)
        };
        assert!(
            failed
                .installation_state()
                .contains("provider usage read timed out")
        );
        assert!(
            failed
                .account_state()
                .contains("provider usage read timed out")
        );
    }

    #[test]
    fn dashboard_cursor_states_never_claim_a_local_installation() {
        use crate::native_profile_usage::EngineReadinessVerdict;

        // Dashboard auth proves the account, never a runnable local CLI.
        let ready = SettingsEngineSnapshot {
            engine_id: "cursor".to_owned(),
            account_email: Some("owner@example.test".to_owned()),
            ..live_snapshot(EngineReadinessVerdict::Ready)
        };
        assert_eq!(ready.availability_badge(), "Signed in");
        assert!(ready.installation_state().contains("unverified"));
        assert!(!ready.installation_state().contains("Installed"));
        assert!(ready.installation_state().contains("owner@example.test"));
        assert!(ready.account_state().contains("owner@example.test"));

        let signin = SettingsEngineSnapshot {
            engine_id: "cursor".to_owned(),
            ..live_snapshot(EngineReadinessVerdict::NeedsSignIn)
        };
        assert_eq!(signin.availability_badge(), "Sign-in required");
        assert!(signin.installation_state().contains("unverified"));
        assert!(!signin.installation_state().contains("Installed"));
        assert!(signin.account_state().contains("No account"));
    }
}

mod about_tests {
    use std::path::Path;

    use artisan_build_info::{BuildIdentity, BuildInfo, Channel, FORMAT_VERSION, UnstagedBuild};

    use super::about_rows;

    fn labels(rows: &[(&'static str, String)]) -> Vec<&'static str> {
        rows.iter().map(|(label, _)| *label).collect()
    }

    #[test]
    fn installed_builds_show_their_recorded_identity() {
        let identity = BuildIdentity::Installed(BuildInfo {
            format_version: FORMAT_VERSION,
            version: "0.4.0-dev.12+g1a2b3c4d5e.dirty".to_owned(),
            channel: Channel::Dev,
            commit: Some("1a2b3c4d5e6f".to_owned()),
            dirty: true,
            profile: "dev".to_owned(),
            target: "x86_64-pc-windows-msvc".to_owned(),
            built_at: None,
        });
        let rows = about_rows(&identity, Some(Path::new("/root/versions/v/bin/editor")));
        assert_eq!(
            labels(&rows),
            [
                "Version",
                "Channel",
                "Commit",
                "Profile",
                "Target",
                "Executable"
            ]
        );
        assert_eq!(rows[1].1, "Dev");
        assert_eq!(rows[2].1, "1a2b3c4d5e6f (with uncommitted changes)");
    }

    #[test]
    fn unstaged_builds_say_so_instead_of_inventing_a_commit() {
        let identity = BuildIdentity::Unstaged(UnstagedBuild::this_binary());
        let rows = about_rows(&identity, None);
        assert_eq!(labels(&rows), ["Version", "Channel", "Profile", "Target"]);
        assert_eq!(rows[1].1, "Unstaged");
    }
}
