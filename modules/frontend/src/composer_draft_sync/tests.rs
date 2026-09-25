use std::cell::Cell;
use std::rc::Rc;

use artisan_domain::{
    ComposerAttachmentDigest, ComposerAttachmentRef, ComposerDraftScope, ImageMimeType, ThreadId,
};

use super::{DraftBody, DraftSave, DraftSync};

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
