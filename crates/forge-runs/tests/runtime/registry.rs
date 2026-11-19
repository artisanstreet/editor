use crate::runtime::{Holds, Member, PROTOCOL, Peer, Phase, Registry};
use artisan_api::ThreadId;
use std::collections::BTreeSet;

fn member() -> Member {
    Member {
        id: String::new(),
        control: "http://127.0.0.1:1".into(),
        token: "test".into(),
        address: "127.0.0.1:2".parse().unwrap(),
        pid: std::process::id(),
        protocol: PROTOCOL,
        schema: forge_store::Store::schema_version(),
    }
}

fn entries(folder: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_draining_peer_owns_the_threads_it_reports_until_it_exits() {
    let directory = tempfile::tempdir().unwrap();
    let registry = Registry::open(directory.path()).unwrap();
    let old = registry.register(member()).unwrap();
    let new = registry.register(member()).unwrap();
    let held = ThreadId::new();
    let finished = ThreadId::new();
    new.track(vec![Peer {
        member: old.member().clone(),
        holds: Holds::Reported(BTreeSet::from([held.clone(), finished.clone()])),
    }]);
    new.set_phase(Phase::Active);
    assert_eq!(new.owner(&held).unwrap().id, old.member().id);
    new.reported(&old.member().id, BTreeSet::from([held.clone()]));
    assert!(new.owner(&finished).is_none());
    assert_eq!(new.foreign_threads(), BTreeSet::from([held.clone()]));
    new.reported(&old.member().id, BTreeSet::new());
    assert!(new.owner(&held).is_none());
    assert!(new.overlapping());
    assert!(new.forget(&old.member().id).is_empty());
    assert!(!new.overlapping());
}

#[test]
fn an_exited_generation_leaves_no_registry_files() {
    let directory = tempfile::tempdir().unwrap();
    let registry = Registry::open(directory.path()).unwrap();
    let generations = registry.root().join("generations");
    let generation = registry.register(member()).unwrap();
    assert_eq!(registry.peers().unwrap(), vec![generation.member().clone()]);
    drop(generation);
    assert!(entries(&generations).is_empty());
}

#[test]
fn a_crashed_generation_and_retired_lease_folders_are_cleared() {
    let directory = tempfile::tempdir().unwrap();
    let registry = Registry::open(directory.path()).unwrap();
    let generations = registry.root().join("generations");
    let mut crashed = member();
    crashed.id = "crashed".into();
    crate::runtime::write(&generations.join("crashed.json"), &crashed).unwrap();
    std::fs::write(generations.join("crashed.lock"), b"").unwrap();
    for folder in ["threads", "slots"] {
        std::fs::create_dir_all(registry.root().join(folder)).unwrap();
        std::fs::write(registry.root().join(folder).join("0.lock"), b"").unwrap();
    }
    std::fs::write(registry.root().join("admission.lock"), b"").unwrap();
    let live = registry.register(member()).unwrap();
    registry.retire_unused().unwrap();
    assert_eq!(
        entries(registry.root()),
        vec!["binaries".to_owned(), "generations".to_owned()]
    );
    assert_eq!(
        entries(&generations),
        vec![
            format!("{}.json", live.member().id),
            format!("{}.lock", live.member().id)
        ]
    );
}

#[test]
fn a_damaged_registry_file_is_named_in_the_error() {
    let directory = tempfile::tempdir().unwrap();
    let registry = Registry::open(directory.path()).unwrap();
    let active = registry.root().join("active.json");
    std::fs::write(&active, b"{\"id\":").unwrap();
    let error = registry.active().unwrap_err();
    let text = format!("{error:#}");
    assert!(
        text.contains(&format!("parse the registry file {}", active.display())),
        "{text}"
    );
}
