use super::*;
use crate::ui::media::source::MediaFailure;
use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
use aqua_text::{SharedFonts, SharedRuns};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn said(app: &aqua::UiApp) -> Vec<String> {
    app.tree()
        .traverse()
        .into_iter()
        .filter_map(|node| match &app.tree().get(node)?.kind {
            aqua_ui::tree::NodeKind::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn an_unreachable_track_offers_retry_that_fetches_it_again() {
    let reachable = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&reachable);
    let fetcher = MediaFetcher::new("forge", move |_| {
        let up = flag.load(Ordering::SeqCst);
        Box::pin(async move {
            if up {
                Err(MediaFailure::Missing)
            } else {
                Err(MediaFailure::Unreachable)
            }
        })
    })
    .owned_by(&[3], 1);
    let block: AudioBlock = serde_json::from_str(r#"{"src":"tracks/intro.ogg"}"#).unwrap();
    let assets = AudioAssets::new();
    let player = Player::new();
    let state = AudioState::default();
    let invalidated = Rc::new(Cell::new(0));
    let render = || {
        let count = Rc::clone(&invalidated);
        block.view(
            "audio",
            &ArtisanTheme::resolved(),
            Some(&fetcher),
            &assets,
            &player,
            state.clone(),
            Rc::new(|_, size, _| ui::Box::new().width(size).height(size).build()),
            Rc::new(move || count.set(count.get() + 1)),
        )
    };
    let mount = |view: Element| {
        let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
        app.mount(|_| view).unwrap();
        app.set_viewport(600.0, 200.0, 1.0);
        app.build_frame().unwrap();
        app
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let failed = "Couldn't load tracks/intro.ogg: Forge did not answer";
    let mut app = loop {
        assets.drain();
        let app = mount(render());
        if said(&app).iter().any(|text| text == failed) {
            break app;
        }
        assert!(
            Instant::now() < deadline,
            "the unreachable fetch fails in time"
        );
        std::thread::yield_now();
    };
    assert!(said(&app).iter().any(|text| text == "Try again"));
    reachable.store(true, Ordering::SeqCst);
    assert!(app.focus_key("audio-track-0-retry"));
    let enter = KeyEvent {
        key: Key::Named(NamedKey::Enter),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    app.handle_input(&InputEvent::KeyDown(enter));
    app.handle_input(&InputEvent::KeyUp(enter));
    assert_eq!(invalidated.get(), 1);
    loop {
        assets.drain();
        let app = mount(render());
        if said(&app)
            .iter()
            .any(|text| text == "Couldn't load tracks/intro.ogg: the file does not exist")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the retried fetch settles in time"
        );
        std::thread::yield_now();
    }
}

fn mounted(view: Element) -> aqua::UiApp {
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| view).unwrap();
    app.set_viewport(600.0, 200.0, 1.0);
    app.build_frame().unwrap();
    app
}

fn press(app: &mut aqua::UiApp, key: Key) {
    let event = KeyEvent {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    };
    app.handle_input(&InputEvent::KeyDown(event));
    app.handle_input(&InputEvent::KeyUp(event));
}

fn node(app: &aqua::UiApp, role: aqua_core::Role, label: &str) -> aqua_core::SemanticNode {
    app.tree()
        .emit_semantics()
        .into_iter()
        .find(|node| node.role == role && node.label == label)
        .unwrap_or_else(|| panic!("no {role:?} named {label:?}"))
}

fn fire(app: &aqua::UiApp, node: aqua_core::NodeId, kind: aqua_ui::tree::ActionKind) {
    fire_request(app, node, kind, None);
}

fn fire_request(
    app: &aqua::UiApp,
    node: aqua_core::NodeId,
    kind: aqua_ui::tree::ActionKind,
    request: Option<AccessibilityRequest>,
) {
    app.tree().fire_action(aqua_ui::tree::ActionEvent {
        source: None,
        node,
        kind,
        text: None,
        focused: None,
        request,
    });
}

#[test]
fn the_play_button_is_named_and_toggles_from_keyboard_and_assistive_tech() {
    let toggles = Rc::new(Cell::new(0));
    let count = Rc::clone(&toggles);
    let button = |shown: &Shown| {
        let count = Rc::clone(&count);
        play_button(
            "row",
            &ArtisanTheme::resolved(),
            shown,
            "intro.ogg",
            false,
            &(Rc::new(|_, size, _| ui::Box::new().width(size).height(size).build())
                as GlyphFactory),
            Rc::new(|_| {}),
            Rc::new(move || count.set(count.get() + 1)),
        )
    };
    let playing = mounted(button(&Shown::Playing {
        position: Duration::ZERO,
        duration: Duration::from_secs(1),
    }));
    node(&playing, aqua_core::Role::Button, "Pause intro.ogg");
    let mut app = mounted(button(&Shown::Idle));
    let play = node(&app, aqua_core::Role::Button, "Play intro.ogg");
    assert!(play.focusable);
    assert!(app.focus_key("row-button"));
    press(&mut app, Key::Named(NamedKey::Enter));
    assert_eq!(toggles.get(), 1);
    press(&mut app, Key::Char(' '));
    assert_eq!(toggles.get(), 2);
    fire(&app, play.node, aqua_ui::tree::ActionKind::Activate);
    assert_eq!(toggles.get(), 3);
}

#[test]
fn the_progress_line_is_a_slider_that_keyboard_and_assistive_tech_seek() {
    let seeks = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seeks);
    let mut app = mounted(progress_line(
        "row",
        &ArtisanTheme::resolved(),
        &Shown::Paused {
            position: Duration::from_secs(20),
            duration: Duration::from_secs(60),
        },
        "intro.ogg",
        Rc::new(move |to| sink.borrow_mut().push(to)),
        AudioState::default(),
        Rc::new(Cell::new(None)),
        Rc::new(|| {}),
    ));
    let slider = node(&app, aqua_core::Role::Slider, "Seek intro.ogg");
    assert!(slider.focusable);
    assert_eq!(
        slider.numeric_range,
        Some(aqua_core::NumericRangeSemantics {
            min: 0.0,
            max: 60.0,
            value: 20.0,
            step: 5.0,
        })
    );
    assert!(app.focus_key("row-line"));
    for key in [
        Key::Named(NamedKey::Right),
        Key::Named(NamedKey::Left),
        Key::Named(NamedKey::Home),
        Key::Named(NamedKey::End),
    ] {
        press(&mut app, key);
    }
    let kind = aqua_ui::tree::ActionKind::AccessibilityRequest;
    for request in [
        AccessibilityRequest::SetValue(42.0),
        AccessibilityRequest::SetValue(600.0),
        AccessibilityRequest::Increment,
        AccessibilityRequest::Decrement,
    ] {
        fire_request(&app, slider.node, kind, Some(request));
    }
    let seconds = |value: u64| Duration::from_secs(value);
    assert_eq!(
        seeks.borrow().as_slice(),
        &[
            seconds(25),
            seconds(15),
            seconds(0),
            seconds(60),
            seconds(42),
            seconds(60),
            seconds(25),
            seconds(15),
        ]
    );
}

#[test]
fn a_track_transcript_is_parsed_and_disclosed_to_the_accessibility_tree() {
    let single: AudioBlock =
        serde_json::from_str(r#"{"src":"a.ogg","transcript":"  Hello there  "}"#).unwrap();
    assert_eq!(single.tracks[0].transcript.as_deref(), Some("Hello there"));
    let tracks: AudioBlock = serde_json::from_str(
        r#"{"tracks":[{"src":"a.ogg","transcript":"First"},{"src":"b.ogg","transcript":" "}]}"#,
    )
    .unwrap();
    assert_eq!(tracks.tracks[0].transcript.as_deref(), Some("First"));
    assert_eq!(tracks.tracks[1].transcript, None);
    let state = AudioState::default();
    let render = || {
        let (_, _, _, disclosure) = state.row("row");
        mounted(transcript_view(
            "row",
            &ArtisanTheme::resolved(),
            "Hello there",
            disclosure,
            state.clone(),
            Rc::new(|| {}),
        ))
    };
    let mut app = render();
    let closed = node(&app, aqua_core::Role::Button, "Show transcript");
    assert_eq!(closed.expanded, Some(false));
    assert!(
        !app.tree()
            .emit_semantics()
            .iter()
            .any(|node| node.label == "Hello there")
    );
    assert!(app.focus_key("row-transcript"));
    press(&mut app, Key::Named(NamedKey::Enter));
    let app = render();
    let open = node(&app, aqua_core::Role::Button, "Hide transcript");
    assert_eq!(open.expanded, Some(true));
    assert!(
        app.tree()
            .emit_semantics()
            .iter()
            .any(|node| node.label == "Hello there")
    );
}
