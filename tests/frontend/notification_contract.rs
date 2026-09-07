//! Focused, dependency-free coverage for the notification contract.

#[path = "../../modules/frontend/src/notification_contract.rs"]
mod notification_contract;

use notification_contract::{
    SystemNotification, SystemNotificationCategory, SystemNotificationGap,
    SystemNotificationPermission, SystemNotificationSettings, system_notification_gap_for,
    system_notifications_are_active,
};

#[test]
fn permission_values_are_typed_and_keep_the_exact_contract_strings() {
    let permissions = [
        (SystemNotificationPermission::Unsupported, "unsupported"),
        (SystemNotificationPermission::Default, "default"),
        (SystemNotificationPermission::Granted, "granted"),
        (SystemNotificationPermission::Denied, "denied"),
    ];

    for (permission, expected) in permissions {
        assert_eq!(permission.as_str(), expected);
    }
}

#[test]
fn notification_categories_are_typed_and_keep_the_exact_contract_strings() {
    let categories = [
        (SystemNotificationCategory::Approval, "approval"),
        (SystemNotificationCategory::Question, "question"),
        (SystemNotificationCategory::RunCompleted, "run_completed"),
        (SystemNotificationCategory::RunFailed, "run_failed"),
    ];

    for (category, expected) in categories {
        assert_eq!(category.as_str(), expected);
    }
}

#[test]
fn an_owned_notification_preserves_all_fields_and_each_category() {
    let fields = [
        (SystemNotificationCategory::Approval, "approval"),
        (SystemNotificationCategory::Question, "question"),
        (SystemNotificationCategory::RunCompleted, "run_completed"),
        (SystemNotificationCategory::RunFailed, "run_failed"),
    ];

    for (category, category_value) in fields {
        let notification = SystemNotification::new(
            "Body with exact punctuation.",
            category,
            "durable-notification-id",
            "/workspace/project/thread-42?raw=%2F",
            "Thread title",
        );

        assert_eq!(notification.body, "Body with exact punctuation.");
        assert_eq!(notification.category, category);
        assert_eq!(notification.category.as_str(), category_value);
        assert_eq!(notification.id, "durable-notification-id");
        assert_eq!(
            notification.route_path,
            "/workspace/project/thread-42?raw=%2F"
        );
        assert_eq!(notification.title, "Thread title");
    }
}

#[test]
fn permission_and_enabled_matrix_preserves_gap_precedence_and_activity() {
    let cases = [
        (
            false,
            SystemNotificationPermission::Unsupported,
            SystemNotificationGap::Unsupported,
            false,
        ),
        (
            true,
            SystemNotificationPermission::Unsupported,
            SystemNotificationGap::Unsupported,
            false,
        ),
        (
            false,
            SystemNotificationPermission::Default,
            SystemNotificationGap::None,
            false,
        ),
        (
            true,
            SystemNotificationPermission::Default,
            SystemNotificationGap::Unprompted,
            false,
        ),
        (
            false,
            SystemNotificationPermission::Granted,
            SystemNotificationGap::None,
            false,
        ),
        (
            true,
            SystemNotificationPermission::Granted,
            SystemNotificationGap::None,
            true,
        ),
        (
            false,
            SystemNotificationPermission::Denied,
            SystemNotificationGap::None,
            false,
        ),
        (
            true,
            SystemNotificationPermission::Denied,
            SystemNotificationGap::Blocked,
            false,
        ),
    ];

    assert_eq!(cases.len(), 2 * 4, "matrix must cover every input pair");
    for (enabled, permission, expected_gap, expected_active) in cases {
        let settings = SystemNotificationSettings::new(enabled, permission);
        assert_eq!(
            system_notification_gap_for(settings),
            expected_gap,
            "gap for enabled={enabled}, permission={}",
            permission.as_str()
        );
        assert_eq!(
            system_notifications_are_active(settings),
            expected_active,
            "active for enabled={enabled}, permission={}",
            permission.as_str()
        );
    }
}

#[test]
fn gap_values_are_typed_and_keep_the_exact_contract_strings() {
    let gaps = [
        (SystemNotificationGap::None, "none"),
        (SystemNotificationGap::Blocked, "blocked"),
        (SystemNotificationGap::Unprompted, "unprompted"),
        (SystemNotificationGap::Unsupported, "unsupported"),
    ];

    for (gap, expected) in gaps {
        assert_eq!(gap.as_str(), expected);
    }
}
