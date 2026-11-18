use artisan_api::{
    ProjectId, ProjectRepository,
    tls::{Identity, fingerprint},
};
use editor_core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use std::net::TcpListener;

fn branch(name: &str) -> ProjectRepository {
    ProjectRepository {
        branch: Some(name.to_owned()),
        host: Some("github".to_owned()),
        web_url: None,
        worktree: None,
    }
}

#[test]
fn rereading_an_unchanged_repository_leaves_the_window_alone() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .expect("core opens");
    let listener = TcpListener::bind("127.0.0.1:0").expect("a Forge that never answers");
    let identity = Identity::generate("repository-test").expect("identity");
    let pin = fingerprint(identity.certificate_der().expect("certificate").as_ref());
    let config = HostConfig {
        id: HostId("repository-forge".into()),
        name: "Repository".into(),
        address: listener.local_addr().expect("address").to_string(),
        fingerprint: pin.clone(),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(&config.address, &pin, &identity).expect("client");
    core.add_test_host(&config, client);
    let data = core.host_mut(&config.id).expect("host");
    data.test_stop_connection();
    let project = ProjectId("project".into());
    assert!(
        data.test_repository(project.clone(), Some(branch("main"))),
        "the first read shows the branch"
    );
    assert!(
        !data.test_repository(project.clone(), Some(branch("main"))),
        "a periodic re-read of the same branch must not rebuild the window"
    );
    assert!(
        data.test_repository(project.clone(), Some(branch("feature"))),
        "a switched branch is shown"
    );
    assert!(data.test_repository(project, None), "leaving git is shown");
}
