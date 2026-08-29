//! Focused public-API coverage for process-wide conversation commit wake hints.

#![forbid(unsafe_code)]

#[path = "../../modules/backend/src/conversation_commit_notifier.rs"]
mod conversation_commit_notifier;

use std::time::Duration;

use artisan_domain::ThreadId;
use conversation_commit_notifier::{
    ConversationCommitNotifier, ConversationCommitPublish, ConversationCommitWaitError,
};

const READY_TIMEOUT: Duration = Duration::from_secs(1);
const PENDING_TIMEOUT: Duration = Duration::from_millis(100);

fn thread(id: &str) -> ThreadId {
    ThreadId::parse(id).expect("fixture thread id should be valid")
}

async fn wait_ready(
    subscription: &mut conversation_commit_notifier::ConversationCommitSubscription,
) {
    tokio::time::timeout(READY_TIMEOUT, subscription.wait())
        .await
        .expect("published wake should arrive before the deadline")
        .expect("notifier should remain open while its owner is live");
}

async fn assert_pending(
    subscription: &mut conversation_commit_notifier::ConversationCommitSubscription,
) {
    assert!(
        tokio::time::timeout(PENDING_TIMEOUT, subscription.wait())
            .await
            .is_err(),
        "wait should remain pending"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn publish_before_subscription_is_unobserved_and_not_replayed() {
    let notifier = ConversationCommitNotifier::new();
    let thread_id = thread("thread-before-subscription");

    assert_eq!(
        notifier.publish(&thread_id),
        ConversationCommitPublish::Unobserved
    );

    let mut subscription = notifier
        .subscribe(thread_id)
        .expect("later subscription should be accepted");
    assert_pending(&mut subscription).await;
}

#[tokio::test(flavor = "current_thread")]
async fn subscription_retains_publish_before_first_wait() {
    let notifier = ConversationCommitNotifier::default();
    let thread_id = thread("thread-before-wait");
    let mut subscription = notifier
        .subscribe(thread_id.clone())
        .expect("subscription should be accepted");

    assert_eq!(
        notifier.publish(&thread_id),
        ConversationCommitPublish::Notified
    );
    wait_ready(&mut subscription).await;
    assert_pending(&mut subscription).await;
}

#[tokio::test(flavor = "current_thread")]
async fn repeated_publishes_coalesce_until_a_later_publish() {
    let notifier = ConversationCommitNotifier::new();
    let thread_id = thread("thread-coalesced");
    let mut subscription = notifier
        .subscribe(thread_id.clone())
        .expect("subscription should be accepted");

    for _ in 0..3 {
        assert_eq!(
            notifier.publish(&thread_id),
            ConversationCommitPublish::Notified
        );
    }

    wait_ready(&mut subscription).await;
    assert_pending(&mut subscription).await;

    assert_eq!(
        notifier.publish(&thread_id),
        ConversationCommitPublish::Notified
    );
    wait_ready(&mut subscription).await;
}

#[tokio::test(flavor = "current_thread")]
async fn threads_are_isolated_and_same_thread_subscribers_wake_independently() {
    let notifier = ConversationCommitNotifier::new();
    let thread_a = thread("thread-a");
    let thread_b = thread("thread-b");
    let mut first_a = notifier
        .subscribe(thread_a.clone())
        .expect("first thread-a subscription should be accepted");
    let mut second_a = notifier
        .subscribe(thread_a.clone())
        .expect("second thread-a subscription should be accepted");
    let mut subscription_b = notifier
        .subscribe(thread_b.clone())
        .expect("thread-b subscription should be accepted");

    assert_eq!(
        notifier.publish(&thread_b),
        ConversationCommitPublish::Notified
    );
    wait_ready(&mut subscription_b).await;
    assert_pending(&mut first_a).await;
    assert_pending(&mut second_a).await;

    assert_eq!(
        notifier.publish(&thread_a),
        ConversationCommitPublish::Notified
    );
    wait_ready(&mut first_a).await;
    wait_ready(&mut second_a).await;
    assert_pending(&mut subscription_b).await;
    assert_pending(&mut first_a).await;
    assert_pending(&mut second_a).await;
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_subscriptions_removes_only_the_last_matching_entry() {
    let notifier = ConversationCommitNotifier::new();
    let thread_id = thread("thread-drop");
    let first = notifier
        .subscribe(thread_id.clone())
        .expect("first subscription should be accepted");
    let mut second = notifier
        .subscribe(thread_id.clone())
        .expect("second subscription should be accepted");

    drop(first);
    assert_eq!(
        notifier.publish(&thread_id),
        ConversationCommitPublish::Notified
    );
    wait_ready(&mut second).await;

    drop(second);
    assert_eq!(
        notifier.publish(&thread_id),
        ConversationCommitPublish::Unobserved
    );
}

#[tokio::test(flavor = "current_thread")]
async fn notifier_clones_keep_waits_open_but_final_owner_closes_them() {
    let notifier = ConversationCommitNotifier::new();
    let clone = notifier.clone();
    let thread_id = thread("thread-clone");
    let mut subscription = notifier
        .subscribe(thread_id.clone())
        .expect("subscription should be accepted");

    drop(notifier);
    assert_pending(&mut subscription).await;
    assert_eq!(
        clone.publish(&thread_id),
        ConversationCommitPublish::Notified
    );
    wait_ready(&mut subscription).await;

    drop(clone);
    assert_eq!(
        tokio::time::timeout(READY_TIMEOUT, subscription.wait())
            .await
            .expect("closed receiver should resolve before the deadline"),
        Err(ConversationCommitWaitError::Closed)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_a_borrowed_wait_leaves_the_subscription_usable() {
    let notifier = ConversationCommitNotifier::new();
    let thread_id = thread("thread-cancel");
    let mut subscription = notifier
        .subscribe(thread_id.clone())
        .expect("subscription should be accepted");

    assert!(
        tokio::time::timeout(PENDING_TIMEOUT, subscription.wait())
            .await
            .is_err(),
        "the bounded in-flight wait should be cancelled while pending"
    );

    assert_eq!(
        notifier.publish(&thread_id),
        ConversationCommitPublish::Notified
    );
    wait_ready(&mut subscription).await;
    assert_pending(&mut subscription).await;
}
