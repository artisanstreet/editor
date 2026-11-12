use artisan_api::{ProjectId, ThreadId};
use editor_aqua::sidebar::{
    geometry::{Entry, ThreadLayout, entries, entry_keys},
    list::{AgeGroup, HistoryGroup, Lists, RowData, RowState},
};

fn row(project: &ProjectId, title: &str) -> RowData {
    RowData {
        id: ThreadId::new(),
        title: title.to_owned(),
        subtitle: "Shared project label".to_owned(),
        project: project.clone(),
        state: RowState::Idle,
        selected: false,
        settleable: true,
        matched: Vec::new(),
    }
}

#[test]
fn inbox_groups_by_project_identity_preserving_activity_order_and_history() {
    let a = ProjectId::new();
    let b = ProjectId::new();
    let scratch = ProjectId::scratch();
    let a1 = row(&a, "newest a");
    let b1 = row(&b, "newest b");
    let a2 = row(&a, "older a");
    let scratch1 = row(&scratch, "no project");
    let b2 = row(&b, "older b");
    let history = row(&a, "settled");
    let lists = Lists {
        inbox: vec![
            a1.clone(),
            b1.clone(),
            a2.clone(),
            scratch1.clone(),
            b2.clone(),
        ],
        history: vec![HistoryGroup {
            age: AgeGroup::LastThreeDays,
            rows: vec![history.clone()],
        }],
        hits: Vec::new(),
        next_tick: None,
    };
    let closed = entries(&lists, false);
    let projects: Vec<_> = closed
        .iter()
        .filter_map(|entry| match entry {
            Entry::Project { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(projects, vec![a, b, scratch]);
    let threads: Vec<_> = closed
        .iter()
        .filter_map(|entry| match entry {
            Entry::Thread(row, ThreadLayout::Grouped) => Some(row.id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(threads, vec![a1.id, a2.id, b1.id, b2.id, scratch1.id]);
    let keys = entry_keys(&closed);
    assert_eq!(
        keys.len(),
        keys.iter().collect::<std::collections::HashSet<_>>().len()
    );
    let open = entries(&lists, true);
    let history_entries: Vec<_> = open
        .iter()
        .skip_while(|entry| !matches!(entry, Entry::History))
        .filter_map(|entry| match entry {
            Entry::Thread(row, ThreadLayout::Standalone) => Some(row.id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(history_entries, vec![history.id]);
}

#[test]
fn a_search_ranks_fuzzy_matches_from_inbox_and_history_and_marks_what_matched() {
    let project = ProjectId::from("editor");
    let lists = || Lists {
        inbox: vec![
            row(&project, "Subagent status and breadcrumb title bugs"),
            row(&project, "Installer binary"),
        ],
        history: vec![HistoryGroup {
            age: AgeGroup::LastDay,
            rows: vec![
                row(&project, "Gradient BUTTON variants"),
                row(&project, "Status line wording"),
            ],
        }],
        hits: Vec::new(),
        next_tick: Some(7),
    };
    let found = |query: &str| {
        let found = lists().searched(query);
        assert!(found.history.is_empty());
        found
            .inbox
            .into_iter()
            .map(|row| {
                let marks: Vec<String> = row
                    .matched
                    .iter()
                    .map(|range| row.title[range.clone()].to_owned())
                    .collect();
                (row.title, marks)
            })
            .collect::<Vec<_>>()
    };
    let titles = |query: &str| found(query).into_iter().map(|row| row.0).collect::<Vec<_>>();

    let unfiltered = lists().searched("  ");
    assert_eq!((unfiltered.inbox.len(), unfiltered.history.len()), (2, 1));
    assert_eq!(
        found("status"),
        [
            ("Status line wording".to_owned(), vec!["Status".to_owned()]),
            (
                "Subagent status and breadcrumb title bugs".to_owned(),
                vec!["status".to_owned()]
            ),
        ]
    );
    assert_eq!(
        found("button grad"),
        [(
            "Gradient BUTTON variants".to_owned(),
            vec!["Grad".to_owned(), "BUTTON".to_owned()]
        )]
    );
    assert_eq!(
        titles("sbgt brd"),
        ["Subagent status and breadcrumb title bugs"]
    );
    assert_eq!(titles("shared label").len(), 4);
    assert_eq!(titles("nothing here"), Vec::<String>::new());
}

#[test]
fn message_hits_follow_the_thread_matches_under_their_own_heading() {
    use artisan_api::{ItemId, TextHit};
    use editor_aqua::sidebar::list::HitRow;

    let project = ProjectId::from("editor");
    let hit = |snippet: &str, matched: (u32, u32)| {
        HitRow::of(&TextHit {
            thread_id: ThreadId::new(),
            thread_title: Some("Scene bound fix".into()),
            project_id: project.clone(),
            item_id: ItemId::new(),
            position: 7,
            created_at: 0,
            snippet: snippet.into(),
            matched,
            offset: (40, 51),
            length: 120,
        })
    };
    let found = hit("Raising the Scene Bound fixed it.", (12, 23)).unwrap();
    assert_eq!(found.needle, "Scene Bound");
    assert_eq!(found.title, "Scene bound fix");
    assert_eq!(hit("short", (2, 40)), None);

    let lists = Lists {
        inbox: vec![row(&project, "Scene bound fix")],
        history: Vec::new(),
        hits: vec![found.clone(), hit("the scene bound is 512", (4, 15)).unwrap()],
        next_tick: None,
    };
    let listed = entries(&lists, true);
    let kinds: Vec<&str> = listed
        .iter()
        .map(|entry| match entry {
            Entry::Project { .. } => "project",
            Entry::Thread(..) => "thread",
            Entry::Heading(_) => "heading",
            Entry::Hit(_) => "hit",
            Entry::Gap(_) => "gap",
            Entry::History | Entry::Spacer => "history",
        })
        .collect();
    assert_eq!(
        kinds,
        ["project", "thread", "gap", "heading", "hit", "gap", "hit"]
    );
    let keys = entry_keys(&listed);
    assert_ne!(keys[4], keys[6]);
}
