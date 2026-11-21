use super::*;
use crate::composer::draft_body::DraftBody;

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
    let spool = DraftSpool::new(directory.path());
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
            &[std::sync::Arc::from(&b"image"[..])],
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
    let spool = DraftSpool::new(directory.path());
    let host = HostId("forge".into());
    spool
        .keep(
            &host,
            "new",
            kept("text beside two images"),
            &[
                std::sync::Arc::from(&b"first"[..]),
                std::sync::Arc::from(&b"second"[..]),
            ],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let mut stored: Vec<_> = std::fs::read_dir(directory.path().join("drafts"))
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
    let spool = DraftSpool::new(directory.path());
    let host = HostId("forge".into());
    spool
        .keep(
            &host,
            "new",
            kept("an older draft"),
            &[std::sync::Arc::from(&b"image"[..])],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let record = directory.path().join("drafts").join("forge-new.json");
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

#[cfg(unix)]
#[test]
fn an_unreadable_kept_draft_that_cannot_be_moved_aside_is_reported_where_it_still_is() {
    let directory = tempfile::tempdir().unwrap();
    let spool = DraftSpool::new(directory.path());
    let host = HostId("forge".into());
    let scope = "s".repeat(239);
    spool
        .keep(
            &host,
            &scope,
            kept("an older draft"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let record = directory
        .path()
        .join("drafts")
        .join(format!("forge-{scope}.json"));
    std::fs::write(&record, b"{ not json").unwrap();

    let Err(TakeSpooledDraftError::Left { path, .. }) = spool.take(&host, &scope) else {
        panic!("expected the unreadable draft to stay where it is");
    };
    assert_eq!(path, record, "the notice names the record's real place");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"{ not json",
        "the unreadable draft is still there for the user"
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_kept_draft_is_moved_aside_with_images_whose_names_are_long() {
    let directory = tempfile::tempdir().unwrap();
    let spool = DraftSpool::new(directory.path());
    let host = HostId("forge".into());
    let scope = "s".repeat(174);
    spool
        .keep(
            &host,
            &scope,
            kept("an older draft"),
            &[std::sync::Arc::from(&b"image"[..])],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let record = directory
        .path()
        .join("drafts")
        .join(format!("forge-{scope}.json"));
    std::fs::write(&record, b"{ not json").unwrap();

    let Err(TakeSpooledDraftError::SetAside { set_aside, .. }) = spool.take(&host, &scope) else {
        panic!("expected the unreadable draft to be moved aside");
    };
    assert_eq!(
        std::fs::read(&set_aside).unwrap(),
        b"{ not json",
        "the notice names where the unreadable draft went"
    );
    assert!(!record.exists(), "nothing is left at the old place");
}

#[test]
fn a_draft_that_did_not_recover_the_record_is_kept_beside_it_instead_of_over_it() {
    let directory = tempfile::tempdir().unwrap();
    let spool = DraftSpool::new(directory.path());
    let host = HostId("forge".into());
    spool
        .keep(
            &host,
            "new",
            kept("the newer draft"),
            &[std::sync::Arc::from(&b"newer image"[..])],
            Holder::Owner(Slot::Record),
        )
        .unwrap();

    let Slot::Beside(slot) = spool
        .keep(
            &host,
            "new",
            kept("an older draft"),
            &[std::sync::Arc::from(&b"older image"[..])],
            Holder::Newcomer,
        )
        .unwrap()
    else {
        panic!("expected the draft to be kept beside the record");
    };

    let recovered = spool.take(&host, "new").unwrap().unwrap();
    assert_eq!(recovered.draft, kept("the newer draft"));
    assert!(matches!(&recovered.images[..], [Ok(bytes)] if bytes == b"newer image"));
    let beside = spool.kept_beside(&host, "new", |_| false);
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
    let spool = DraftSpool::new(directory.path());
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
            &[std::sync::Arc::from(&b"first image"[..])],
            Holder::Newcomer,
        )
        .unwrap();

    assert_eq!(
        spool
            .keep(
                &host,
                "new",
                kept("second keep"),
                &[std::sync::Arc::from(&b"second image"[..])],
                Holder::Owner(slot),
            )
            .unwrap(),
        slot
    );

    let beside = spool.kept_beside(&host, "new", |_| false);
    let [Ok(beside)] = &beside[..] else {
        panic!("expected one draft kept beside the record: {beside:?}");
    };
    assert_eq!(beside.spooled.draft, kept("second keep"));
    assert!(matches!(&beside.spooled.images[..], [Ok(bytes)] if bytes == b"second image"));
    let folder = directory.path().join("drafts").join("forge-new.kept-1");
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);

    spool.forget(&host, "new", slot).unwrap();
    assert!(!folder.exists());
    assert!(spool.kept_beside(&host, "new", |_| false).is_empty());
}
