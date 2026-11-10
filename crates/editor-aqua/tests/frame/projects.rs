use artisan_api::{EngineConfig, EngineId, Project, ProjectId, RunStatus, Thread, ThreadId};
use editor_aqua::frame::projects::groups;
use editor_aqua::sidebar::list::{HostSnapshot, RowState};

struct Host {
    threads: Vec<Thread>,
    projects: Vec<Project>,
    working: Vec<ThreadId>,
}

impl HostSnapshot for Host {
    fn threads(&self) -> Vec<&Thread> {
        self.threads.iter().collect()
    }
    fn project(&self, id: &ProjectId) -> Option<&Project> {
        self.projects.iter().find(|project| &project.id == id)
    }
    fn repository_url(&self, _: &ProjectId) -> Option<&str> {
        None
    }
    fn is_waiting(&self, _: &ThreadId) -> bool {
        false
    }
    fn is_working(&self, id: &ThreadId) -> bool {
        self.working.contains(id)
    }
    fn run_status(&self, _: &ThreadId) -> Option<RunStatus> {
        None
    }
}

fn project(name: &str) -> Project {
    Project {
        id: ProjectId::new(),
        path: format!("/work/{name}"),
        name: name.to_owned(),
        seq: 0,
        created_at: 0,
        removed: false,
    }
}

fn thread(project: &Project, seq: u64, title: Option<&str>) -> Thread {
    Thread {
        id: ThreadId::new(),
        project_id: project.id.clone(),
        title: title.map(str::to_owned),
        config: EngineConfig::new(EngineId::Claude),
        activity_seq: seq,
        activity_at: 0,
        read_seq: seq,
        marked_unread: false,
        settled_seq: 0,
        seq,
        created_at: 0,
        updated_at: 0,
        archived: false,
        parent_thread_id: None,
        spawned_by: None,
        parked_until: None,
    }
}

#[test]
fn projects_follow_their_latest_thread_and_idle_projects_sort_by_name() {
    let editor = project("editor");
    let web = project("web");
    let zeta = project("Zeta");
    let alpha = project("alpha");
    let mut gone = project("gone");
    gone.removed = true;
    let old_editor = thread(&editor, 1, Some("Old"));
    let new_web = thread(&web, 3, Some("  "));
    let new_editor = thread(&editor, 2, Some("New"));
    let mut archived = thread(&web, 9, Some("Archived"));
    archived.archived = true;
    let mut child = thread(&editor, 8, Some("Child"));
    child.parent_thread_id = Some(old_editor.id.clone());
    let host = Host {
        threads: vec![old_editor.clone(), new_web.clone(), new_editor.clone(), archived, child],
        projects: vec![editor.clone(), web.clone(), zeta.clone(), alpha.clone(), gone],
        working: vec![new_editor.id.clone()],
    };
    let tree = groups(&host, &host.projects, Some(&old_editor.id));
    let names = tree.iter().map(|group| group.name.as_str()).collect::<Vec<_>>();
    assert_eq!(names, ["web", "editor", "alpha", "Zeta"]);
    assert_eq!(tree[0].threads[0].title, "New thread");
    let editor_rows = &tree[1].threads;
    assert_eq!(editor_rows.len(), 2);
    assert_eq!(editor_rows[0].id, new_editor.id);
    assert_eq!(editor_rows[0].state, RowState::Working);
    assert!(editor_rows[1].selected && !editor_rows[0].selected);
    assert!(tree[2].threads.is_empty());
}

fn group(threads: usize) -> editor_aqua::frame::projects::Group {
    let project = project("p");
    editor_aqua::frame::projects::Group {
        project: project.id.clone(),
        name: project.name.clone(),
        threads: (0..threads)
            .map(|seq| editor_aqua::frame::projects::ThreadRow {
                id: ThreadId::new(),
                title: format!("t{seq}"),
                state: RowState::Idle,
                selected: false,
            })
            .collect(),
    }
}

#[test]
fn an_open_projects_header_sticks_until_the_next_header_pushes_it_away() {
    use editor_aqua::frame::projects::{Metrics, pinned, sections};
    let groups = [group(3), group(2), group(4)];
    let open = groups[0].project.clone();
    let metrics = Metrics::at(1.0);
    let layout = sections(&groups, metrics, |project| f32::from(u8::from(project == &open)));
    assert_eq!(layout[0].end, 32.0 + 2.0 + 3.0 * 32.0 + 2.0 * 2.0);
    assert_eq!(layout[1].top, layout[0].end + 2.0);
    assert_eq!(pinned(&layout, metrics, 0.0), None);
    assert_eq!(pinned(&layout, metrics, 40.0), Some((open.clone(), 0.0)));
    let near_end = layout[0].end - 10.0;
    assert_eq!(pinned(&layout, metrics, near_end), Some((open, -22.0)));
    assert_eq!(pinned(&layout, metrics, layout[1].top + 5.0), None);
}

#[test]
fn a_half_open_project_lays_out_by_its_openness() {
    use editor_aqua::frame::projects::{Metrics, sections};
    let groups = [group(2), group(0)];
    let layout = sections(&groups, Metrics::at(1.0), |_| 0.5);
    assert_eq!(layout[0].end, 32.0 + 0.5 * (2.0 + 2.0 * 32.0 + 2.0));
    assert_eq!(layout[1].end - layout[1].top, 32.0);
}

#[test]
fn fractional_scales_measure_rows_in_whole_device_pixels() {
    use editor_aqua::frame::projects::{Metrics, sections};
    let groups = [group(200), group(1)];
    let metrics = Metrics::at(1.25);
    let layout = sections(&groups, metrics, |_| 1.0);
    let device = |logical: f32| (logical * 1.25).round();
    assert_eq!(device(metrics.row), 40.0);
    assert_eq!(device(metrics.gap), 2.0);
    assert_eq!(device(layout[0].end), 40.0 + 2.0 + 200.0 * 40.0 + 199.0 * 2.0);
    assert_eq!(device(layout[1].top), device(layout[0].end) + 2.0);
}
