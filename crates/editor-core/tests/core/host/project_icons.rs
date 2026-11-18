use aqua::images::{DecodedImage, ImageAssets, ImageHandle};
use artisan_api::{
    BlobHash, Event, Project, ProjectId,
    tls::{Identity, fingerprint},
};
use editor_core::runtime::runtime;
use editor_core::{BlobImage, Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;
use test_support::wait::wait_until;

struct Forge {
    _directory: tempfile::TempDir,
    _listener: TcpListener,
    core: Core,
    host: HostId,
}

fn forge() -> Forge {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .expect("core opens");
    let listener = TcpListener::bind("127.0.0.1:0").expect("a Forge that never answers");
    let identity = Identity::generate("icons-test").expect("identity");
    let pin = fingerprint(identity.certificate_der().expect("certificate").as_ref());
    let config = HostConfig {
        id: HostId("icons-forge".into()),
        name: "Icons".into(),
        address: listener.local_addr().expect("address").to_string(),
        fingerprint: pin.clone(),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(&config.address, &pin, &identity).expect("client");
    core.add_test_host(&config, client);
    core.host_mut(&config.id)
        .expect("host")
        .test_stop_connection();
    Forge {
        _directory: directory,
        _listener: listener,
        core,
        host: config.id,
    }
}

impl Forge {
    fn decode(&mut self, hash: &BlobHash, image: Arc<ImageHandle>) {
        let host = self.host.clone();
        let generation = self
            .core
            .host_mut(&host)
            .expect("host")
            .test_begin_image(hash.clone());
        let sender = self
            .core
            .host(&host)
            .expect("host")
            .test_transition_sender();
        runtime().block_on(sender.image(hash.clone(), generation, image));
        wait_until("the decoded image lands", Duration::from_secs(10), || {
            self.core.drain_with(|_, _| {});
            matches!(
                self.core.host(&host).expect("host").image(hash),
                Some(BlobImage::Ready(_))
            )
        });
    }
}

fn pixel(assets: &ImageAssets, shade: u8) -> Arc<ImageHandle> {
    Arc::new(
        assets
            .insert(DecodedImage::from_rgba(1, 1, vec![shade, shade, shade, 255]).expect("pixel"))
            .expect("inserted"),
    )
}

#[test]
fn an_evicted_project_icon_frees_its_pixels() {
    let mut forge = forge();
    let host = forge.host.clone();
    let project = ProjectId("project".into());
    let sender = forge
        .core
        .host(&host)
        .expect("host")
        .test_transition_sender();
    runtime().block_on(sender.event(Event::Project {
        project: Project {
            id: project.clone(),
            path: "/work/project".into(),
            name: "project".into(),
            seq: 1,
            created_at: 0,
            removed: false,
        },
    }));
    wait_until("the project arrives", Duration::from_secs(10), || {
        forge.core.drain_with(|_, _| {});
        forge
            .core
            .host(&host)
            .expect("host")
            .projects
            .contains_key(&project)
    });
    let assets = ImageAssets::default();
    let icon = BlobHash("icon".into());
    let image = pixel(&assets, 10);
    let pixels = image.downgrade();
    forge.decode(&icon, image);
    let shown = {
        let data = forge.core.host_mut(&host).expect("host");
        data.test_project_icon(project.clone(), icon.clone());
        data.load_project_icon(&project);
        data.project_icon(&project)
    };
    assert!(shown.is_some(), "the sidebar shows the decoded icon");
    drop(shown);
    let other = pixel(&assets, 30);
    for index in 0..32 {
        forge.decode(&BlobHash(format!("other-{index}")), Arc::clone(&other));
    }
    assert!(
        pixels.upgrade().is_none(),
        "no hidden reference keeps the evicted icon's pixels alive"
    );
    assert!(
        forge
            .core
            .host(&host)
            .expect("host")
            .project_icon(&project)
            .is_none()
    );
}

#[test]
fn retrying_project_icons_reloads_an_icon_whose_image_failed() {
    let mut forge = forge();
    let host = forge.host.clone();
    let project = ProjectId("project".into());
    let sender = forge
        .core
        .host(&host)
        .expect("host")
        .test_transition_sender();
    runtime().block_on(sender.event(Event::Project {
        project: Project {
            id: project.clone(),
            path: "/work/project".into(),
            name: "project".into(),
            seq: 1,
            created_at: 0,
            removed: false,
        },
    }));
    wait_until("the project arrives", Duration::from_secs(10), || {
        forge.core.drain_with(|_, _| {});
        forge
            .core
            .host(&host)
            .expect("host")
            .projects
            .contains_key(&project)
    });
    let icon = BlobHash("icon".into());
    let generation = forge
        .core
        .host_mut(&host)
        .expect("host")
        .test_begin_image(icon.clone());
    runtime().block_on(sender.image_failure(icon.clone(), generation, "connection reset"));
    wait_until("the icon image fails", Duration::from_secs(10), || {
        forge.core.drain_with(|_, _| {});
        matches!(
            forge.core.host(&host).expect("host").image(&icon),
            Some(BlobImage::Failed(_))
        )
    });
    let data = forge.core.host_mut(&host).expect("host");
    data.test_project_icon(project, icon.clone());
    assert!(
        data.project_icons_failed(),
        "the sidebar offers Retry for an icon whose image failed"
    );
    data.retry_project_icons();
    assert!(
        matches!(data.image(&icon), Some(BlobImage::Loading)),
        "Retry downloads the failed icon image again"
    );
    assert!(!data.project_icons_failed());
}

#[test]
fn rereading_an_unchanged_project_icon_leaves_the_window_alone() {
    let mut forge = forge();
    let project = ProjectId("project".into());
    let data = forge.core.host_mut(&forge.host).expect("host");
    assert!(
        data.test_project_icon(project.clone(), BlobHash("icon".into())),
        "the first icon read shows the icon"
    );
    assert!(
        !data.test_project_icon(project.clone(), BlobHash("icon".into())),
        "a periodic re-read of the same icon must not rebuild the window"
    );
    assert!(
        data.test_project_icon(project, BlobHash("new-icon".into())),
        "a replaced icon is shown"
    );
}
