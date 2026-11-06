use artisan_api::{ItemId, LiveTarget, ThreadId};
use editor_aqua::core::{CoreEvent, HostEvent, HostId};
use editor_aqua::shell::host::event_changes_view;

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

#[test]
fn live_deltas_of_the_open_thread_rebuild() {
    let watched = (host("a"), thread("open"));
    assert!(event_changes_view(&live("a", "open"), Some(&watched)));
}

#[test]
fn live_deltas_of_other_threads_do_not_rebuild() {
    let watched = (host("a"), thread("open"));
    assert!(!event_changes_view(&live("a", "other"), Some(&watched)));
    assert!(!event_changes_view(&live("b", "open"), Some(&watched)));
    assert!(!event_changes_view(&live("a", "open"), None));
}

#[test]
fn thread_summaries_of_other_threads_rebuild_the_lists() {
    let watched = (host("a"), thread("open"));
    for event in [
        HostEvent::ThreadChanged {
            thread_id: thread("other"),
        },
        HostEvent::ThreadSummaryChanged {
            thread_id: thread("other"),
        },
    ] {
        assert!(event_changes_view(
            &host_event("a", event),
            Some(&watched)
        ));
    }
}

#[test]
fn host_wide_and_core_events_rebuild() {
    let watched = (host("a"), thread("open"));
    assert!(event_changes_view(
        &host_event("a", HostEvent::Changed),
        Some(&watched)
    ));
    assert!(event_changes_view(&CoreEvent::SettingsChanged, None));
    assert!(event_changes_view(&CoreEvent::ScreenChanged, None));
}
