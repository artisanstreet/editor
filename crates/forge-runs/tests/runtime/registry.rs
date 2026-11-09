use crate::runtime::{Member, PROTOCOL, Registry};
use artisan_api::ThreadId;

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

#[test]
fn promotion_fences_new_claims_while_existing_owners_keep_their_leases() {
    let directory = tempfile::tempdir().unwrap();
    let registry = Registry::open(directory.path()).unwrap();
    let first = registry.register(member()).unwrap();
    let second = registry.register(member()).unwrap();
    first.set_accepting(true);
    registry.promote(&first.member).unwrap();
    let thread = ThreadId::new();
    let lease = first.claim(&thread).unwrap().unwrap();
    second.set_accepting(true);
    registry.promote(&second.member).unwrap();
    assert!(first.claim(&ThreadId::new()).unwrap().is_none());
    assert!(second.claim(&thread).unwrap().is_none());
    assert_eq!(
        registry.owner(&thread).unwrap().unwrap().id,
        first.member.id
    );
    drop(lease);
    let _new = second.claim(&thread).unwrap().unwrap();
    assert_eq!(
        registry.owner(&thread).unwrap().unwrap().id,
        second.member.id
    );
}

#[test]
fn generations_share_the_global_capacity_and_failed_candidates_do_not_take_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let registry = Registry::open(directory.path()).unwrap();
    let first = registry.register(member()).unwrap();
    first.set_accepting(true);
    registry.promote(&first.member).unwrap();
    let first_slots = (0..5)
        .map(|_| first.slot().unwrap().unwrap())
        .collect::<Vec<_>>();
    {
        let candidate = registry.register(member()).unwrap();
        assert!(candidate.claim(&ThreadId::new()).unwrap().is_none());
    }
    assert_eq!(registry.active().unwrap().unwrap(), first.member);
    let second = registry.register(member()).unwrap();
    let mut second_slots = (0..3)
        .map(|_| second.slot().unwrap().unwrap())
        .collect::<Vec<_>>();
    assert!(second.slot().unwrap().is_none());
    assert_eq!(second_slots.len(), 3);
    second_slots.pop();
    assert!(first.slot().unwrap().is_some());
    drop(first_slots);
}

#[test]
fn thread_identities_with_extensions_have_independent_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let registry = Registry::open(directory.path()).unwrap();
    let generation = registry.register(member()).unwrap();
    generation.set_accepting(true);
    registry.promote(&generation.member).unwrap();
    let first = ThreadId::from("thread.one");
    let second = ThreadId::from("thread.two");
    let _first = generation.claim(&first).unwrap().unwrap();
    let _second = generation.claim(&second).unwrap().unwrap();
    let owners = registry.owners().unwrap();
    assert_eq!(owners.len(), 2);
    assert!(owners.iter().any(|(id, _)| id == &first));
    assert!(owners.iter().any(|(id, _)| id == &second));
}
