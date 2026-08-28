//! Focused direct tests for the dependency-free web notification presenter.

#[path = "../../modules/frontend/src/notification_events.rs"]
mod notification_events;
#[path = "../../modules/frontend/src/notification_web_presenter_policy.rs"]
mod notification_web_presenter_policy;

use notification_events::{SystemNotification, SystemNotificationCategory};
use notification_web_presenter_policy::{
    ARTISAN_NOTIFICATION_ICON, PermissionRequestOutcome, SystemNotificationPermission,
    WebNotificationHandle, WebNotificationPayload, WebNotificationPresenterAction,
    WebNotificationPresenterHostAction, WebNotificationPresenterState, normalize_permission,
    permission_for_api,
};

fn notification(id: &str, title: &str, body: &str, route_path: &str) -> SystemNotification {
    SystemNotification {
        body: body.to_owned(),
        category: SystemNotificationCategory::RunCompleted,
        id: id.to_owned(),
        route_path: route_path.to_owned(),
        title: title.to_owned(),
    }
}

fn show(
    state: &mut WebNotificationPresenterState,
    value: SystemNotification,
) -> (WebNotificationHandle, WebNotificationPayload) {
    let transition = state.apply(WebNotificationPresenterAction::Show {
        notification: value,
    });
    match transition.into_actions().as_slice() {
        [WebNotificationPresenterHostAction::Post { payload, handle }] => {
            (handle.clone(), payload.clone())
        }
        actions => panic!("expected one post action, got {actions:?}"),
    }
}

fn finish_post(state: &mut WebNotificationPresenterState, handle: WebNotificationHandle) {
    assert!(
        state
            .apply(WebNotificationPresenterAction::PostSucceeded { handle })
            .is_empty()
    );
}

#[test]
fn permission_normalization_and_api_absence_are_conservative() {
    assert_eq!(
        normalize_permission("granted"),
        SystemNotificationPermission::Granted
    );
    assert_eq!(
        normalize_permission("denied"),
        SystemNotificationPermission::Denied
    );
    assert_eq!(
        normalize_permission("default"),
        SystemNotificationPermission::Default
    );
    assert_eq!(
        normalize_permission("prompt"),
        SystemNotificationPermission::Default
    );
    assert_eq!(
        permission_for_api(false, Some("granted")),
        SystemNotificationPermission::Unsupported
    );
    assert_eq!(
        permission_for_api(true, None),
        SystemNotificationPermission::Unsupported
    );
    assert_eq!(SystemNotificationPermission::Granted.as_str(), "granted");
    assert_eq!(
        SystemNotificationPermission::Unsupported.as_str(),
        "unsupported"
    );
}

#[test]
fn request_success_and_failure_use_readable_permission_without_browser_calls() {
    let mut state = WebNotificationPresenterState::new(true, Some("default"));
    assert_eq!(
        state
            .apply(WebNotificationPresenterAction::RequestPermission)
            .actions(),
        &[WebNotificationPresenterHostAction::RequestPermission]
    );

    assert!(
        state
            .apply(WebNotificationPresenterAction::RequestResolved {
                outcome: PermissionRequestOutcome::Answered("granted"),
            })
            .is_empty()
    );
    assert_eq!(state.permission(), SystemNotificationPermission::Granted);

    // A failed request retains the latest readable snapshot. No host action
    // is emitted by either failure representation.
    assert!(
        state
            .apply(WebNotificationPresenterAction::ReadPermission {
                api_available: true,
                value: Some("denied"),
            })
            .is_empty()
    );
    assert!(
        state
            .apply(WebNotificationPresenterAction::RequestResolved {
                outcome: PermissionRequestOutcome::Failed,
            })
            .is_empty()
    );
    assert_eq!(state.permission(), SystemNotificationPermission::Denied);

    assert!(
        state
            .apply(WebNotificationPresenterAction::RequestFailed {
                api_available: true,
                value: Some("granted"),
            })
            .is_empty()
    );
    assert_eq!(state.permission(), SystemNotificationPermission::Granted);
    assert_eq!(
        state.readable_permission(),
        SystemNotificationPermission::Granted
    );

    assert!(
        state
            .apply(WebNotificationPresenterAction::RequestResolved {
                outcome: PermissionRequestOutcome::FailedWithReadable {
                    api_available: true,
                    value: Some("default"),
                },
            })
            .is_empty()
    );
    assert_eq!(state.permission(), SystemNotificationPermission::Default);

    let mut unsupported = WebNotificationPresenterState::new(false, None);
    assert!(
        unsupported
            .apply(WebNotificationPresenterAction::RequestPermission)
            .is_empty()
    );
    assert_eq!(
        unsupported.permission(),
        SystemNotificationPermission::Unsupported
    );
}

#[test]
fn denied_unsupported_and_default_permissions_make_show_a_no_op() {
    for (api_available, permission) in [
        (false, None),
        (true, Some("default")),
        (true, Some("denied")),
        (true, Some("future-permission")),
    ] {
        let mut state = WebNotificationPresenterState::new(api_available, permission);
        assert!(
            state
                .apply(WebNotificationPresenterAction::Show {
                    notification: notification("id", "Title", "Body", "/route"),
                })
                .is_empty()
        );
        assert!(state.live_notifications().is_empty());
    }
}

#[test]
fn post_payload_and_successful_live_tracking_preserve_the_notification() {
    let value = notification("run-7", "Build title", "Build body", "/runs/7");
    let mut state = WebNotificationPresenterState::new(true, Some("granted"));
    let (handle, payload) = show(&mut state, value.clone());

    assert_eq!(
        payload,
        WebNotificationPayload {
            title: "Build title".to_owned(),
            body: "Build body".to_owned(),
            icon: ARTISAN_NOTIFICATION_ICON,
            tag: "run-7".to_owned(),
        }
    );
    assert!(!state.is_live("run-7"));
    finish_post(&mut state, handle.clone());
    assert_eq!(state.live_notifications().get("run-7"), Some(&value));
    assert_eq!(state.live_handle("run-7"), Some(handle));
}

#[test]
fn failed_post_never_enters_the_live_map() {
    let mut state = WebNotificationPresenterState::new(true, Some("granted"));
    let (handle, _) = show(
        &mut state,
        notification("failed", "Title", "Body", "/failed"),
    );

    assert!(
        state
            .apply(WebNotificationPresenterAction::PostFailed { handle })
            .is_empty()
    );
    assert!(state.live_notifications().is_empty());
}

#[test]
fn same_id_replacement_dismisses_before_posting_and_retires_old_handle() {
    let mut state = WebNotificationPresenterState::new(true, Some("granted"));
    let first = notification("same", "First", "First body", "/first");
    let second = notification("same", "Second", "Second body", "/second");
    let (first_handle, _) = show(&mut state, first.clone());
    finish_post(&mut state, first_handle.clone());

    let transition = state.apply(WebNotificationPresenterAction::Show {
        notification: second.clone(),
    });
    let second_handle = match transition.actions() {
        [
            WebNotificationPresenterHostAction::Dismiss { handle: dismissed },
            WebNotificationPresenterHostAction::Post { handle, .. },
        ] => {
            assert_eq!(dismissed, &first_handle);
            handle.clone()
        }
        actions => panic!("expected dismiss then post, got {actions:?}"),
    };
    assert!(!state.is_live("same"));
    finish_post(&mut state, second_handle.clone());
    assert_eq!(state.live_notifications().get("same"), Some(&second));
    assert_eq!(state.live_handle("same"), Some(second_handle.clone()));

    // Both callback kinds from the replaced host object are stale and cannot
    // remove or activate the replacement.
    assert!(
        state
            .apply(WebNotificationPresenterAction::Closed {
                handle: first_handle.clone(),
            })
            .is_empty()
    );
    assert!(
        state
            .apply(WebNotificationPresenterAction::Clicked {
                handle: first_handle,
            })
            .is_empty()
    );
    assert!(state.is_live("same"));
    assert_eq!(state.live_handle("same"), Some(second_handle));
}

#[test]
fn stale_post_results_cannot_promote_or_remove_a_replacement() {
    let mut state = WebNotificationPresenterState::new(true, Some("granted"));
    let (first_handle, _) = show(&mut state, notification("same", "First", "Body", "/first"));
    let (second_handle, _) = show(
        &mut state,
        notification("same", "Second", "Body", "/second"),
    );

    assert!(
        state
            .apply(WebNotificationPresenterAction::PostSucceeded {
                handle: first_handle.clone(),
            })
            .is_empty()
    );
    assert!(!state.is_live("same"));
    assert!(
        state
            .apply(WebNotificationPresenterAction::PostFailed {
                handle: first_handle,
            })
            .is_empty()
    );
    assert!(!state.is_live("same"));

    finish_post(&mut state, second_handle.clone());
    assert_eq!(state.live_handle("same"), Some(second_handle));
}

#[test]
fn explicit_dismissal_closes_only_the_current_live_instance() {
    let mut state = WebNotificationPresenterState::new(true, Some("granted"));
    let (handle, _) = show(
        &mut state,
        notification("dismissed", "Dismiss", "Body", "/dismiss"),
    );
    finish_post(&mut state, handle.clone());

    assert_eq!(
        state
            .apply(WebNotificationPresenterAction::Dismiss {
                id: "dismissed".to_owned(),
            })
            .actions(),
        &[WebNotificationPresenterHostAction::Dismiss { handle }]
    );
    assert!(!state.is_live("dismissed"));
    assert!(
        state
            .apply(WebNotificationPresenterAction::Dismiss {
                id: "dismissed".to_owned(),
            })
            .is_empty()
    );
}

#[test]
fn host_close_removes_exact_live_instance_and_is_idempotent() {
    let mut state = WebNotificationPresenterState::new(true, Some("granted"));
    let (handle, _) = show(
        &mut state,
        notification("closed", "Close", "Body", "/close"),
    );
    finish_post(&mut state, handle.clone());

    assert!(
        state
            .apply(WebNotificationPresenterAction::Closed {
                handle: handle.clone(),
            })
            .is_empty()
    );
    assert!(!state.is_live("closed"));
    assert!(
        state
            .apply(WebNotificationPresenterAction::Closed { handle })
            .is_empty()
    );
}

#[test]
fn activation_removes_closes_and_returns_the_exact_notification_payload() {
    let value = notification("clicked", "Click title", "Click body", "/exact/route");
    let mut state = WebNotificationPresenterState::new(true, Some("granted"));
    let (handle, _) = show(&mut state, value.clone());
    finish_post(&mut state, handle.clone());

    assert_eq!(
        state
            .apply(WebNotificationPresenterAction::Clicked {
                handle: handle.clone(),
            })
            .actions(),
        &[
            WebNotificationPresenterHostAction::Dismiss { handle },
            WebNotificationPresenterHostAction::Activate {
                notification: value
            },
        ]
    );
    assert!(!state.is_live("clicked"));
}

#[test]
fn multiple_independent_ids_have_independent_handles_and_lifetimes() {
    let mut state = WebNotificationPresenterState::new(true, Some("granted"));
    let (one_handle, _) = show(&mut state, notification("one", "One", "Body", "/one"));
    let (two_handle, _) = show(&mut state, notification("two", "Two", "Body", "/two"));
    finish_post(&mut state, one_handle.clone());
    finish_post(&mut state, two_handle.clone());

    assert_ne!(one_handle, two_handle);
    assert!(state.is_live("one"));
    assert!(state.is_live("two"));
    assert!(
        state
            .apply(WebNotificationPresenterAction::DismissHandle { handle: one_handle })
            .actions()
            .iter()
            .any(|action| matches!(
                action,
                WebNotificationPresenterHostAction::Dismiss { handle } if handle.id == "one"
            ))
    );
    assert!(!state.is_live("one"));
    assert!(state.is_live("two"));
    assert_eq!(state.live_notifications().len(), 1);
}
