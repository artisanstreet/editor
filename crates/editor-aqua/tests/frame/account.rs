use editor_aqua::frame::account::Identity;

#[test]
fn username_replaces_email_without_replacing_the_display_name() {
    assert_eq!(
        Identity::of(" Sander ", "sander@example.com", Some(" ss ")),
        Identity::Named {
            name: "Sander".into(),
            detail: "@ss".into()
        }
    );
    assert_eq!(
        Identity::of("", "sander@example.com", Some("@ss")),
        Identity::Unnamed {
            detail: "@ss".into()
        }
    );
    for username in [None, Some(""), Some("  ")] {
        assert_eq!(
            Identity::of("Sander", "sander@example.com", username),
            Identity::Named {
                name: "Sander".into(),
                detail: "sander@example.com".into()
            }
        );
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one menu hover scenario walked frame by frame"
)]
fn hovering_a_usage_row_in_the_open_menu_shows_its_hover_card() {
    use aqua::UiApp;
    use aqua_core::{InputEvent, Point, PointerEvent, PointerKind};
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::{element::Box as UiBox, popup::PopupFocus, style::Style, tree::NodeKind};
    use artisan_api::{EngineId, UsageCadence};
    use editor_aqua::frame::account::{Account, Sheet, menu};
    use editor_aqua::frame::limits::{EngineLimits, Limit};
    use editor_aqua::frame::menu::Menu;
    use editor_aqua::ui::{
        popover::{AnimatedPickerHost, PickerEntrance},
        theme::ArtisanTheme,
    };
    use std::{rc::Rc, time::Duration};

    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(800.0)
            .height(900.0)
            .child(
                UiBox::new()
                    .width(80.0)
                    .height(30.0)
                    .style(Style::new().absolute_at(Some(0.0), Some(800.0), None, None))
                    .build()
                    .with_key("owner"),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(800.0, 900.0, 1.0);
    app.build_frame().unwrap();
    let owner_key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("owner"));
    let owner = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(owner_key))
        })
        .unwrap();
    let account = Account {
        identity: Identity::of("Sander", "sander@example.com", None),
        avatar: None,
        limits: vec![EngineLimits {
            engine: EngineId::Claude,
            limits: vec![Limit {
                label: "Weekly".into(),
                cadence: UsageCadence::Week,
                scope: None,
                used_percent: 40.0,
                resets_at: None,
            }],
        }],
    };
    let state = Menu::default();
    let content = || {
        menu(
            &ArtisanTheme::resolved(),
            &Sheet {
                account: &account,
                menu: &state,
                width: 300.0,
                now: std::time::Instant::now(),
                emit: Rc::new(|_| {}),
            },
        )
    };
    let mut host = AnimatedPickerHost::open_content(
        &mut app,
        owner,
        content(),
        8.0,
        PopupFocus::Preserve,
        PickerEntrance::Scale {
            origin: Point::new(0.0, 1.0),
        },
        Duration::from_millis(100),
        Duration::from_millis(100),
        0.0,
        |_| {},
    )
    .unwrap();
    let mut frame = 0_u32;
    let mut run_until = |app: &mut UiApp, host: &mut AnimatedPickerHost, end_ms: u32| {
        while frame * 16 < end_ms {
            frame += 1;
            let now = f64::from(frame) * 0.016;
            host.refresh_content(app, content()).unwrap();
            host.advance(app, now).unwrap();
            app.advance_clock(now);
            app.build_frame().unwrap();
        }
    };
    let shows = |app: &UiApp, wanted: &str| {
        app.tree().traverse().into_iter().any(|node| {
            matches!(&app.tree().get(node).unwrap().kind, NodeKind::Text(text) if text.text() == wanted)
        })
    };
    run_until(&mut app, &mut host, 300);
    let row = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.semantics.label.contains("Weekly limit"))
        })
        .unwrap();
    let rect = app.tree().rect(row).unwrap();
    app.handle_input(&InputEvent::PointerMove(PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(rect.w.mul_add(0.5, rect.x), rect.h.mul_add(0.5, rect.y)),
        buttons: 0,
    }));
    run_until(&mut app, &mut host, 1000);
    assert!(shows(&app, "Weekly usage for Claude Code"));
}

#[test]
fn every_entry_in_the_account_menu_leads_somewhere() {
    use aqua_core::Role;
    use editor_aqua::frame::account::{Account, Sheet, menu};
    use editor_aqua::frame::menu::{Effect, Event, Menu};
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::{cell::RefCell, rc::Rc, time::Instant};
    use test_support::editor::ui::{click, mounted};

    let account = Account {
        identity: Identity::of("Sander", "sander@example.com", None),
        avatar: None,
        limits: Vec::new(),
    };
    let state = Menu::default();
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let mut app = mounted(
        400.0,
        600.0,
        menu(
            &ArtisanTheme::resolved(),
            &Sheet {
                account: &account,
                menu: &state,
                width: 300.0,
                now: Instant::now(),
                emit: Rc::new(move |event| sink.borrow_mut().push(event)),
            },
        ),
    );
    let entries = app
        .tree()
        .traverse()
        .into_iter()
        .filter_map(|node| {
            let record = app.tree().get(node)?;
            (record.semantics.role == Role::MenuItem).then(|| record.semantics.label.clone())
        })
        .collect::<Vec<_>>();
    assert_eq!(entries, ["Sander", "Theme", "Settings", "Sign out"]);
    let now = Instant::now();
    for (key, effect) in [
        ("artisan-account-identity", Effect::OpenAccount),
        ("artisan-account-Theme", Effect::FlipTheme),
        ("artisan-account-Settings", Effect::OpenSettings),
        ("artisan-account-SignOut", Effect::SignOut),
    ] {
        events.borrow_mut().clear();
        click(&mut app, key);
        let mut menu = Menu::default();
        menu.apply(Event::Toggle, now, false);
        let picked = events
            .borrow()
            .iter()
            .filter_map(|event| menu.apply(*event, now, false))
            .collect::<Vec<_>>();
        assert_eq!(picked, [effect], "clicking {key}");
    }
}
