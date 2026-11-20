use super::*;
use crate::composer::draft_session::{KeptDraft, Unrecovered};
use artisan_api::tls::{Identity, fingerprint};
use editor_core::{HostConfig, HostKind};

struct Device {
    directory: tempfile::TempDir,
    core: Core,
    host: HostId,
    client: ForgeClient,
}

impl Device {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut core = test_support::editor::core::test_core(directory.path());
        let identity = Identity::generate("recovery-test").unwrap();
        let config = HostConfig {
            id: HostId("recovering".into()),
            name: "Forge".into(),
            address: "127.0.0.1:1".into(),
            fingerprint: fingerprint(identity.certificate_der().unwrap().as_ref()),
            kind: HostKind::Local,
        };
        let client = ForgeClient::new(&config.address, &config.fingerprint, &identity).unwrap();
        test_support::editor::core::connect_client(&mut core, &config, client.clone());
        Self {
            directory,
            core,
            host: config.id,
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
        .keep(&device.host, "new", kept("an older draft"), &[])
        .unwrap();
    let record = device.drafts().join("recovering-new.json");
    std::fs::write(&record, b"{ not json").unwrap();

    let lease = device.reopen();
    let Some(Unrecovered::SetAside(set_aside)) = lease.take_unrecovered() else {
        panic!("expected the unreadable draft to be reported");
    };
    assert_eq!(
        std::fs::read(set_aside).unwrap(),
        b"{ not json",
        "the unreadable draft is kept where the notice says"
    );
}

#[test]
fn signing_out_keeps_a_recovered_draft_forge_has_not_saved_yet() {
    let mut device = Device::new();
    device
        .spool()
        .keep(&device.host, "new", kept("not on Forge yet"), &[])
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
    device.core.remove_host(&device.host).unwrap();
    sessions.retain_hosts(&device.core);
    assert!(lease.read(DraftSession::removed));
    drop(lease);
    drop(sessions);
    let kept = device.spool().take(&device.host, "new").unwrap();
    assert_eq!(
        kept.map(|kept| kept.draft.body.text),
        Some("not on Forge yet".to_owned()),
        "signing back in brings the draft back"
    );
}

fn saved_by_forge(device: &Device) -> DraftHandle {
    let mut state = DraftSession::new("new".into());
    assert!(matches!(state.next_operation(), Some(Operation::Load)));
    state.loaded(Ok(artisan_api::DraftSnapshot::default()));
    let (changed, _) = watch::channel(0);
    DraftHandle(Arc::new(Inner {
        host: device.host.clone(),
        scope: "new".into(),
        client: device.client.clone(),
        spool: Some(device.spool()),
        shared: Mutex::new(Shared {
            state,
            viewers: Vec::new(),
            timer: None,
            version: 0,
            kept: Kept::Recovered,
        }),
        changed,
        cancelled: CancellationToken::new(),
    }))
}

#[test]
fn a_recovered_draft_that_could_not_be_removed_once_saved_is_removed_on_closing() {
    let device = Device::new();
    device
        .spool()
        .keep(&device.host, "new", kept("saved since"), &[])
        .unwrap();
    let record = device.drafts().join("recovering-new.json");
    let kept_record = std::fs::read(&record).unwrap();
    std::fs::remove_file(&record).unwrap();
    std::fs::create_dir(&record).unwrap();
    std::fs::write(record.join("blocker"), b"").unwrap();

    let session = saved_by_forge(&device);
    session.retry();
    std::fs::remove_dir_all(&record).unwrap();
    std::fs::write(&record, kept_record).unwrap();
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
