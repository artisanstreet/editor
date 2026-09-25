//! The profile refresh must settle even when an admitted request never replies.

use super::*;
use crate::native_application::impl_profile_usage::PROFILE_USAGE_REFRESH_TIMEOUT;
use crate::native_profile_usage::ProfileUsageGeneration;
use std::time::Duration;

fn codex_usage(percent_used: f64) -> NativeUsageEntry {
    reported_usage_entry(
        "codex",
        "Codex",
        vec![reported_usage_window(
            "weekly",
            NativeUsageCadence::Weekly,
            None,
            percent_used,
        )],
    )
}

fn latest_codex_request(
    commands: &Rc<RefCell<Vec<NativeTransportCommand>>>,
) -> (ProfileUsageGeneration, u64) {
    commands
        .borrow()
        .iter()
        .rev()
        .find_map(|command| match command {
            NativeTransportCommand::ReadAccountUsage {
                engine_id,
                generation,
                request_seq,
                ..
            } if engine_id == "codex" => Some((*generation, *request_seq)),
            _ => None,
        })
        .expect("a Codex usage read was admitted")
}

#[gpui::test]
fn profile_usage_timeout_preserves_cached_meter_and_stops_spinner(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([]);
    let cached = codex_usage(42.0);
    cx.update(|window, app| {
        app.set_reduce_motion(true);
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.profile_usage.entries.push(cached.clone());
            window.focus(&application.profile_focus, cx);
        });
    });
    cx.simulate_keystrokes("enter");
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.refresh_single_profile_engine("codex", cx);
        });
    });
    cx.run_until_parked();
    let (_, sequence) = latest_codex_request(&commands);
    cx.update(|_, app| {
        let application = view.read(app);
        assert_eq!(
            application.profile_usage.pending_seq("codex"),
            Some(sequence)
        );
        let swaps = application.profile_refresh_swap.borrow();
        assert_eq!(
            swaps.get("codex").expect("refresh is rendered").displayed,
            [0.0, 0.0, 1.0]
        );
    });

    cx.executor()
        .advance_clock(PROFILE_USAGE_REFRESH_TIMEOUT - Duration::from_millis(1));
    cx.run_until_parked();
    assert_eq!(
        cx.update(|_, app| view.read(app).profile_usage.pending_seq("codex")),
        Some(sequence),
        "the flight remains pending until its deadline"
    );
    cx.executor().advance_clock(Duration::from_millis(1));
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert_eq!(application.profile_usage.pending_seq("codex"), None);
        assert!(
            !application
                .profile_usage
                .refreshing_engine_ids
                .iter()
                .any(|id| id == "codex")
        );
        let entry = application
            .profile_usage
            .entry("codex")
            .expect("cached usage");
        assert_eq!(entry.report, cached.report);
        assert_eq!(entry.fetched_at_ms, cached.fetched_at_ms);
        assert!(
            entry
                .failure
                .as_deref()
                .is_some_and(|failure| failure.contains("timed out"))
        );
        let swaps = application.profile_refresh_swap.borrow();
        let swap = swaps.get("codex").expect("refresh is still rendered");
        assert_eq!(swap.displayed[2], 0.0, "the spinner is no longer painted");
        assert_eq!(swap.to[2], 0.0);
    });
    assert!(
        cx.debug_bounds("artisan-profile-usage-meter-codex-weekly")
            .is_some()
    );
    assert!(
        cx.debug_bounds("artisan-profile-usage-failure-codex")
            .is_some()
    );
}

#[gpui::test]
fn profile_usage_retry_ignores_old_timeout_and_late_response(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([]);
    let cached = codex_usage(42.0);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.profile_usage.entries.push(cached.clone());
            application.refresh_single_profile_engine("codex", cx);
        });
    });
    cx.run_until_parked();
    let (generation, first_sequence) = latest_codex_request(&commands);
    cx.executor().advance_clock(Duration::from_secs(20));
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.refresh_single_profile_engine("codex", cx);
        });
    });
    cx.run_until_parked();
    let (_, retry_sequence) = latest_codex_request(&commands);
    assert_ne!(retry_sequence, first_sequence);
    cx.executor()
        .advance_clock(PROFILE_USAGE_REFRESH_TIMEOUT - Duration::from_secs(20));
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert_eq!(
                application.profile_usage.pending_seq("codex"),
                Some(retry_sequence)
            );
            assert_eq!(application.profile_usage.entry("codex"), Some(&cached));
            application.handle_account_usage(
                "codex",
                generation,
                first_sequence,
                codex_usage(99.0),
                cx,
            );
            assert_eq!(
                application.profile_usage.pending_seq("codex"),
                Some(retry_sequence)
            );
            assert_eq!(application.profile_usage.entry("codex"), Some(&cached));
        });
    });
    cx.executor().advance_clock(Duration::from_secs(20));
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert_eq!(application.profile_usage.pending_seq("codex"), None);
        let entry = application
            .profile_usage
            .entry("codex")
            .expect("cached usage");
        assert_eq!(entry.report, cached.report);
        assert!(
            entry
                .failure
                .as_deref()
                .is_some_and(|failure| failure.contains("timed out"))
        );
    });
}

#[gpui::test]
fn profile_usage_success_is_not_replaced_when_its_deadline_fires(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.refresh_single_profile_engine("codex", cx);
        });
    });
    cx.run_until_parked();
    let (generation, sequence) = latest_codex_request(&commands);
    let response = codex_usage(18.0);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_account_usage("codex", generation, sequence, response.clone(), cx);
        });
    });
    cx.executor().advance_clock(PROFILE_USAGE_REFRESH_TIMEOUT);
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert_eq!(application.profile_usage.pending_seq("codex"), None);
        assert_eq!(application.profile_usage.entry("codex"), Some(&response));
    });
}

#[gpui::test]
fn profile_usage_connection_reset_fences_the_old_deadline(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.refresh_single_profile_engine("codex", cx);
        });
    });
    cx.run_until_parked();
    let (old_generation, old_sequence) = latest_codex_request(&commands);
    cx.executor().advance_clock(Duration::from_secs(20));
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.reset_profile_usage_for_connection();
            application.refresh_single_profile_engine("codex", cx);
        });
    });
    cx.run_until_parked();
    let (generation, sequence) = latest_codex_request(&commands);
    assert_ne!(generation, old_generation);
    cx.executor()
        .advance_clock(PROFILE_USAGE_REFRESH_TIMEOUT - Duration::from_secs(20));
    cx.run_until_parked();
    let response = codex_usage(18.0);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert_eq!(
                application.profile_usage.pending_seq("codex"),
                Some(sequence)
            );
            assert!(
                application
                    .profile_usage
                    .entry("codex")
                    .expect("new pending usage")
                    .failure
                    .is_none()
            );
            application.handle_account_usage_failed(
                "codex",
                old_generation,
                old_sequence,
                command_failure(CommandSendError::Stopped),
                cx,
            );
            assert_eq!(
                application.profile_usage.pending_seq("codex"),
                Some(sequence)
            );
            application.handle_account_usage("codex", generation, sequence, response.clone(), cx);
        });
    });
    cx.executor().advance_clock(Duration::from_secs(20));
    cx.run_until_parked();
    cx.update(|_, app| {
        let application = view.read(app);
        assert_eq!(application.profile_usage.pending_seq("codex"), None);
        assert_eq!(application.profile_usage.entry("codex"), Some(&response));
    });
}
