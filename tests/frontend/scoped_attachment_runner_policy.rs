//! Focused dependency-free coverage for scoped attachment runner policy.

#[allow(dead_code)]
#[path = "../../modules/frontend/src/scoped_attachment_runner_policy.rs"]
mod scoped_attachment_runner_policy;

use scoped_attachment_runner_policy::{RunnerCommand, RunnerEffect, ScopedAttachmentRunnerPolicy};

#[test]
fn attachments_receive_unique_monotonic_keys_and_queue_their_runs() {
    let mut policy = ScopedAttachmentRunnerPolicy::new();

    let first = policy.attach("first");
    let second = policy.attach("second");
    let third = policy.attach("third");

    assert_eq!(
        (first.as_str(), second.as_str(), third.as_str()),
        ("attachment:0", "attachment:1", "attachment:2")
    );
    assert_eq!(
        policy.pending_keys().collect::<Vec<_>>(),
        ["attachment:0", "attachment:1", "attachment:2",]
    );
    assert_eq!(
        policy.next_effect(),
        RunnerEffect::Start {
            key: first,
            input: "first",
        }
    );
    assert_eq!(
        policy.next_effect(),
        RunnerEffect::Start {
            key: second,
            input: "second",
        }
    );
    assert_eq!(
        policy.next_effect(),
        RunnerEffect::Start {
            key: third,
            input: "third",
        }
    );
    assert_eq!(policy.next_effect(), RunnerEffect::<&str>::NoOp);
}

#[test]
fn same_key_replacements_coalesce_to_the_latest_input() {
    let mut policy = ScopedAttachmentRunnerPolicy::new();

    policy.replace("attachment:7", "first");
    policy.replace("attachment:7", "second");
    policy.replace("attachment:7", "latest");

    assert_eq!(policy.pending_len(), 1);
    assert_eq!(policy.mailbox_len(), 1);
    assert_eq!(
        policy.latest_command("attachment:7"),
        Some(&RunnerCommand::run("attachment:7", "latest"))
    );
    assert_eq!(
        policy.next_effect(),
        RunnerEffect::Start {
            key: "attachment:7".to_owned(),
            input: "latest",
        }
    );
    assert_eq!(policy.pending_len(), 0);
    assert_eq!(policy.mailbox_len(), 0);
}

#[test]
fn different_keys_keep_independent_fifo_fairness_when_one_is_replaced() {
    let mut policy = ScopedAttachmentRunnerPolicy::new();

    policy.replace("a", 1);
    policy.replace("b", 2);
    policy.replace("a", 10);

    assert_eq!(policy.pending_keys().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(
        policy.next_effect(),
        RunnerEffect::Start {
            key: "a".to_owned(),
            input: 10,
        }
    );
    assert_eq!(
        policy.next_effect(),
        RunnerEffect::Start {
            key: "b".to_owned(),
            input: 2,
        }
    );
}

#[test]
fn replacement_during_interruption_supersedes_the_taken_command() {
    let mut policy = ScopedAttachmentRunnerPolicy::new();
    let key = policy.attach("initial");

    assert_eq!(
        policy.next_effect(),
        RunnerEffect::Start {
            key: key.clone(),
            input: "initial",
        }
    );

    policy.replace(&key, "taken");
    assert_eq!(
        policy.next_effect(),
        RunnerEffect::Interrupt { key: key.clone() }
    );
    assert!(!policy.is_active(&key));
    assert!(policy.is_interrupting());

    // This ingress occurs at the external interruption boundary. It must
    // replace the command that was already taken, with no second FIFO entry.
    policy.replace(&key, "replacement");
    assert_eq!(policy.pending_len(), 1);
    assert_eq!(
        policy.complete_interruption(),
        RunnerEffect::Start {
            key: key.clone(),
            input: "replacement",
        }
    );
    assert!(policy.is_active(&key));
    assert!(!policy.is_interrupting());
    assert_eq!(policy.pending_len(), 0);
    assert_eq!(policy.mailbox_len(), 0);
}

#[test]
fn release_before_start_is_a_noop_and_never_starts_queued_work() {
    let mut policy = ScopedAttachmentRunnerPolicy::new();
    let key = policy.attach("queued");

    policy.release(&key);

    assert_eq!(policy.pending_len(), 1);
    assert_eq!(
        policy.latest_command(&key),
        Some(&RunnerCommand::<&str>::release(key.clone()))
    );
    assert_eq!(policy.next_effect(), RunnerEffect::<&str>::NoOp);
    assert!(!policy.is_active(&key));
    assert_eq!(policy.next_effect(), RunnerEffect::<&str>::NoOp);
}

#[test]
fn active_release_interrupts_before_returning_noop_and_preserves_order() {
    let mut policy = ScopedAttachmentRunnerPolicy::new();
    let key = policy.attach("running");

    assert!(matches!(policy.next_effect(), RunnerEffect::Start { .. }));
    policy.release(&key);

    assert_eq!(
        policy.next_effect(),
        RunnerEffect::Interrupt { key: key.clone() }
    );
    // Removal happens before the interrupt effect is exposed.
    assert!(!policy.is_active(&key));
    assert_eq!(policy.next_effect(), RunnerEffect::<&str>::NoOp);
    assert!(!policy.is_active(&key));
    assert_eq!(policy.active_len(), 0);
}

#[test]
fn a_release_arriving_during_interruption_supersedes_a_run_without_starting() {
    let mut policy = ScopedAttachmentRunnerPolicy::new();
    let key = policy.attach("initial");

    assert!(matches!(policy.next_effect(), RunnerEffect::Start { .. }));
    policy.replace(&key, "next");
    assert_eq!(
        policy.next_effect(),
        RunnerEffect::Interrupt { key: key.clone() }
    );

    policy.release(&key);

    assert_eq!(policy.next_effect(), RunnerEffect::<&str>::NoOp);
    assert!(!policy.is_active(&key));
    assert_eq!(policy.pending_len(), 0);
    assert_eq!(policy.mailbox_len(), 0);
}
