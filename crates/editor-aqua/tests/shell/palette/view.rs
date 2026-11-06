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
