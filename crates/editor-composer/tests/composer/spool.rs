use super::*;
use crate::composer::draft_body::DraftBody;
use std::sync::mpsc;

pub(crate) trait Direct {
    fn keep(
        &self,
        host: &HostId,
        scope: &str,
        draft: KeptDraft,
        images: &[Arc<[u8]>],
        holder: Holder,
    ) -> Result<Slot, KeepSpooledDraftError>;
    fn forget(&self, host: &HostId, scope: &str, slot: Slot)
    -> Result<(), ForgetSpooledDraftError>;
    fn kept_beside(&self, host: &HostId, scope: &str) -> Vec<Result<BesideDraft, KeptBesideError>>;
}

impl Direct for DraftSpool {
    fn keep(
        &self,
        host: &HostId,
        scope: &str,
        draft: KeptDraft,
        images: &[Arc<[u8]>],
        holder: Holder,
    ) -> Result<Slot, KeepSpooledDraftError> {
        self.turn(host, scope).wait().keep(draft, images, holder)
    }

    fn forget(
        &self,
        host: &HostId,
        scope: &str,
        slot: Slot,
    ) -> Result<(), ForgetSpooledDraftError> {
        self.turn(host, scope).wait().forget(slot)
    }

    fn kept_beside(&self, host: &HostId, scope: &str) -> Vec<Result<BesideDraft, KeptBesideError>> {
        self.turn(host, scope).wait().kept_beside()
    }
}

pub(crate) enum Rule {
    PauseBefore,
    PauseAfter,
    Refuse,
    RefuseRemoval,
    RefuseSync,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    Rename,
    Removal,
    Sync,
}

struct Armed {
    operation: Operation,
    matches: Box<dyn Fn(&Path) -> bool + Send>,
    rule: Rule,
    reached: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

#[derive(Default)]
pub(crate) struct ScriptedDisk {
    armed: Mutex<Vec<Armed>>,
}

pub(crate) struct Gate {
    reached: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
}

impl Gate {
    pub(crate) fn reached(&self) {
        self.reached
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the spool reaches the gated rename");
    }

    pub(crate) fn release(self) {
        self.release.send(()).unwrap();
    }
}

impl ScriptedDisk {
    pub(crate) fn arm(&self, rule: Rule, matches: impl Fn(&Path) -> bool + Send + 'static) -> Gate {
        let (reached_sender, reached) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel();
        self.armed.lock().unwrap().push(Armed {
            operation: match rule {
                Rule::RefuseRemoval => Operation::Removal,
                Rule::RefuseSync => Operation::Sync,
                Rule::PauseBefore | Rule::PauseAfter | Rule::Refuse => Operation::Rename,
            },
            matches: Box::new(matches),
            rule,
            reached: reached_sender,
            release: release_receiver,
        });
        Gate { reached, release }
    }
}

impl ScriptedDisk {
    fn take(&self, operation: Operation, path: &Path) -> Option<Armed> {
        let mut armed = self.armed.lock().unwrap();
        armed
            .iter()
            .position(|armed| armed.operation == operation && (armed.matches)(path))
            .map(|index| armed.remove(index))
    }
}

impl SpoolDisk for ScriptedDisk {
    fn sync_folder(&self, folder: &Path) -> std::io::Result<()> {
        match self.take(Operation::Sync, folder) {
            Some(_) => Err(std::io::Error::other("flush refused by the test disk")),
            None => LocalDisk::shared().sync_folder(folder),
        }
    }

    fn remove_empty_folder(&self, path: &Path) -> std::io::Result<()> {
        match self.take(Operation::Removal, path) {
            Some(_) => Err(std::io::Error::other("removal refused by the test disk")),
            None => LocalDisk::shared().remove_empty_folder(path),
        }
    }

    fn remove_tree(&self, path: &Path) -> std::io::Result<()> {
        match self.take(Operation::Removal, path) {
            Some(_) => Err(std::io::Error::other("removal refused by the test disk")),
            None => LocalDisk::shared().remove_tree(path),
        }
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        let Some(armed) = self.take(Operation::Rename, to) else {
            return LocalDisk::shared().rename(from, to);
        };
        let pause = |armed: &Armed| {
            armed.reached.send(()).unwrap();
            armed.release.recv().unwrap();
        };
        match armed.rule {
            Rule::PauseBefore => {
                pause(&armed);
                LocalDisk::shared().rename(from, to)
            }
            Rule::PauseAfter => {
                let renamed = LocalDisk::shared().rename(from, to);
                pause(&armed);
                renamed
            }
            Rule::Refuse | Rule::RefuseRemoval | Rule::RefuseSync => {
                Err(std::io::Error::other("refused by the test disk"))
            }
        }
    }
}

pub(crate) fn named(prefix: &'static str) -> impl Fn(&Path) -> bool + Send + 'static {
    move |path: &Path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(prefix))
    }
}

pub(crate) fn unit(drafts: &Path, stem: &str, prefix: &str) -> PathBuf {
    let units: Vec<_> = std::fs::read_dir(drafts.join(stem))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect();
    let [unit] = &units[..] else {
        panic!("expected one {prefix} folder for {stem}: {units:?}");
    };
    unit.clone()
}

fn kept(text: &str) -> KeptDraft {
    KeptDraft {
        body: DraftBody {
            text: text.to_owned(),
            ..DraftBody::default()
        },
        base: None,
        post: None,
        lost: Vec::new(),
        set_aside: Vec::new(),
    }
}

fn images(spooled: &SpooledDraft) -> Vec<Option<&[u8]>> {
    spooled
        .images
        .iter()
        .map(|image| image.as_ref().ok().map(Vec::as_slice))
        .collect()
}

#[test]
fn drafts_whose_host_and_scope_join_alike_are_kept_apart() {
    let directory = tempfile::tempdir().unwrap();
    let spool = DraftSpool::new(directory.path(), LocalDisk::shared());
    let first = HostId("forge-a".into());
    let second = HostId("forge".into());
    spool
        .keep(
            &first,
            "new",
            kept("on the first Forge"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    spool
        .keep(
            &second,
            "a-new",
            kept("on the second Forge"),
            &[Arc::from(&b"image"[..])],
            Holder::Owner(Slot::Record),
        )
        .unwrap();

    let taken = spool.take(&first, "new").unwrap().unwrap();
    assert_eq!(taken.draft, kept("on the first Forge"));
    assert!(taken.images.is_empty(), "the first draft has no images");
    spool.forget(&first, "new", Slot::Record).unwrap();
    let taken = spool.take(&second, "a-new").unwrap().unwrap();
    assert_eq!(
        (taken.draft.body.text.as_str(), images(&taken)),
        ("on the second Forge", vec![Some(&b"image"[..])]),
        "forgetting one draft leaves the other's text and images"
    );
}

#[test]
fn a_missing_or_damaged_image_still_brings_back_the_drafts_text() {
    let directory = tempfile::tempdir().unwrap();
    let spool = DraftSpool::new(directory.path(), LocalDisk::shared());
    let host = HostId("forge".into());
    spool
        .keep(
            &host,
            "new",
            kept("text beside two images"),
            &[Arc::from(&b"first"[..]), Arc::from(&b"second"[..])],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let mut stored: Vec<_> = std::fs::read_dir(unit(
        &directory.path().join("drafts"),
        "forge-new",
        "record.",
    ))
    .unwrap()
    .map(|entry| entry.unwrap().path())
    .filter(|path| {
        path.extension()
            .is_some_and(|extension| extension == "image")
    })
    .collect();
    stored.sort();
    std::fs::remove_file(&stored[0]).unwrap();
    std::fs::write(&stored[1], b"damaged").unwrap();

    let taken = spool.take(&host, "new").unwrap().unwrap();
    assert_eq!(taken.draft, kept("text beside two images"));
    assert!(
        taken
            .images
            .iter()
            .any(|image| matches!(image, Err(TakeSpooledImageError::Read { .. })))
            && taken
                .images
                .iter()
                .any(|image| matches!(image, Err(TakeSpooledImageError::Damaged { .. }))),
        "each image says what happened to it: {:?}",
        taken.images
    );
}

#[test]
fn an_unreadable_kept_draft_is_moved_aside_instead_of_being_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let spool = DraftSpool::new(directory.path(), LocalDisk::shared());
    let host = HostId("forge".into());
    spool
        .keep(
            &host,
            "new",
            kept("an older draft"),
            &[Arc::from(&b"image"[..])],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let record = unit(&directory.path().join("drafts"), "forge-new", "record.").join("record.json");
    std::fs::write(&record, b"{ not json").unwrap();

    let Err(TakeSpooledDraftError::SetAside { set_aside, .. }) = spool.take(&host, "new") else {
        panic!("expected the unreadable draft to be moved aside");
    };
    spool
        .keep(
            &host,
            "new",
            kept("a newer draft"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    assert_eq!(
        std::fs::read(&set_aside).unwrap(),
        b"{ not json",
        "keeping a newer draft leaves the unreadable one untouched"
    );
    let set_aside_images = std::fs::read_dir(set_aside.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| std::fs::read(path).unwrap() == b"image")
        .count();
    assert_eq!(set_aside_images, 1, "its image is kept beside it");
    assert_eq!(
        spool.take(&host, "new").unwrap().unwrap().draft,
        kept("a newer draft")
    );
}

#[test]
fn an_unreadable_kept_draft_that_cannot_be_moved_aside_is_reported_where_it_still_is() {
    let directory = tempfile::tempdir().unwrap();
    let disk = Arc::new(ScriptedDisk::default());
    let spool = DraftSpool::new(directory.path(), Arc::<ScriptedDisk>::clone(&disk));
    let host = HostId("forge".into());
    spool
        .keep(
            &host,
            "new",
            kept("an older draft"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let record = unit(&directory.path().join("drafts"), "forge-new", "record.").join("record.json");
    std::fs::write(&record, b"{ not json").unwrap();
    drop(disk.arm(Rule::Refuse, named("unreadable-")));

    let Err(TakeSpooledDraftError::Left { path, .. }) = spool.take(&host, "new") else {
        panic!("expected the unreadable draft to stay where it is");
    };
    assert_eq!(path, record, "the notice names the record's real place");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"{ not json",
        "the unreadable draft is still there for the user"
    );
}

#[test]
fn a_draft_that_did_not_recover_the_record_is_kept_beside_it_instead_of_over_it() {
    let directory = tempfile::tempdir().unwrap();
    let spool = DraftSpool::new(directory.path(), LocalDisk::shared());
    let host = HostId("forge".into());
    spool
        .keep(
            &host,
            "new",
            kept("the newer draft"),
            &[Arc::from(&b"newer image"[..])],
            Holder::Owner(Slot::Record),
        )
        .unwrap();

    let Slot::Beside(slot) = spool
        .keep(
            &host,
            "new",
            kept("an older draft"),
            &[Arc::from(&b"older image"[..])],
            Holder::Newcomer,
        )
        .unwrap()
    else {
        panic!("expected the draft to be kept beside the record");
    };

    let recovered = spool.take(&host, "new").unwrap().unwrap();
    assert_eq!(recovered.draft, kept("the newer draft"));
    assert!(matches!(&recovered.images[..], [Ok(bytes)] if bytes == b"newer image"));
    let beside = spool.kept_beside(&host, "new");
    let [Ok(beside)] = &beside[..] else {
        panic!("expected one draft kept beside the record: {beside:?}");
    };
    assert_eq!(beside.slot, slot);
    assert_eq!(beside.spooled.draft, kept("an older draft"));
    assert!(
        matches!(&beside.spooled.images[..], [Ok(bytes)] if bytes == b"older image"),
        "its image is kept beside it"
    );
}

#[test]
fn a_draft_kept_beside_again_rewrites_its_own_slot_and_drops_images_it_no_longer_holds() {
    let directory = tempfile::tempdir().unwrap();
    let spool = DraftSpool::new(directory.path(), LocalDisk::shared());
    let host = HostId("forge".into());
    spool
        .keep(
            &host,
            "new",
            kept("the record"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let slot = spool
        .keep(
            &host,
            "new",
            kept("first keep"),
            &[Arc::from(&b"first image"[..])],
            Holder::Newcomer,
        )
        .unwrap();

    assert_eq!(
        spool
            .keep(
                &host,
                "new",
                kept("second keep"),
                &[Arc::from(&b"second image"[..])],
                Holder::Owner(slot),
            )
            .unwrap(),
        slot
    );

    let beside = spool.kept_beside(&host, "new");
    let [Ok(beside)] = &beside[..] else {
        panic!("expected one draft kept beside the record: {beside:?}");
    };
    assert_eq!(beside.spooled.draft, kept("second keep"));
    assert!(matches!(&beside.spooled.images[..], [Ok(bytes)] if bytes == b"second image"));
    let folder = unit(&directory.path().join("drafts"), "forge-new", "kept-1.");
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);

    spool.forget(&host, "new", slot).unwrap();
    assert!(!folder.exists());
    assert!(spool.kept_beside(&host, "new").is_empty());
}

#[test]
fn a_draft_kept_where_a_discarded_one_is_still_being_removed_keeps_its_text_and_images() {
    let directory = tempfile::tempdir().unwrap();
    let disk = Arc::new(ScriptedDisk::default());
    let spool = DraftSpool::new(directory.path(), Arc::<ScriptedDisk>::clone(&disk));
    let host = HostId("forge".into());
    spool
        .keep(
            &host,
            "new",
            kept("the discarded draft"),
            &[Arc::from(&b"discarded image"[..])],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let gate = disk.arm(Rule::PauseAfter, named("rejected-"));
    let forgetting = std::thread::spawn({
        let spool = spool.clone();
        let host = host.clone();
        move || spool.forget(&host, "new", Slot::Record)
    });
    gate.reached();

    let replacing = std::thread::spawn({
        let spool = spool.clone();
        let host = host.clone();
        move || {
            spool.keep(
                &host,
                "new",
                kept("the replacement"),
                &[Arc::from(&b"replacement image"[..])],
                Holder::Newcomer,
            )
        }
    });
    gate.release();
    forgetting.join().unwrap().unwrap();
    assert_eq!(replacing.join().unwrap().unwrap(), Slot::Record);

    let taken = spool
        .take(&host, "new")
        .unwrap()
        .expect("the replacement is kept");
    assert_eq!(
        (&taken.draft, images(&taken)),
        (
            &kept("the replacement"),
            vec![Some(&b"replacement image"[..])]
        ),
        "removing the discarded draft never touches the replacement's image"
    );
}

#[test]
fn two_drafts_discarded_at_once_are_both_removed() {
    let directory = tempfile::tempdir().unwrap();
    let disk = Arc::new(ScriptedDisk::default());
    let spool = DraftSpool::new(directory.path(), Arc::<ScriptedDisk>::clone(&disk));
    let host = HostId("forge".into());
    for text in ["the record", "first beside", "second beside"] {
        spool
            .keep(&host, "new", kept(text), &[], Holder::Newcomer)
            .unwrap();
    }
    let gate = disk.arm(Rule::PauseAfter, named("rejected-"));
    let first = std::thread::spawn({
        let spool = spool.clone();
        let host = host.clone();
        move || spool.forget(&host, "new", Slot::Beside(BesideSlot(1)))
    });
    gate.reached();
    let second = std::thread::spawn({
        let spool = spool.clone();
        let host = host.clone();
        move || spool.forget(&host, "new", Slot::Beside(BesideSlot(2)))
    });
    gate.release();

    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
    assert!(spool.kept_beside(&host, "new").is_empty());
    assert_eq!(
        spool.take(&host, "new").unwrap().map(|taken| taken.draft),
        Some(kept("the record"))
    );
}

#[test]
fn a_replaced_version_the_disk_refuses_to_remove_stays_and_the_newer_one_is_taken() {
    let directory = tempfile::tempdir().unwrap();
    let disk = Arc::new(ScriptedDisk::default());
    let spool = DraftSpool::new(directory.path(), Arc::<ScriptedDisk>::clone(&disk));
    let host = HostId("forge".into());
    spool
        .keep(
            &host,
            "new",
            kept("first"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    drop(disk.arm(Rule::RefuseRemoval, named("record.1")));
    spool
        .keep(
            &host,
            "new",
            kept("second"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();

    let drafts = directory.path().join("drafts");
    assert!(
        drafts.join("forge-new").join("record.1").is_dir(),
        "the replaced version is removed through the spool's disk"
    );
    assert_eq!(
        spool.take(&host, "new").unwrap().map(|taken| taken.draft),
        Some(kept("second"))
    );
}

#[test]
fn an_emptied_draft_folder_is_removed_through_the_spools_disk() {
    let directory = tempfile::tempdir().unwrap();
    let disk = Arc::new(ScriptedDisk::default());
    let spool = DraftSpool::new(directory.path(), Arc::<ScriptedDisk>::clone(&disk));
    let host = HostId("forge".into());
    spool
        .keep(&host, "new", kept("kept"), &[], Holder::Owner(Slot::Record))
        .unwrap();
    drop(disk.arm(Rule::RefuseRemoval, named("forge-new")));

    spool.forget(&host, "new", Slot::Record).unwrap();
    assert!(
        directory.path().join("drafts").join("forge-new").is_dir(),
        "the disk refused to remove the empty folder, so it stays"
    );
    assert!(spool.take(&host, "new").unwrap().is_none());
}

#[test]
fn a_drafts_queue_is_let_go_once_no_turn_waits_on_it() {
    let directory = tempfile::tempdir().unwrap();
    let spool = DraftSpool::new(directory.path(), LocalDisk::shared());
    let host = HostId("forge".into());
    let first = spool.turn(&host, "new");
    let second = spool.turn(&host, "new");
    drop(first.wait());
    assert_eq!(spool.owner.queues().len(), 1, "the second turn still waits");
    drop(second);
    assert!(
        spool.owner.queues().is_empty(),
        "no turn is left, so the queue is gone"
    );
    spool
        .keep(&host, "new", kept("kept"), &[], Holder::Owner(Slot::Record))
        .unwrap();
    assert!(spool.owner.queues().is_empty());
    assert_eq!(
        spool.take(&host, "new").unwrap().map(|taken| taken.draft),
        Some(kept("kept")),
        "a later turn starts a new queue"
    );
}

#[test]
fn a_second_editor_never_reads_or_removes_the_drafts_another_editor_keeps() {
    let directory = tempfile::tempdir().unwrap();
    let host = HostId("forge".into());
    let first = DraftSpool::new(directory.path(), LocalDisk::shared());
    first
        .keep(
            &host,
            "new",
            kept("typed offline in the first window"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    drop(first);
    let other_editor = std::fs::File::open(directory.path().join("drafts.lock")).unwrap();
    other_editor.lock().unwrap();

    let second = DraftSpool::new(directory.path(), LocalDisk::shared());
    assert!(
        second.take(&host, "new").unwrap().is_none(),
        "the second window doesn't take over the first window's draft"
    );
    assert!(matches!(
        second.keep(
            &host,
            "new",
            kept("an older copy saved from the second window"),
            &[],
            Holder::Owner(Slot::Record),
        ),
        Err(KeepSpooledDraftError::OwnedElsewhere { .. })
    ));
    second.forget(&host, "new", Slot::Record).unwrap();
    drop(other_editor);

    assert_eq!(
        second.take(&host, "new").unwrap().unwrap().draft,
        kept("typed offline in the first window"),
        "once the first window is gone, the second takes over its draft unharmed"
    );
}

#[test]
fn keeping_again_after_the_drafts_folder_failed_to_flush_flushes_it_again() {
    let directory = tempfile::tempdir().unwrap();
    let disk = Arc::new(ScriptedDisk::default());
    let spool = DraftSpool::new(directory.path(), Arc::<ScriptedDisk>::clone(&disk));
    let host = HostId("forge".into());
    let drafts = directory.path().join("drafts");
    let keep = || {
        spool.keep(
            &host,
            "new",
            kept("typed offline"),
            &[],
            Holder::Owner(Slot::Record),
        )
    };
    let refuse_drafts_flush = || {
        let drafts = drafts.clone();
        drop(disk.arm(Rule::RefuseSync, move |path| path == drafts));
    };

    refuse_drafts_flush();
    assert!(
        matches!(keep(), Err(KeepSpooledDraftError::Sync { ref path, .. }) if *path == drafts),
        "a failed flush of the drafts folder is reported as that flush"
    );
    refuse_drafts_flush();
    assert!(
        matches!(keep(), Err(KeepSpooledDraftError::Sync { .. })),
        "keeping again flushes the drafts folder again instead of trusting it is on disk"
    );
    keep().unwrap();
    assert_eq!(
        spool.take(&host, "new").unwrap().unwrap().draft,
        kept("typed offline")
    );
}
