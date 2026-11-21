use super::*;
use crate::composer::draft_session::Submission;
use crate::composer::draft_session::{KeptDraft, Unrecovered};
use artisan_api::tls::{Identity, fingerprint};
use editor_core::{HostConfig, HostKind};

enum Forge {
    Silent(test_support::net::SilentListener),
    Refusing(test_support::net::ClosingListener),
}

impl Forge {
    fn address(&self) -> String {
        match self {
            Self::Silent(listener) => listener.address(),
            Self::Refusing(listener) => listener.address(),
        }
        .to_string()
    }
}

struct Device {
    _forge: Forge,
    directory: tempfile::TempDir,
    core: Core,
    host: HostId,
    config: HostConfig,
    client: ForgeClient,
}

impl Device {
    fn new() -> Self {
        Self::at(Forge::Silent(test_support::net::SilentListener::bind()))
    }

    fn refusing() -> Self {
        Self::at(Forge::Refusing(test_support::net::ClosingListener::bind()))
    }

    fn at(forge: Forge) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut core = test_support::editor::core::test_core(directory.path());
        let identity = Identity::generate("recovery-test").unwrap();
        let config = HostConfig {
            id: HostId("recovering".into()),
            name: "Forge".into(),
            address: forge.address(),
            fingerprint: fingerprint(identity.certificate_der().unwrap().as_ref()),
            kind: HostKind::Local,
        };
        let client = ForgeClient::new(&config.address, &config.fingerprint, &identity).unwrap();
        test_support::editor::core::connect_client(&mut core, &config, client.clone());
        Self {
            _forge: forge,
            directory,
            core,
            host: config.id.clone(),
            config,
            client,
        }
    }

    fn spool(&self) -> DraftSpool {
        DraftSpool::new(self.directory.path())
    }

    fn drafts(&self) -> std::path::PathBuf {
        self.directory.path().join("drafts")
    }

    fn reopen(&self) -> DraftLease {
        let instance = self.core.host(&self.host).unwrap().instance();
        DraftSessions::with_spool(self.spool())
            .open(
                self.host.clone(),
                instance,
                "new".into(),
                self.client.clone(),
            )
            .unwrap()
    }
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

#[test]
fn a_kept_image_gone_from_disk_comes_back_as_a_failed_image_beside_the_text() {
    let device = Device::new();
    device
        .spool()
        .keep(
            &device.host,
            "new",
            kept("text beside an image"),
            &[Arc::from(&b"image"[..])],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    for entry in std::fs::read_dir(device.drafts()).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "image")
        {
            std::fs::remove_file(path).unwrap();
        }
    }

    let lease = device.reopen();
    let (text, lost) = lease.read(|state| (state.body().text.clone(), state.lost().len()));
    assert_eq!(text, "text beside an image", "the draft's text comes back");
    assert_eq!(lost, 1, "the missing image shows as one that failed");
    assert!(
        lease.read(DraftSession::keepable),
        "closing again keeps the failed image with the draft"
    );
}

#[test]
fn an_unreadable_kept_draft_is_reported_and_left_for_the_user() {
    let device = Device::new();
    device
        .spool()
        .keep(
            &device.host,
            "new",
            kept("an older draft"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let record = device.drafts().join("recovering-new.json");
    std::fs::write(&record, b"{ not json").unwrap();

    let lease = device.reopen();
    let [Unrecovered::SetAside(set_aside)] = &lease.take_unrecovered()[..] else {
        panic!("expected the unreadable draft to be reported");
    };
    assert_eq!(
        std::fs::read(set_aside).unwrap(),
        b"{ not json",
        "the unreadable draft is kept where the notice says"
    );
}

fn type_into(lease: &DraftLease, text: &str) {
    lease.queue(
        DraftBody {
            text: text.to_owned(),
            ..DraftBody::default()
        },
        lease.read(DraftSession::epoch),
        false,
    );
}

fn png() -> Vec<u8> {
    png_of(1)
}

fn png_of(edge: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(edge, edge)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn repair(device: &mut Device) -> HostInstance {
    device.core.remove_host(&device.host).unwrap();
    test_support::editor::core::connect_client(
        &mut device.core,
        &device.config,
        device.client.clone(),
    );
    device.core.host(&device.host).unwrap().instance()
}

#[test]
fn removing_the_pairing_keeps_a_recovered_draft_as_it_was_last_edited() {
    let mut device = Device::new();
    device
        .spool()
        .keep(
            &device.host,
            "new",
            kept("recovered"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    let lease = sessions
        .open(
            device.host.clone(),
            instance,
            "new".into(),
            device.client.clone(),
        )
        .unwrap();
    type_into(&lease, "edited while Forge was away");

    let replaced = repair(&mut device);
    sessions.retain_hosts(&device.core);
    assert!(lease.read(DraftSession::removed));
    drop(lease);
    let reopened = sessions
        .open(
            device.host.clone(),
            replaced,
            "new".into(),
            device.client.clone(),
        )
        .unwrap();

    assert_eq!(
        reopened.read(|state| state.body().text.clone()),
        "edited while Forge was away",
        "the pairing comes back with the edit, not the older recovered draft"
    );
}

#[test]
fn removing_the_pairing_keeps_a_fresh_draft_its_unanswered_message_and_unfinished_image() {
    let mut device = Device::new();
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    let lease = sessions
        .open(
            device.host.clone(),
            instance,
            "new".into(),
            device.client.clone(),
        )
        .unwrap();
    type_into(&lease, "typed while Forge was away");
    lease.submit(Submission {
        message: artisan_api::SendMessage {
            id: ItemId("unanswered".into()),
            text: "typed while Forge was away".into(),
            images: Vec::new(),
            config: None,
            draft: None,
        },
        create: None,
        thread: ThreadId("thread".into()),
    });
    type_into(&lease, "typed after sending");
    let image = png();
    lease.adopt_image(PendingUpload::kept(image.clone()));

    device.core.remove_host(&device.host).unwrap();
    sessions.retain_hosts(&device.core);
    drop(lease);
    drop(sessions);

    let kept = device
        .spool()
        .take(&device.host, "new")
        .unwrap()
        .expect("the removed pairing's draft is kept on this device");
    let post = kept.draft.post.expect("the unanswered message is kept");
    assert_eq!(post.submission.message.id, ItemId("unanswered".into()));
    assert_eq!(post.sent.text, "typed while Forge was away");
    assert_eq!(kept.draft.body.text, "typed after sending");
    let images = kept
        .images
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(images, vec![image]);
}

#[test]
fn a_send_that_fails_before_the_pairing_is_removed_keeps_the_text_typed_after_it() {
    let mut device = Device::refusing();
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    let lease = sessions
        .open(
            device.host.clone(),
            instance,
            "new".into(),
            device.client.clone(),
        )
        .unwrap();
    type_into(&lease, "typed while Forge was away");
    lease.submit(Submission {
        message: artisan_api::SendMessage {
            id: ItemId("unanswered".into()),
            text: "typed while Forge was away".into(),
            images: Vec::new(),
            config: None,
            draft: None,
        },
        create: None,
        thread: ThreadId("thread".into()),
    });
    type_into(&lease, "typed after sending");
    wait_until(&lease, || lease.read(|state| state.error().is_some()));

    device.core.remove_host(&device.host).unwrap();
    sessions.retain_hosts(&device.core);
    drop(lease);
    drop(sessions);

    let kept = device
        .spool()
        .take(&device.host, "new")
        .unwrap()
        .expect("the removed pairing's draft is kept on this device");
    let post = kept.draft.post.expect("the failed message is kept");
    assert_eq!(post.submission.message.id, ItemId("unanswered".into()));
    assert_eq!(post.sent.text, "typed while Forge was away");
    let typed: Vec<_> = std::iter::once(&kept.draft.body)
        .chain(&kept.draft.body.follow_ups)
        .map(|body| body.text.as_str())
        .collect();
    assert!(
        typed.contains(&"typed after sending"),
        "the text typed after the failed send is kept beside it: {typed:?}"
    );
}

#[test]
fn a_draft_the_device_cannot_keep_stays_held_and_is_kept_when_trying_again() {
    let mut device = Device::new();
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    let lease = sessions
        .open(
            device.host.clone(),
            instance,
            "new".into(),
            device.client.clone(),
        )
        .unwrap();
    type_into(&lease, "only copy of this text");
    std::fs::write(device.drafts(), b"not a folder").unwrap();

    device.core.remove_host(&device.host).unwrap();
    sessions.retain_hosts(&device.core);

    assert!(
        !lease.read(DraftSession::removed),
        "a draft that couldn't be written is not thrown away"
    );
    assert_eq!(
        lease.read(|state| state.body().text.clone()),
        "only copy of this text"
    );
    let unkept = sessions.unkept().expect("the failed write is reported");
    assert_eq!(unkept.count, 1);
    assert_eq!(
        unkept.problem.headline(),
        "Artisan couldn't save your unsent draft to this device. Try again keeps it."
    );
    assert!(
        sessions.keep_unkept().is_some(),
        "trying again while the folder is still blocked keeps reporting it"
    );

    std::fs::remove_file(device.drafts()).unwrap();
    assert_eq!(sessions.keep_unkept(), None);
    assert!(lease.read(DraftSession::removed));
    let kept = device
        .spool()
        .take(&device.host, "new")
        .unwrap()
        .expect("trying again keeps the draft on this device");
    assert_eq!(kept.draft.body.text, "only copy of this text");
}

#[test]
fn a_draft_held_across_two_pairings_keeps_both_pairings_images_when_trying_again() {
    let mut device = Device::new();
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    let first = sessions
        .open(
            device.host.clone(),
            instance,
            "new".into(),
            device.client.clone(),
        )
        .unwrap();
    type_into(&first, "typed before the first removal");
    first.adopt_image(PendingUpload::kept(png_of(1)));
    std::fs::write(device.drafts(), b"not a folder").unwrap();
    let instance = repair(&mut device);
    sessions.retain_hosts(&device.core);
    drop(first);
    assert_eq!(sessions.unkept().map(|unkept| unkept.count), Some(1));

    let second = sessions
        .open(
            device.host.clone(),
            instance,
            "new".into(),
            device.client.clone(),
        )
        .unwrap();
    assert_eq!(
        second.read(|state| state.body().text.clone()),
        "typed before the first removal",
        "reopening the thread brings back the draft the device couldn't keep"
    );
    assert_eq!(sessions.unkept(), None, "the draft is open again, not held");
    second.adopt_image(PendingUpload::kept(png_of(2)));
    repair(&mut device);
    sessions.retain_hosts(&device.core);
    drop(second);
    assert_eq!(sessions.unkept().map(|unkept| unkept.count), Some(1));

    std::fs::remove_file(device.drafts()).unwrap();
    assert_eq!(sessions.keep_unkept(), None);
    let kept = device
        .spool()
        .take(&device.host, "new")
        .unwrap()
        .expect("trying again keeps the draft on this device");
    assert_eq!(kept.draft.body.text, "typed before the first removal");
    let mut images: Vec<Vec<u8>> = kept.images.into_iter().map(Result::unwrap).collect();
    images.sort();
    let mut expected = vec![png_of(1), png_of(2)];
    expected.sort();
    assert_eq!(images, expected, "neither pairing's image is lost");
}

#[cfg(unix)]
fn saved_by_forge(device: &Device) -> DraftHandle {
    let mut state = DraftSession::new("new".into());
    assert!(matches!(state.next_operation(), Some(Operation::Load)));
    state.loaded(Ok(artisan_api::DraftSnapshot::default()));
    let (changed, _) = watch::channel(0);
    DraftHandle(Arc::new(Inner {
        host: device.host.clone(),
        scope: "new".into(),
        client: device.client.clone(),
        spool: device.spool(),
        rejections: Rejections::default(),
        shared: Mutex::new(Shared {
            state,
            viewers: Vec::new(),
            timer: None,
            version: 0,
            kept: Kept::Recovered(Slot::Record),
        }),
        changed,
        cancelled: CancellationToken::new(),
    }))
}

#[cfg(unix)]
#[test]
fn a_recovered_draft_that_could_not_be_removed_once_saved_is_removed_on_closing() {
    use std::os::unix::fs::PermissionsExt;

    let device = Device::new();
    device
        .spool()
        .keep(
            &device.host,
            "new",
            kept("saved since"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    let drafts = device.drafts();
    std::fs::set_permissions(&drafts, std::fs::Permissions::from_mode(0o555)).unwrap();

    let session = saved_by_forge(&device);
    session.retry();
    std::fs::set_permissions(&drafts, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        drafts.join("recovering-new.json").exists(),
        "the draft stays until it can be set aside"
    );
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    sessions
        .sessions
        .insert((device.host.clone(), instance, "new".into()), session);
    drop(sessions);

    assert!(
        device.spool().take(&device.host, "new").unwrap().is_none(),
        "the next start doesn't bring back a draft Forge already saved"
    );
}

#[cfg(unix)]
#[test]
fn retiring_reads_a_pasted_image_outside_the_draft_and_keeps_what_was_typed_meanwhile() {
    let device = Device::new();
    let pasted = device.directory.path().join("pasted.png");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&pasted)
            .status()
            .unwrap()
            .success()
    );
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    let lease = sessions
        .open(
            device.host.clone(),
            instance,
            "new".into(),
            device.client.clone(),
        )
        .unwrap();
    let session = lease.handle.clone();
    session.mutate(|shared| {
        shared.state.adopt(PendingUpload::from_path(
            pasted.clone(),
            "pasted.png".into(),
        ));
    });
    let retiring = std::thread::spawn({
        let session = session.clone();
        move || session.retire()
    });
    let writer = std::fs::OpenOptions::new()
        .write(true)
        .open(&pasted)
        .unwrap();
    assert!(
        session.0.shared.try_lock().is_ok(),
        "reading the pasted image doesn't hold the draft that saves wait on"
    );
    session.mutate(|shared| {
        let epoch = shared.state.epoch();
        assert!(shared.state.queue(
            DraftBody {
                text: "typed while closing".to_owned(),
                ..DraftBody::default()
            },
            epoch,
            false,
        ));
    });
    let replacement = device.directory.path().join("pasted.png.new");
    std::fs::write(&replacement, png()).unwrap();
    std::fs::rename(&replacement, &pasted).unwrap();
    drop(writer);
    retiring.join().unwrap().unwrap();

    assert!(session.read(DraftSession::removed));
    let kept = device
        .spool()
        .take(&device.host, "new")
        .unwrap()
        .expect("the retired draft is kept on this device");
    assert_eq!(kept.draft.body.text, "typed while closing");
    assert_eq!(kept.images.len(), 1);
    assert!(kept.images[0].as_ref().is_ok_and(|bytes| *bytes == png()));
}

#[cfg(unix)]
#[test]
fn a_draft_opened_beside_an_unreadable_one_that_stays_is_kept_beside_it_on_retiring() {
    let mut device = Device::new();
    let scope = "s".repeat(235);
    let stem = format!("recovering-{scope}");
    let record = device.drafts().join(format!("{stem}.json"));
    std::fs::create_dir_all(device.drafts()).unwrap();
    std::fs::write(&record, b"{ not json").unwrap();
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    let lease = sessions
        .open(device.host.clone(), instance, scope, device.client.clone())
        .unwrap();
    assert!(matches!(
        lease.take_unrecovered()[..],
        [Unrecovered::Left(_)]
    ));
    type_into(&lease, "typed beside the unreadable one");

    device.core.remove_host(&device.host).unwrap();
    sessions.retain_hosts(&device.core);

    assert_eq!(
        std::fs::read(&record).unwrap(),
        b"{ not json",
        "the unreadable draft stays untouched for the user"
    );
    let beside = device
        .drafts()
        .join(format!("{stem}.kept-1"))
        .join(format!("{stem}.json"));
    let beside = std::fs::read_to_string(&beside).expect("the draft is written beside it");
    assert!(
        beside.contains("typed beside the unreadable one"),
        "{beside}"
    );
    assert!(lease.read(DraftSession::removed));
}

#[cfg(unix)]
#[test]
fn a_session_kept_beside_an_unreadable_draft_again_and_again_reuses_one_slot() {
    let device = Device::new();
    let scope = "s".repeat(235);
    let stem = format!("recovering-{scope}");
    std::fs::create_dir_all(device.drafts()).unwrap();
    std::fs::write(device.drafts().join(format!("{stem}.json")), b"{ not json").unwrap();
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    let lease = sessions
        .open(device.host.clone(), instance, scope, device.client.clone())
        .unwrap();
    type_into(&lease, "first");
    assert!(lease.keep_locally().unwrap());
    type_into(&lease, "second");
    assert!(lease.keep_locally().unwrap());

    let slots: Vec<_> = std::fs::read_dir(device.drafts())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.contains(".kept-"))
        .collect();
    assert_eq!(slots, [format!("{stem}.kept-1")]);
    let beside = std::fs::read_to_string(
        device
            .drafts()
            .join(format!("{stem}.kept-1"))
            .join(format!("{stem}.json")),
    )
    .unwrap();
    assert!(beside.contains("second"), "{beside}");
}

#[cfg(unix)]
#[test]
fn reopening_beside_an_unreadable_draft_brings_back_the_one_kept_beside_it() {
    let mut device = Device::new();
    let scope = "s".repeat(235);
    let stem = format!("recovering-{scope}");
    std::fs::create_dir_all(device.drafts()).unwrap();
    std::fs::write(device.drafts().join(format!("{stem}.json")), b"{ not json").unwrap();
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    let lease = sessions
        .open(
            device.host.clone(),
            instance,
            scope.clone(),
            device.client.clone(),
        )
        .unwrap();
    type_into(&lease, "typed beside the unreadable one");
    device.core.remove_host(&device.host).unwrap();
    sessions.retain_hosts(&device.core);
    drop(lease);
    let instance = repair(&mut device);

    let lease = DraftSessions::with_spool(device.spool())
        .open(device.host.clone(), instance, scope, device.client.clone())
        .unwrap();
    assert_eq!(
        lease.read(|state| state.body().text.clone()),
        "typed beside the unreadable one"
    );
    assert!(!lease.read(DraftSession::conflict));
}

fn wait_until(lease: &DraftHandle, done: impl Fn() -> bool) {
    let mut changed = lease.subscribe();
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !done() {
                changed.changed().await.unwrap();
            }
        })
        .await
        .expect("the draft finishes its work on this device");
    });
}

fn wait_until_removed(lease: &DraftHandle, path: &std::path::Path) {
    wait_until(lease, || !path.exists());
}

fn kept_beside_a_record(device: &Device) {
    let spool = device.spool();
    spool
        .keep(
            &device.host,
            "new",
            kept("kept in the record"),
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    assert!(matches!(
        spool
            .keep(
                &device.host,
                "new",
                kept("kept beside the record"),
                &[],
                Holder::Newcomer,
            )
            .unwrap(),
        Slot::Beside(_)
    ));
}

#[test]
fn reopening_offers_a_draft_kept_beside_the_record_and_keeping_mine_removes_it() {
    let device = Device::new();
    kept_beside_a_record(&device);

    let lease = device.reopen();
    assert_eq!(
        lease.read(|state| state.body().text.clone()),
        "kept in the record"
    );
    assert!(lease.read(DraftSession::conflict));
    assert_eq!(
        lease.read(|state| state.error().map(|problem| problem.headline().to_owned())),
        Some(
            "Another version of this draft was kept on this device. Choose which version to keep."
                .to_owned()
        )
    );

    lease.keep_local();

    assert!(!lease.read(DraftSession::conflict));
    assert_eq!(
        lease.read(|state| state.body().text.clone()),
        "kept in the record"
    );
    wait_until_removed(&lease, &device.drafts().join("recovering-new.kept-1"));
    assert!(
        device
            .spool()
            .kept_beside(&device.host, "new", |_| false)
            .is_empty()
    );
}

#[test]
fn loading_the_draft_kept_beside_replaces_the_record_and_keeps_it_until_forge_has_it() {
    let device = Device::new();
    kept_beside_a_record(&device);

    let lease = device.reopen();
    lease.use_remote();

    assert!(!lease.read(DraftSession::conflict));
    assert_eq!(
        lease.read(|state| state.body().text.clone()),
        "kept beside the record"
    );
    wait_until_removed(&lease, &device.drafts().join("recovering-new.json"));
    assert!(
        device.spool().take(&device.host, "new").unwrap().is_none(),
        "the version the user didn't pick is removed"
    );
    assert!(lease.keep_locally().unwrap());
    let beside = device.spool().kept_beside(&device.host, "new", |_| false);
    let [Ok(beside)] = &beside[..] else {
        panic!("expected the chosen draft to stay kept beside: {beside:?}");
    };
    assert_eq!(beside.spooled.draft.body.text, "kept beside the record");
    assert!(device.spool().take(&device.host, "new").unwrap().is_none());
}

#[test]
fn taking_a_held_draft_back_does_not_count_as_a_new_draft_the_device_could_not_keep() {
    let device = Device::new();
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    for scope in ["first", "second"] {
        let lease = sessions
            .open(
                device.host.clone(),
                instance,
                scope.into(),
                device.client.clone(),
            )
            .unwrap();
        type_into(&lease, scope);
    }
    std::fs::write(device.drafts(), b"not a folder").unwrap();
    sessions.retire();
    let held = sessions.unkept().expect("both drafts are held");
    assert_eq!(held.count, 2);

    let _first = sessions
        .open(device.host.clone(), instance, "first".into(), device.client)
        .unwrap();

    let still = sessions.unkept().expect("the other draft is still held");
    assert_eq!(still.count, 1);
    assert_eq!(
        still.generation, held.generation,
        "a notice the user dismissed isn't brought back by taking a draft back"
    );
}

fn sending(text: &str, id: &str, create: Option<artisan_api::CreateThread>) -> Submission {
    Submission {
        message: artisan_api::SendMessage {
            id: ItemId(id.into()),
            text: text.into(),
            images: Vec::new(),
            config: None,
            draft: None,
        },
        create,
        thread: ThreadId("thread".into()),
    }
}

fn kept_sending(submission: &Submission) -> KeptDraft {
    serde_json::from_value(serde_json::json!({
        "body": {"text": submission.message.text},
        "post": {
            "submission": submission,
            "sent": {"text": submission.message.text}
        }
    }))
    .unwrap()
}

fn keep_record_and_beside(device: &Device, record: KeptDraft, beside: KeptDraft) {
    let spool = device.spool();
    spool
        .keep(
            &device.host,
            "new",
            record,
            &[],
            Holder::Owner(Slot::Record),
        )
        .unwrap();
    assert!(matches!(
        spool
            .keep(&device.host, "new", beside, &[], Holder::Newcomer)
            .unwrap(),
        Slot::Beside(_)
    ));
}

#[test]
fn loading_a_draft_kept_beside_with_an_unanswered_send_sends_it_again_under_its_ids() {
    let device = Device::new();
    let sent = sending(
        "sent beside",
        "beside-message",
        Some(artisan_api::CreateThread {
            id: ThreadId("thread".into()),
            project_id: artisan_api::ProjectId::from("project"),
            config: artisan_api::EngineConfig::new(artisan_api::EngineId::Codex),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        }),
    );
    keep_record_and_beside(&device, kept("kept in the record"), kept_sending(&sent));

    let lease = device.reopen();
    lease.use_remote();

    assert!(
        lease.read(DraftSession::sending),
        "the message sent from the chosen draft goes out again"
    );
    assert_eq!(
        lease.read(|state| state.kept().post.map(|post| post.submission)),
        Some(sent),
        "it keeps its message and new thread ids, so Forge never takes either twice"
    );
}

#[test]
fn loading_the_draft_kept_beside_keeps_the_records_unanswered_send_until_forge_has_it() {
    let device = Device::new();
    keep_record_and_beside(
        &device,
        kept_sending(&sending("sent from the record", "record-message", None)),
        kept("kept beside the record"),
    );

    let lease = device.reopen();
    lease.use_remote();

    assert_eq!(
        lease.read(|state| state.body().text.clone()),
        "kept beside the record"
    );
    wait_until(&lease, settled(&lease));
    let beside = device.spool().kept_beside(&device.host, "new", |_| false);
    let [Ok(beside)] = &beside[..] else {
        panic!("expected the chosen draft to be kept beside: {beside:?}");
    };
    assert_eq!(
        beside
            .spooled
            .draft
            .set_aside
            .iter()
            .map(|sent| sent.message.id.clone())
            .collect::<Vec<_>>(),
        [ItemId("record-message".into())],
        "the message the user already sent stays on this device until Forge answers"
    );
}

#[test]
fn keeping_mine_over_a_draft_kept_beside_with_an_unanswered_send_keeps_it_until_forge_has_it() {
    let device = Device::new();
    keep_record_and_beside(
        &device,
        kept("kept in the record"),
        kept_sending(&sending("sent beside", "beside-message", None)),
    );

    let lease = device.reopen();
    lease.keep_local();

    assert!(!lease.read(DraftSession::conflict));
    assert_eq!(
        lease.read(|state| state.body().text.clone()),
        "kept in the record"
    );
    wait_until(&lease, settled(&lease));
    let record = device
        .spool()
        .take(&device.host, "new")
        .unwrap()
        .expect("the kept draft stays on this device");
    assert_eq!(
        record
            .draft
            .set_aside
            .iter()
            .map(|sent| sent.message.id.clone())
            .collect::<Vec<_>>(),
        [ItemId("beside-message".into())],
        "the message the user already sent stays on this device until Forge answers"
    );
}

#[test]
fn an_unreadable_draft_kept_beside_is_moved_aside_and_reported_once() {
    let device = Device::new();
    kept_beside_a_record(&device);
    let beside = device.drafts().join("recovering-new.kept-1");
    std::fs::write(beside.join("recovering-new.json"), b"{ not json").unwrap();

    let lease = device.reopen();
    let [Unrecovered::SetAside(set_aside)] = &lease.take_unrecovered()[..] else {
        panic!("expected the unreadable draft kept beside to be reported");
    };
    assert_eq!(
        std::fs::read(set_aside).unwrap(),
        b"{ not json",
        "the unreadable draft is kept where the notice says"
    );
    assert!(!beside.exists(), "it is no longer offered as a draft");
    assert!(!lease.read(DraftSession::conflict));
    drop(lease);

    assert!(
        device.reopen().take_unrecovered().is_empty(),
        "opening again doesn't report it again"
    );
}

#[test]
fn an_empty_folder_left_for_a_draft_kept_beside_is_removed_on_opening() {
    let device = Device::new();
    let beside = device.drafts().join("recovering-new.kept-1");
    std::fs::create_dir_all(&beside).unwrap();

    let lease = device.reopen();

    assert!(!beside.exists());
    assert!(lease.take_unrecovered().is_empty());
}

#[test]
fn a_draft_held_after_remounting_shows_as_new_although_the_last_notice_was_dismissed() {
    let device = Device::new();
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    let lease = sessions
        .open(
            device.host.clone(),
            instance,
            "first".into(),
            device.client.clone(),
        )
        .unwrap();
    type_into(&lease, "first");
    std::fs::write(device.drafts(), b"not a folder").unwrap();
    sessions.retire();
    let dismissed = sessions.unkept().expect("the draft is held").generation;
    std::fs::remove_file(device.drafts()).unwrap();
    assert_eq!(sessions.keep_unkept(), None, "trying again keeps it");

    let mut remounted = DraftSessions::with_spool(device.spool());
    remounted.take_held(sessions.hand_off_held());
    let lease = remounted
        .open(
            device.host.clone(),
            instance,
            "second".into(),
            device.client.clone(),
        )
        .unwrap();
    type_into(&lease, "second");
    std::fs::remove_dir_all(device.drafts()).unwrap();
    std::fs::write(device.drafts(), b"not a folder").unwrap();
    remounted.retire();

    assert_ne!(
        remounted
            .unkept()
            .expect("the new draft is held")
            .generation,
        dismissed,
        "the notice for a draft held after remounting isn't taken for the one dismissed before"
    );
}

fn settled(lease: &DraftHandle) -> impl Fn() -> bool {
    let rejections = lease.0.rejections.clone();
    move || rejections.lock().is_empty()
}

#[cfg(unix)]
#[test]
fn a_draft_the_user_chose_against_that_could_not_be_removed_is_removed_on_the_next_open() {
    use std::os::unix::fs::PermissionsExt;

    let device = Device::new();
    kept_beside_a_record(&device);
    let beside = device.drafts().join("recovering-new.kept-1");
    let tombstone = device.drafts().join("recovering-new.rejected-1");
    let lease = device.reopen();
    std::fs::set_permissions(&beside, std::fs::Permissions::from_mode(0o555)).unwrap();
    lease.keep_local();
    assert!(
        !lease.read(DraftSession::conflict),
        "the choice is made at once"
    );
    wait_until(&lease, settled(&lease));
    assert!(!beside.exists());
    assert!(tombstone.exists(), "the removal failed part way");
    std::fs::set_permissions(&tombstone, std::fs::Permissions::from_mode(0o755)).unwrap();
    drop(lease);

    let lease = device.reopen();
    assert!(
        !lease.read(DraftSession::conflict),
        "the draft the user chose against isn't offered again"
    );
    wait_until_removed(&lease, &tombstone);
}

#[cfg(unix)]
#[test]
fn a_draft_saved_where_a_half_removed_one_was_survives_the_next_open() {
    use std::os::unix::fs::PermissionsExt;

    let device = Device::new();
    kept_beside_a_record(&device);
    let stuck = device.drafts().join("recovering-new.stuck.image");
    std::fs::create_dir(&stuck).unwrap();
    std::fs::write(stuck.join("inside"), b"pinned").unwrap();
    std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o555)).unwrap();
    let lease = device.reopen();
    lease.use_remote();
    wait_until(&lease, settled(&lease));
    drop(lease);
    device
        .spool()
        .keep(
            &device.host,
            "new",
            kept("saved where the old record was"),
            &[],
            Holder::Newcomer,
        )
        .unwrap();

    let lease = device.reopen();
    wait_until(&lease, || {
        !device.drafts().join("recovering-new.rejected-1").exists()
    });
    std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        lease.read(|state| state.body().text.clone()),
        "saved where the old record was",
        "the new draft is recovered rather than taken for the one removed"
    );
    drop(lease);
    assert!(
        device.drafts().join("recovering-new.json").exists(),
        "the new draft stays on this device"
    );
}

#[test]
fn a_choice_still_being_carried_out_is_honoured_by_the_drafts_that_take_over() {
    let device = Device::new();
    kept_beside_a_record(&device);
    let instance = device.core.host(&device.host).unwrap().instance();
    let mut sessions = DraftSessions::with_spool(device.spool());
    sessions.rejections.lock().push(Rejected {
        host: device.host.clone(),
        scope: "new".into(),
        slot: Slot::Record,
    });

    let mut remounted = DraftSessions::with_spool(device.spool());
    remounted.take_held(sessions.hand_off_held());
    let lease = remounted
        .open(
            device.host.clone(),
            instance,
            "new".into(),
            device.client.clone(),
        )
        .unwrap();
    assert_eq!(
        lease.read(|state| state.body().text.clone()),
        "kept beside the record",
        "the draft being discarded isn't brought back"
    );
    assert!(!lease.read(DraftSession::conflict));
    drop(lease);

    assert!(
        device.reopen().read(DraftSession::conflict),
        "drafts that didn't take over know nothing of the choice"
    );
}

fn beside_slot(device: &Device) -> BesideSlot {
    kept_beside_a_record(device);
    let beside = device.spool().kept_beside(&device.host, "new", |_| false);
    let [Ok(beside)] = &beside[..] else {
        panic!("expected one draft kept beside the record: {beside:?}");
    };
    beside.slot
}

const SET_ASIDE_FAILED: &str = "Couldn't send a message from the draft you set aside.";

fn set_aside_failed(lease: &DraftHandle) -> impl Fn() -> bool {
    move || {
        lease.read(|state| state.error().map(|problem| problem.headline().to_owned()))
            == Some(SET_ASIDE_FAILED.to_owned())
    }
}

#[test]
fn a_failed_message_from_a_set_aside_draft_no_device_slot_held_is_shown_kept_and_sent_again() {
    let device = Device::refusing();
    let slot = beside_slot(&device);
    std::fs::remove_file(device.drafts().join("recovering-new.json")).unwrap();
    let mut state = DraftSession::new("new".into());
    state.offer_rival(slot, kept("kept beside"), Vec::new(), Vec::new());
    state.submit(sending("sent before choosing", "displaced", None));
    let (changed, _) = watch::channel(0);
    let session = DraftHandle(Arc::new(Inner {
        host: device.host.clone(),
        scope: "new".into(),
        client: device.client.clone(),
        spool: device.spool(),
        rejections: Rejections::default(),
        shared: Mutex::new(Shared {
            state,
            viewers: Vec::new(),
            timer: None,
            version: 0,
            kept: Kept::Nothing,
        }),
        changed,
        cancelled: CancellationToken::new(),
    }));

    session.use_remote();

    wait_until(&session, set_aside_failed(&session));
    let beside = device.spool().kept_beside(&device.host, "new", |_| false);
    let [Ok(beside)] = &beside[..] else {
        panic!("expected the chosen draft to be kept beside: {beside:?}");
    };
    assert_eq!(
        beside.spooled.draft.set_aside,
        [sending("sent before choosing", "displaced", None)],
        "the failed message is kept on this device"
    );
    session.0.cancelled.cancel();
    drop(session);

    let lease = device.reopen();
    assert_eq!(
        lease.read(|state| state.kept().set_aside),
        [sending("sent before choosing", "displaced", None)],
        "opening the draft again sends it again under its id"
    );
    wait_until(&lease, set_aside_failed(&lease));
}

#[test]
fn a_set_aside_message_is_sent_before_the_chosen_drafts_own_and_only_once_it_is_kept() {
    let device = Device::new();
    let slot = beside_slot(&device);
    let mine = sending("sent from mine", "mine", None);
    let theirs = sending("sent from theirs", "theirs", None);
    let mut state = DraftSession::new("new".into());
    state.recover(kept_sending(&mine), Vec::new(), Vec::new());
    state.offer_rival(slot, kept_sending(&theirs), Vec::new(), Vec::new());
    assert!(matches!(state.next_operation(), Some(Operation::Load)));
    state.loaded(Ok(artisan_api::DraftSnapshot::default()));

    assert_eq!(state.use_remote(), Some(slot));
    let next = |state: &mut DraftSession| loop {
        match state.next_operation() {
            Some(Operation::Write { body, .. }) => {
                state.written(Ok(artisan_api::DraftWriteResult {
                    saved: true,
                    current: artisan_api::DraftSnapshot {
                        revision: 1,
                        body: body.to_wire(),
                    },
                }));
            }
            operation => break operation,
        }
    };
    assert!(
        next(&mut state).is_none(),
        "nothing is sent until the set-aside message is kept on this device"
    );

    state.set_aside_kept();
    let Some(Operation::PostSetAside(first)) = next(&mut state) else {
        panic!("expected the set-aside message to go first");
    };
    assert_eq!(first.message.id, mine.message.id);
    assert!(state.next_operation().is_none());
    state.posted_set_aside(Ok(()));
    let Some(Operation::Post(second)) = next(&mut state) else {
        panic!("expected the chosen draft's message once the set-aside one is through");
    };
    assert_eq!(second.message.id, theirs.message.id);
}
