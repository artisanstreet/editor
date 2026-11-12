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
fn a_search_lists_every_match_from_inbox_and_history_in_one_list() {
    let project = ProjectId::from("editor");
    let lists = || Lists {
        inbox: vec![
            row(&project, "Subagent status bugs"),
            row(&project, "Installer binary"),
        ],
        history: vec![HistoryGroup {
            age: AgeGroup::LastDay,
            rows: vec![
                row(&project, "Gradient BUTTON variants"),
                row(&project, "Status line wording"),
            ],
        }],
        next_tick: Some(7),
    };
    let titles = |query: &str| {
        let found = lists().searched(query);
        assert!(found.history.is_empty());
        found
            .inbox
            .into_iter()
            .map(|row| row.title)
            .collect::<Vec<_>>()
    };

    let unfiltered = lists().searched("  ");
    assert_eq!((unfiltered.inbox.len(), unfiltered.history.len()), (2, 1));
    assert_eq!(
        titles("status"),
        ["Subagent status bugs", "Status line wording"]
    );
    assert_eq!(titles("button grad"), ["Gradient BUTTON variants"]);
    assert_eq!(titles("nothing here"), Vec::<String>::new());
}
