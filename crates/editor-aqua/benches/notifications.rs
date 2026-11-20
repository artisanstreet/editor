use artisan_api::{
    EngineConfig, EngineId, ProjectId, Run, RunId, RunOutcome, RunState, Thread, ThreadId, Usage,
};
use divan::Bencher;
use editor_aqua::core::{HostId, HostSwitch};
use editor_aqua::shell::notifications::{NotificationHost, Notifications};
use std::collections::{HashMap, HashSet};

const POSTED: usize = 2_000;
const CHANGES: usize = 1_000;

fn main() {
    divan::main();
}

struct Host {
    id: HostId,
    threads: HashMap<ThreadId, Thread>,
    runs: HashMap<ThreadId, Run>,
    working: HashSet<ThreadId>,
}

impl NotificationHost for Host {
    fn id(&self) -> &HostId {
        &self.id
    }
    fn is_live(&self) -> bool {
        true
    }
    fn threads(&self) -> &HashMap<ThreadId, Thread> {
        &self.threads
    }
    fn run(&self, thread: &ThreadId) -> Option<&Run> {
        self.runs.get(thread)
    }
    fn is_working(&self, thread: &ThreadId) -> bool {
        self.working.contains(thread)
    }
    fn is_waiting(&self, _: &ThreadId) -> bool {
        false
    }
    fn is_deleted(&self, _: &ThreadId) -> bool {
        false
    }
    fn switch(&self, _: HostSwitch) -> bool {
        true
    }
}

fn thread(id: &ThreadId) -> Thread {
    Thread {
        id: id.clone(),
        project_id: ProjectId("project".into()),
        title: Some("Fix the build".into()),
        config: EngineConfig::new(EngineId::Claude),
        activity_seq: 2,
        activity_at: 0,
        read_seq: 1,
        marked_unread: false,
        settled_seq: 0,
        seq: 2,
        created_at: 0,
        updated_at: 0,
        archived: false,
        parent_thread_id: None,
        spawned_by: None,
        parked_until: None,
    }
}

fn finished(id: &ThreadId) -> Run {
    Run {
        config: None,
        id: RunId(format!("run-{id}")),
        thread_id: id.clone(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        state: RunState::Finished {
            outcome: RunOutcome::Done,
            error: None,
            ended_at: 0,
        },
        usage: Usage::default(),
        session_id: None,
        seq: 1,
        started_at: 0,
        continuation: None,
    }
}

fn posted() -> (Host, Vec<ThreadId>, Notifications) {
    let ids: Vec<ThreadId> = (0..POSTED)
        .map(|index| ThreadId(format!("thread-{index}")))
        .collect();
    let mut host = Host {
        id: HostId("host".into()),
        threads: ids.iter().map(|id| (id.clone(), thread(id))).collect(),
        runs: HashMap::new(),
        working: ids.iter().cloned().collect(),
    };
    let mut notifications = Notifications::default();
    notifications.reset(&host);
    host.working.clear();
    for id in &ids {
        host.runs.insert(id.clone(), finished(id));
        let posted = notifications.thread_changed(&host, id, None, true);
        assert!(posted.show.is_some(), "bench: {id} is notified");
    }
    (host, ids, notifications)
}

#[divan::bench(sample_count = 20)]
fn change_threads_while_many_notifications_are_posted(bencher: Bencher<'_, '_>) {
    let (host, ids, mut notifications) = posted();
    bencher.bench_local(|| {
        for id in ids.iter().cycle().take(CHANGES) {
            divan::black_box(notifications.thread_changed(&host, id, None, true));
        }
    });
}
