use artisan_api::{Item, ItemId, ItemKind, LiveTarget, Phase, Status, ThreadId};
use editor_aqua::core::{CoreEvent, HostEvent, HostId};
use editor_aqua::shell::host::event_cause;
use editor_aqua::shell::refresh::Refresh;
use std::sync::Arc;

fn host(name: &str) -> HostId {
    HostId(name.to_owned())
}

fn thread(name: &str) -> ThreadId {
    ThreadId(name.to_owned())
}

fn live(host_name: &str, thread_name: &str) -> CoreEvent {
    CoreEvent::Host {
        host: host(host_name),
        event: HostEvent::Live {
            thread_id: thread(thread_name),
            target: LiveTarget::Text {
                item_id: ItemId::new(),
            },
        },
    }
}

fn host_event(host_name: &str, event: HostEvent) -> CoreEvent {
    CoreEvent::Host {
        host: host(host_name),
        event,
    }
}

fn stored(host_name: &str, thread_name: &str, status: Status) -> CoreEvent {
    host_event(
        host_name,
        HostEvent::Item(Arc::new(Item {
            id: ItemId::from("reply"),
            thread_id: thread(thread_name),
            run_id: None,
            position: Default::default(),
            seq: Default::default(),
            parent_id: None,
            status,
            created_at: Default::default(),
            updated_at: Default::default(),
            kind: ItemKind::AssistantMessage {
                text: "An answer".to_owned().into(),
                phase: Phase::Final,
            },
        })),
    )
}

fn refresh(
    event: &CoreEvent,
    watched: Option<&(HostId, ThreadId)>,
    on_screen: bool,
) -> Option<Refresh> {
    event_cause(event, watched, on_screen).map(|cause| cause.refresh())
}

#[test]
fn live_deltas_of_the_open_thread_refresh_only_the_transcript() {
    let watched = (host("a"), thread("open"));
    assert_eq!(
        refresh(&live("a", "open"), Some(&watched), true),
        Some(Refresh::Transcript)
    );
}

#[test]
fn stored_updates_to_the_open_thread_refresh_only_the_transcript() {
    let watched = (host("a"), thread("open"));
    for status in [Status::Running, Status::Done] {
        assert_eq!(
            refresh(&stored("a", "open", status), Some(&watched), true),
            Some(Refresh::Transcript)
        );
    }
}

#[test]
fn updates_to_threads_not_shown_change_nothing() {
    let watched = (host("a"), thread("open"));
    for event in [
        live("a", "other"),
        live("b", "open"),
        stored("a", "other", Status::Done),
        stored("b", "open", Status::Running),
    ] {
        assert_eq!(refresh(&event, Some(&watched), true), None);
    }
    assert_eq!(refresh(&live("a", "open"), None, true), None);
}

#[test]
fn other_threads_changing_refresh_the_sidebar_and_transcript_beside_an_open_thread() {
    let watched = (host("a"), thread("open"));
    for event in [
        HostEvent::ThreadChanged {
            thread_id: thread("other"),
        },
        HostEvent::ThreadSummaryChanged {
            thread_id: thread("other"),
        },
    ] {
        assert_eq!(
            refresh(&host_event("a", event), Some(&watched), true),
            Some(Refresh::Sections)
        );
    }
}

#[test]
fn thread_changes_off_the_thread_screen_or_to_the_open_thread_rebuild_the_window() {
    let watched = (host("a"), thread("open"));
    let other = || HostEvent::ThreadChanged {
        thread_id: thread("other"),
    };
    let open = || HostEvent::ThreadChanged {
        thread_id: thread("open"),
    };
    assert_eq!(
        refresh(&host_event("a", other()), Some(&watched), false),
        Some(Refresh::Workspace)
    );
    assert_eq!(
        refresh(&host_event("a", open()), Some(&watched), true),
        Some(Refresh::Workspace)
    );
}

#[test]
fn host_wide_and_core_events_rebuild_the_window() {
    let watched = (host("a"), thread("open"));
    for event in [
        host_event("a", HostEvent::Changed),
        CoreEvent::SettingsChanged,
        CoreEvent::ScreenChanged,
    ] {
        assert_eq!(
            refresh(&event, Some(&watched), true),
            Some(Refresh::Workspace)
        );
    }
}
