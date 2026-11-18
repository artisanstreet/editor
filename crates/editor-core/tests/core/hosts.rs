use editor_core::{EditorFiles, HostConfig, HostId, HostKind};
use std::sync::Barrier;

fn host(id: &str) -> HostConfig {
    HostConfig {
        id: HostId(id.to_owned()),
        name: id.to_owned(),
        address: "127.0.0.1:1".to_owned(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    }
}

#[test]
fn concurrent_first_launches_agree_on_the_stored_identity() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let launches = 8;
    let barrier = Barrier::new(launches);
    let identities: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..launches)
            .map(|_| {
                scope.spawn(|| {
                    let files = EditorFiles {
                        dir: directory.path().to_owned(),
                    };
                    barrier.wait();
                    let identity = files.identity().expect("identity opens");
                    serde_json::to_string(&identity).expect("identity encodes")
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("launch thread"))
            .collect()
    });
    let stored = serde_json::to_string(
        &EditorFiles {
            dir: directory.path().to_owned(),
        }
        .identity()
        .expect("stored identity"),
    )
    .expect("identity encodes");
    for identity in identities {
        assert_eq!(
            identity, stored,
            "a launch kept using an identity that lost the race to disk"
        );
    }
}

#[test]
fn two_editors_adding_hosts_keep_both_pairings() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let first = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let second = first.clone();
    let stale = second.hosts().expect("hosts read");
    first
        .update_hosts(|file| file.hosts.push(host("first")))
        .expect("first save");
    second
        .update_hosts(|file| {
            assert!(stale.hosts.is_empty());
            file.hosts.push(host("second"));
        })
        .expect("second save");
    let saved: Vec<_> = first
        .hosts()
        .expect("hosts read")
        .hosts
        .into_iter()
        .map(|config| config.id.0)
        .collect();
    assert_eq!(saved, ["first", "second"]);
}

#[test]
fn an_unreadable_hosts_file_is_never_replaced() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let path = directory.path().join("hosts.json");
    std::fs::write(&path, b"{\"hosts\": [ truncated").expect("seed hosts.json");
    files
        .update_hosts(|file| file.hosts.push(host("new")))
        .expect_err("an unreadable hosts.json refuses changes");
    assert_eq!(
        std::fs::read(&path).expect("hosts.json still exists"),
        b"{\"hosts\": [ truncated"
    );
}
