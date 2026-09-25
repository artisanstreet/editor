//! State the Forge pushes: account usage with readiness verdicts, and a
//! subscribed thread's refined title, applied without the Editor polling.

use super::*;
use crate::native_transport_service::HostStateEvent;
use artisan_domain::{
    EngineReadiness, EngineReadinessVerdict, EngineUsageAuth, EngineUsageAuthentication,
    EngineUsageReport, EngineUsageSnapshot, QuotaSurface, ThreadRetitled,
};

fn pushed_usage(authentication: EngineUsageAuthentication, at: &str) -> NativeTransportEvent {
    let readiness = match authentication {
        EngineUsageAuthentication::Authenticated => EngineReadiness::ready(),
        _ => EngineReadiness::new(
            EngineReadinessVerdict::NeedsSignIn,
            Some("Codex account sign-in is required.".to_owned()),
        )
        .expect("verdict"),
    };
    let report = EngineUsageReport::new(
        None,
        EngineUsageAuth::new(authentication, None).expect("auth"),
        "Codex".to_owned(),
        "codex".to_owned(),
        None,
        Some(QuotaSurface::Unknown),
        Vec::new(),
    )
    .expect("report")
    .with_readiness(readiness);
    NativeTransportEvent::HostState(HostStateEvent::AccountUsage(
        EngineUsageSnapshot::new(vec![report], at.to_owned()).expect("snapshot"),
    ))
}

fn usage_reads(commands: &[NativeTransportCommand]) -> usize {
    commands
        .iter()
        .filter(|command| matches!(command, NativeTransportCommand::ReadAccountUsage { .. }))
        .count()
}

#[gpui::test]
fn pushed_verdicts_replace_the_usage_rows_without_a_read(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.handle_service_event(
                pushed_usage(
                    EngineUsageAuthentication::Unauthenticated,
                    "2026-09-25T12:00:00Z",
                ),
                cx,
            );
            let verdict = |application: &NativeApplication| {
                crate::native_profile_usage::engine_readiness(&application.profile_usage, "codex")
            };
            assert_eq!(verdict(application), EngineReadinessVerdict::NeedsSignIn);
            // The user signs in; the Forge's next read is pushed.
            application.handle_service_event(
                pushed_usage(
                    EngineUsageAuthentication::Authenticated,
                    "2026-09-25T12:02:00Z",
                ),
                cx,
            );
            assert_eq!(verdict(application), EngineReadinessVerdict::Ready);
            // A late push of an older reading never replaces a newer one.
            application.handle_service_event(
                pushed_usage(
                    EngineUsageAuthentication::Unauthenticated,
                    "2026-09-25T12:01:00Z",
                ),
                cx,
            );
            assert_eq!(verdict(application), EngineReadinessVerdict::Ready);
            // Opening Settings and the model picker's retry schedule nothing.
            application.navigate(
                NativeRoute::Settings {
                    section: SettingsRoute::Engines,
                    engine: None,
                },
                cx,
            );
            application.handle_composer_model_event(
                &crate::native_model_selector::NativeModelSelectorEvent::Retry,
                cx,
            );
            assert_eq!(usage_reads(&commands.borrow()), 0);
        });
    });
}

#[gpui::test]
fn a_pushed_title_renames_the_thread_at_once(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|window, cx| test_application(window, cx));
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.selected_project = Some(ProjectId::parse("titles").unwrap());
            application.thread_listing = Some(
                ThreadListing::new(vec![
                    thread("titled", "titles", "Explain the parser"),
                    thread("other", "titles", "Other"),
                ])
                .expect("listing"),
            );
            let before = commands.borrow().len();
            application.handle_service_event(
                NativeTransportEvent::HostState(HostStateEvent::ThreadRetitled(ThreadRetitled {
                    thread_id: ThreadId::parse("titled").unwrap(),
                    title: ThreadTitle::parse("Parser walkthrough").unwrap(),
                })),
                cx,
            );
            let titles: Vec<_> = application
                .thread_listing
                .as_ref()
                .expect("listing")
                .threads()
                .iter()
                .map(|thread| thread.title.as_str().to_owned())
                .collect();
            assert_eq!(titles, ["Parser walkthrough", "Other"]);
            assert_eq!(
                commands.borrow().len(),
                before,
                "no listing read was needed"
            );
        });
    });
}
