use super::*;
use std::sync::mpsc;

#[test]
fn weak_epochs_keep_evicted_live_leaf_until_explicit_release() {
    let mut epochs = Epochs::default();
    let cached = epochs.issue("a");
    let mounted = cached.clone();
    drop(cached);
    epochs.prune();
    assert!(mounted.load(Ordering::Relaxed));
    assert!(epochs.entries.contains_key("a"));
    epochs.release("a");
    assert!(!mounted.load(Ordering::Relaxed));
    let first = epochs.issue("a");
    let replacement = epochs.issue("a");
    assert!(!first.load(Ordering::Relaxed));
    assert!(replacement.load(Ordering::Relaxed));
    let other = epochs.issue("b");
    epochs.clear();
    assert!(!replacement.load(Ordering::Relaxed));
    assert!(!other.load(Ordering::Relaxed));
    let unused = epochs.issue("unused");
    drop(unused);
    epochs.prune();
    assert!(epochs.entries.is_empty());
}

#[test]
fn retained_raw_permit_follows_last_lease_and_wakes_capacity_waiters() {
    let pool = Arc::new(Semaphore::new(9));
    let permit = pool.clone().try_acquire_many_owned(7).unwrap();
    let (wake, received) = mpsc::channel();
    let lease = Arc::new(Lease {
        permit: Some(permit),
        wake: Arc::new(move || {
            let _ = wake.send(());
        }),
    });
    let selection = lease.clone();
    drop(lease);
    assert_eq!(pool.available_permits(), 2);
    assert!(received.try_recv().is_err());
    drop(selection);
    assert_eq!(pool.available_permits(), 9);
    received.recv().unwrap();
}

#[test]
fn literal_selection_copies_exact_raw_text_while_display_removes_one_newline() {
    let text: Arc<str> = Arc::from("**literal** &amp; 界🙂\n\n");
    let source = selection_source(text.clone(), SelectionSourceId(7), vec![4, 2], "\n\n");
    assert_eq!(visible_text(&text), "**literal** &amp; 界🙂\n");
    assert_eq!(source.text.as_ref(), text.as_ref());
    let range = 0..visible_text(&text).len();
    let slice = aqua_text::document::SourceSlice {
        source: source.id,
        range: range.clone(),
    };
    assert_eq!(&source.text[range], "**literal** &amp; 界🙂\n");
    assert_eq!(slice.position(slice.range.end).offset, slice.range.end);
    assert_eq!(source.order, [4, 2]);
    assert_eq!(source.separator.as_ref(), "\n\n");
    assert!(source.copies.is_empty());
    assert_eq!(visible_text("no newline"), "no newline");
}
