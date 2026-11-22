use artisan_api::{Event, InspectorSummary, ItemId, OutlineEntry, Seq, Status, ThreadId};
use editor_core::{Core, HostId};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;

const PATH: &str = "/threads/outlined/inspector";

fn thread() -> ThreadId {
    ThreadId("outlined".into())
}

fn entry(position: Seq, label: &str) -> OutlineEntry {
    OutlineEntry {
        id: ItemId(label.into()),
        position,
        label: label.into(),
    }
}

fn shown(core: &Core, host: &HostId) -> Option<(Seq, Vec<String>)> {
    let summary = core.host(host)?.thread_summary(&thread())?.value.as_ref()?;
    Some((
        summary.revision,
        summary
            .outline
            .iter()
            .map(|entry| entry.label.clone())
            .collect(),
    ))
}

fn settle_on(stubbed: &mut StubbedHost, revision: Seq) -> Vec<String> {
    stubbed.settle("the refreshed summary arrives", |core, host| {
        shown(core, host).is_some_and(|(shown, _)| shown == revision)
    });
    shown(&stubbed.core, &stubbed.host)
        .map(|(_, labels)| labels)
        .expect("the summary is shown")
}

fn touch(stubbed: &StubbedHost) -> Seq {
    stubbed.forge.publish(Event::ItemState {
        id: ItemId("tool".into()),
        thread_id: thread(),
        status: Status::Done,
    })
}

#[test]
fn a_refresh_keeps_the_outline_forge_says_has_not_changed_and_takes_a_new_one() {
    let mut stubbed = StubbedHost::start(empty_state(), axum::Router::new());
    stubbed.host_mut().observe_thread(Some(thread()));
    let first = stubbed.take("GET", PATH);
    assert_eq!(
        first.query(),
        None,
        "the first load asks for the whole outline"
    );
    first.respond(InspectorSummary {
        revision: 0,
        outline: vec![entry(1, "Fix the build")],
        outline_revision: 1,
        ..InspectorSummary::default()
    });
    assert_eq!(settle_on(&mut stubbed, 0), ["Fix the build"]);

    let quiet = touch(&stubbed);
    let refresh = stubbed.take("GET", PATH);
    assert_eq!(refresh.query(), Some("outline=1"));
    refresh.respond(InspectorSummary {
        revision: quiet,
        outline_revision: 1,
        ..InspectorSummary::default()
    });
    assert_eq!(settle_on(&mut stubbed, quiet), ["Fix the build"]);

    let prompted = touch(&stubbed);
    let refresh = stubbed.take("GET", PATH);
    assert_eq!(refresh.query(), Some("outline=1"));
    refresh.respond(InspectorSummary {
        revision: prompted,
        outline: vec![entry(1, "Fix the build"), entry(9, "Now the tests")],
        outline_revision: prompted,
        ..InspectorSummary::default()
    });
    assert_eq!(
        settle_on(&mut stubbed, prompted),
        ["Fix the build", "Now the tests"]
    );
}
