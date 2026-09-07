//! Dependency-free coverage for the scoped attachment command queue.

#[path = "../../modules/frontend/src/scoped_attachment_queue.rs"]
mod scoped_attachment_queue;

use scoped_attachment_queue::{AttachmentCommand, ScopedAttachmentQueue};

#[test]
fn attach_allocates_unique_deterministic_keys_and_queues_runs() {
    let mut queue = ScopedAttachmentQueue::new();

    let first = queue.attach("first");
    let second = queue.attach("second");

    assert_eq!(
        (first.as_str(), second.as_str()),
        ("attachment:0", "attachment:1")
    );
    assert_eq!(queue.pending_len(), 2);
    assert_eq!(queue.mailbox_len(), 2);
    assert_eq!(
        queue.pending_keys().collect::<Vec<_>>(),
        ["attachment:0", "attachment:1"]
    );
    assert_eq!(
        queue.take_next(),
        Some(AttachmentCommand::run("attachment:0", "first"))
    );
    assert_eq!(
        queue.take_next(),
        Some(AttachmentCommand::run("attachment:1", "second"))
    );
}

#[test]
fn repeated_replacements_keep_one_pending_key_and_only_the_latest_input() {
    let mut queue = ScopedAttachmentQueue::new();

    queue.replace("attachment:7", "first");
    queue.replace("attachment:7", "second");
    queue.replace("attachment:7", "latest");

    assert_eq!(queue.pending_len(), 1);
    assert_eq!(queue.mailbox_len(), 1);
    assert!(queue.contains_pending_key("attachment:7"));
    assert_eq!(
        queue.latest_command("attachment:7"),
        Some(&AttachmentCommand::run("attachment:7", "latest"))
    );
    assert_eq!(
        queue.take_next(),
        Some(AttachmentCommand::run("attachment:7", "latest"))
    );
    assert!(queue.is_empty());
    assert_eq!(queue.mailbox_len(), 0);
}

#[test]
fn different_keys_keep_distinct_fifo_order_while_a_key_is_replaced() {
    let mut queue = ScopedAttachmentQueue::new();

    queue.replace("a", 1);
    queue.replace("b", 2);
    queue.replace("a", 10);

    assert_eq!(queue.pending_keys().collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!(queue.take_next(), Some(AttachmentCommand::run("a", 10)));
    assert_eq!(queue.take_next(), Some(AttachmentCommand::run("b", 2)));
}

#[test]
fn run_release_run_supersession_keeps_only_the_final_run() {
    let mut queue = ScopedAttachmentQueue::new();

    queue.replace("key", "initial");
    queue.release("key");
    assert_eq!(
        queue.latest_command("key"),
        Some(&AttachmentCommand::<&str>::release("key"))
    );

    queue.replace("key", "final");

    assert_eq!(queue.pending_len(), 1);
    assert_eq!(queue.pending_keys().collect::<Vec<_>>(), ["key"]);
    assert_eq!(
        queue.take_next(),
        Some(AttachmentCommand::run("key", "final"))
    );
}

#[test]
fn release_supersedes_a_queued_run_when_it_is_taken() {
    let mut queue = ScopedAttachmentQueue::new();

    queue.replace("key", "initial");
    queue.release("key");

    assert_eq!(
        queue.take_next(),
        Some(AttachmentCommand::<&str>::release("key"))
    );
}

#[test]
fn a_key_can_be_reused_after_its_command_is_taken() {
    let mut queue = ScopedAttachmentQueue::new();

    queue.replace("key", "before");
    assert_eq!(
        queue.take_next(),
        Some(AttachmentCommand::run("key", "before"))
    );
    assert!(queue.is_empty());

    queue.replace("key", "after");

    assert_eq!(queue.pending_len(), 1);
    assert_eq!(
        queue.take_next(),
        Some(AttachmentCommand::run("key", "after"))
    );
    assert!(queue.is_empty());
}

#[test]
fn taking_an_empty_queue_returns_none_without_changing_state() {
    let mut queue = ScopedAttachmentQueue::<String>::new();

    assert!(queue.is_empty());
    assert_eq!(queue.pending_len(), 0);
    assert_eq!(queue.mailbox_len(), 0);
    assert_eq!(queue.take_next(), None);
    assert!(queue.is_empty());
    assert_eq!(queue.pending_keys().count(), 0);
}
