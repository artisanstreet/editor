use super::*;
use crate::core::SettingsPage;

fn event(view: &PaletteView, action: Action) -> Event {
    Event {
        session: view.session(),
        action,
    }
}

#[test]
fn palette_navigation_clamps_and_enter_closes_even_when_empty() {
    let view = PaletteView::new(Rc::new(|_| {}));
    let mut palette = Palette::default();
    assert_eq!(
        view.dispatch(&mut palette, event(&view, Action::Submit))
            .effect,
        Effect::Close(None)
    );
    palette.entries = vec![
        super::super::Entry {
            label: "Settings".into(),
            detail: "Settings".into(),
            screen: Screen::Settings(SettingsPage::General),
        },
        super::super::Entry {
            label: "About".into(),
            detail: "Settings".into(),
            screen: Screen::Settings(SettingsPage::About),
        },
    ];
    assert!(
        !view
            .dispatch(&mut palette, event(&view, Action::Up))
            .changed
    );
    assert!(
        view.dispatch(&mut palette, event(&view, Action::Down))
            .changed
    );
    assert!(
        !view
            .dispatch(&mut palette, event(&view, Action::Down))
            .changed
    );
    assert_eq!(
        view.dispatch(&mut palette, event(&view, Action::Submit))
            .effect,
        Effect::Close(Some(Screen::Settings(SettingsPage::About)))
    );
}

#[test]
fn stale_and_closed_sessions_reject_search_selection_and_callbacks() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = events.clone();
    let old = PaletteView::new(Rc::new(move |event| received.borrow_mut().push(event)));
    let received = events.clone();
    let callback = guarded(
        old.session,
        &old.active,
        Rc::new(move |event| {
            received.borrow_mut().push(event);
        }),
    );
    let next = PaletteView::new(Rc::new(|_| {}));
    let mut palette = Palette::default();
    let stale = event(&old, Action::QueryChanged("needle".into()));
    assert_eq!(
        next.dispatch(&mut palette, stale.clone()).effect,
        Effect::None
    );
    assert_eq!(
        old.dispatch(&mut palette, stale.clone()).effect,
        Effect::Search("needle".into())
    );
    callback(Action::Down);
    assert_eq!(events.borrow().len(), 1);
    old.close();
    callback(Action::Down);
    assert_eq!(events.borrow().len(), 1);
    assert_eq!(old.dispatch(&mut palette, stale).effect, Effect::None);
    assert!(!old.focus_query(&mut aqua::UiApp::new(
        aqua_text::SharedFonts::new(),
        aqua_text::SharedRuns::new()
    )));
}

fn palette_texts(
    view: &PaletteView,
    palette: &Palette,
    emit: Rc<dyn Fn(Event)>,
) -> (aqua::UiApp, Vec<String>) {
    let content = view.content(palette, &ArtisanTheme::resolved(), emit);
    let app = test_support::editor::ui::mounted(600.0, 800.0, content);
    let texts = test_support::editor::ui::painted_texts(&app);
    (app, texts)
}

#[test]
fn an_empty_query_keeps_room_for_recent_threads_after_the_commands() {
    use artisan_api::{Project, ProjectId, Thread, ThreadId};
    use std::collections::{BTreeMap, HashMap};
    use test_support::editor::core::{connect, remote_host, test_core};
    let threads: HashMap<ThreadId, Thread> = (0..20)
        .map(|index| {
            let thread: Thread = serde_json::from_value(serde_json::json!({
                "id": format!("thread-{index}"),
                "project_id": "project",
                "title": format!("Thread {index}"),
                "config": {"engine": "claude"},
                "activity_seq": index,
                "activity_at": index,
                "read_seq": 0,
                "seq": index,
                "created_at": 0,
                "updated_at": 0,
                "archived": false,
            }))
            .unwrap();
            (thread.id.clone(), thread)
        })
        .collect();
    let projects: BTreeMap<ProjectId, Project> = BTreeMap::new();
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_core(directory.path());
    let config = remote_host("forge");
    connect(&mut core, &config);
    let host = config.id.clone();
    let instance = core.host(&host).unwrap().instance();
    let mut palette = Palette::default();
    palette.refresh(
        Some(&host),
        &[super::super::HostSnapshot {
            id: &host,
            name: "Forge",
            instance,
            threads: &threads,
            projects: &projects,
        }],
    );
    let labels: Vec<&str> = palette
        .entries
        .iter()
        .map(|entry| entry.label.as_str())
        .collect();
    assert_eq!(labels.len(), 12);
    assert_eq!(labels[0], "New thread");
    assert_eq!(
        &labels[6..],
        [
            "Thread 19",
            "Thread 18",
            "Thread 17",
            "Thread 16",
            "Thread 15",
            "Thread 14"
        ]
    );
}

#[test]
fn an_unreachable_search_offers_retry_and_an_empty_one_says_nothing_matches() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let received = Rc::clone(&events);
    let emit: Rc<dyn Fn(Event)> = Rc::new(move |event| received.borrow_mut().push(event));
    let view = PaletteView::new(Rc::clone(&emit));
    let mut palette = Palette {
        query: "zebra".into(),
        pending: 2,
        ..Palette::default()
    };
    let (_app, texts) = palette_texts(&view, &palette, Rc::clone(&emit));
    assert!(texts.iter().any(|text| text == "Searching threads…"));
    assert!(!texts.iter().any(|text| text.starts_with("Nothing matches")));

    palette.pending = 0;
    palette.search_error = Some(SearchProblem::Unreachable(2));
    let (mut app, texts) = palette_texts(&view, &palette, Rc::clone(&emit));
    assert!(
        texts
            .iter()
            .any(|text| text == "Search couldn't reach 2 Forges.")
    );
    assert!(texts.iter().any(|text| text == "Nothing matches “zebra”."));
    test_support::editor::ui::click(&mut app, "command-palette-search-action");
    let retry = events.borrow_mut().pop().expect("Retry emits an event");
    assert_eq!(
        view.dispatch(&mut palette, retry).effect,
        Effect::Search("zebra".into())
    );

    palette.search_error = Some(SearchProblem::NoForge);
    let (mut app, texts) = palette_texts(&view, &palette, Rc::clone(&emit));
    assert!(
        texts
            .iter()
            .any(|text| text == "Connect to a Forge to search older threads.")
    );
    test_support::editor::ui::click(&mut app, "command-palette-search-action");
    let forges = events.borrow_mut().pop().expect("Forges emits an event");
    assert_eq!(
        view.dispatch(&mut palette, forges).effect,
        Effect::Close(Some(Screen::Settings(SettingsPage::Hosts)))
    );
}
