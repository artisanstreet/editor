use std::cell::Cell;
use std::rc::Rc;

use artisan_domain::{
    ComposerAttachmentDigest, ComposerAttachmentRef, ComposerDraftRevision, ComposerDraftScope,
    ImageMimeType, ThreadId,
};

use super::{DraftBody, DraftSave, DraftSync, SubmitReadiness};

fn revision(value: u64) -> ComposerDraftRevision {
    ComposerDraftRevision::new(value).unwrap()
}

/// A hold that counts how many are alive, like a connection hold.
struct CountedHold(Rc<Cell<usize>>);

impl Drop for CountedHold {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

fn acquire(live: &Rc<Cell<usize>>) -> impl FnOnce() -> Option<CountedHold> + '_ {
    move || {
        live.set(live.get() + 1);
        Some(CountedHold(Rc::clone(live)))
    }
}

fn scope(name: &str) -> ComposerDraftScope {
    ComposerDraftScope::Thread(ThreadId::parse(name).unwrap())
}

fn body(text: &str) -> DraftBody {
    DraftBody {
        text: text.to_owned(),
        attachments: Vec::new(),
    }
}

fn save(scope: &ComposerDraftScope, sequence: u64, text: &str) -> DraftSave {
    DraftSave {
        scope: scope.clone(),
        sequence,
        body: body(text),
    }
}

#[test]
fn keystrokes_coalesce_behind_the_one_save_in_flight() {
    let live = Rc::new(Cell::new(0));
    let thread = scope("thread-a");
    let mut sync = DraftSync::default();
    assert_eq!(
        sync.edit(&thread, body("h"), acquire(&live)),
        Some(save(&thread, 1, "h"))
    );
    assert_eq!(live.get(), 1, "an unsaved change holds the connection");
    for text in ["he", "hel", "hell", "hello"] {
        assert_eq!(sync.edit(&thread, body(text), acquire(&live)), None);
    }
    assert_eq!(live.get(), 1, "one hold per scope, however many keystrokes");
    // Only the newest unsent body follows the acknowledged save.
    assert_eq!(sync.settled(&thread, 1), Some(save(&thread, 2, "hello")));
    assert_eq!(live.get(), 1);
    assert_eq!(sync.settled(&thread, 2), None);
    assert!(sync.is_settled(&thread));
    assert_eq!(live.get(), 0);
}

#[test]
fn the_hold_drops_only_when_the_latest_sent_save_is_acknowledged() {
    let live = Rc::new(Cell::new(0));
    let thread = scope("thread-b");
    let mut sync = DraftSync::default();
    assert!(sync.edit(&thread, body("one"), acquire(&live)).is_some());
    assert_eq!(sync.edit(&thread, body("two"), acquire(&live)), None);
    // A stale or foreign acknowledgement settles nothing.
    assert_eq!(sync.settled(&thread, 7), None);
    assert_eq!(sync.settled(&scope("other"), 1), None);
    assert_eq!(live.get(), 1);
    assert_eq!(sync.settled(&thread, 1), Some(save(&thread, 2, "two")));
    assert_eq!(sync.settled(&thread, 1), None, "an ack is consumed once");
    assert_eq!(live.get(), 1, "save 2 is still unacknowledged");
    assert_eq!(sync.settled(&thread, 2), None);
    assert_eq!(live.get(), 0);
}

#[test]
fn failures_release_the_chain_and_uploads_hold_it() {
    let live = Rc::new(Cell::new(0));
    let thread = scope("thread-d");
    let other = scope("thread-e");
    let mut sync = DraftSync::default();
    sync.begin_upload(&thread, acquire(&live));
    assert_eq!(live.get(), 1, "a pending upload holds the connection");
    let sent = sync.edit(&thread, body("caption"), acquire(&live)).unwrap();
    assert_eq!(sync.settled(&thread, sent.sequence), None);
    assert_eq!(live.get(), 1, "the upload still holds");
    sync.finish_upload(&thread);
    assert_eq!(live.get(), 0);

    let reference = ComposerAttachmentRef::new(
        ComposerAttachmentDigest::new([1; 32]),
        ImageMimeType::Png,
        "a.png",
        3,
    )
    .unwrap();
    let with_image = DraftBody {
        text: String::new(),
        attachments: vec![reference],
    };
    assert!(sync.edit(&other, with_image, acquire(&live)).is_some());
    assert_eq!(sync.edit(&other, body("later"), acquire(&live)), None);
    let flushed = sync.flush();
    assert_eq!(flushed.len(), 1);
    assert_eq!(flushed[0].0, save(&other, 2, "later"));
    assert!(
        flushed[0].1.is_some(),
        "a flushed save is admitted under the scope's hold"
    );
    sync.release_all();
    assert_eq!(live.get(), 0);
}

#[test]
fn a_send_names_the_revision_of_the_save_that_carries_its_body() {
    let live = Rc::new(Cell::new(0));
    let thread = scope("thread-send");
    let mut sync = DraftSync::default();
    // "hel" is in flight and "hello" waits behind it when Send is pressed.
    sync.edit(&thread, body("hel"), acquire(&live));
    assert_eq!(sync.edit(&thread, body("hello"), acquire(&live)), None);
    assert_eq!(sync.begin_submit(&thread, body("hello")), None);
    assert_eq!(sync.submit_readiness(&thread), SubmitReadiness::Waiting);
    // Typed after Send: it must not replace the body being sent.
    assert_eq!(sync.edit(&thread, body("next"), acquire(&live)), None);

    // The in-flight save lands; the body being sent goes next.
    assert_eq!(
        sync.acknowledged(&thread, 1, revision(4)),
        Some(save(&thread, 2, "hello"))
    );
    assert_eq!(sync.submit_readiness(&thread), SubmitReadiness::Waiting);
    // Its revision is the one the send names; "next" waits for the send.
    assert_eq!(sync.acknowledged(&thread, 2, revision(5)), None);
    assert_eq!(
        sync.submit_readiness(&thread),
        SubmitReadiness::Ready(revision(5))
    );
    assert_eq!(sync.end_submit(&thread), Some(save(&thread, 3, "next")));
    assert!(!sync.is_submitting(&thread));
}

#[test]
fn a_stored_body_is_sent_at_once_and_its_revision_outlives_the_connection() {
    let live = Rc::new(Cell::new(0));
    let thread = scope("thread-stored");
    let mut sync = DraftSync::default();
    sync.edit(&thread, body("hello"), acquire(&live));
    assert_eq!(sync.acknowledged(&thread, 1, revision(3)), None);
    sync.release_all();

    // A send repeated after its answer was lost names the same revision.
    for _ in 0..2 {
        assert_eq!(sync.begin_submit(&thread, body("hello")), None);
        assert_eq!(
            sync.submit_readiness(&thread),
            SubmitReadiness::Ready(revision(3))
        );
        assert_eq!(sync.end_submit(&thread), None);
    }
    // The emptied draft's revision only moves the known revision forward.
    sync.observe_revision(&thread, revision(4));
    sync.observe_revision(&thread, revision(2));
    sync.begin_submit(&thread, body(""));
    assert_eq!(
        sync.submit_readiness(&thread),
        SubmitReadiness::Ready(revision(4))
    );
}

#[test]
fn a_body_the_forge_never_stored_is_saved_before_it_is_sent() {
    let live = Rc::new(Cell::new(0));
    let thread = scope("thread-unsaved");
    let mut sync = DraftSync::<CountedHold>::default();
    assert_eq!(
        sync.begin_submit(&thread, body("seeded")),
        Some(save(&thread, 1, "seeded"))
    );
    assert_eq!(sync.submit_readiness(&thread), SubmitReadiness::Waiting);
    // The save fails: the send cannot name a revision.
    assert_eq!(sync.settled(&thread, 1), None);
    assert_eq!(sync.submit_readiness(&thread), SubmitReadiness::Failed);
    // Abandoning the send releases anything held behind it.
    sync.edit(&thread, body("typed"), acquire(&live));
    assert_eq!(sync.end_submit(&thread), Some(save(&thread, 2, "typed")));
}
