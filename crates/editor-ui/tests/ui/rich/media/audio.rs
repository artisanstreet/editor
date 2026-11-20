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
            aqua_ui::tree::NodeKind::Box(_)
            | aqua_ui::tree::NodeKind::Media(_)
            | aqua_ui::tree::NodeKind::Scroll(_)
            | aqua_ui::tree::NodeKind::Canvas(_)
            | aqua_ui::tree::NodeKind::Viewport3D(_) => None,
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
                Err(MediaFailure::ForgeUnreachable)
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
        let glyph: GlyphFactory =
            Rc::new(|_, size, _| ui::Box::new().width(size).height(size).build());
        let invalidate: Rc<dyn Fn()> = Rc::new(move || count.set(count.get() + 1));
        block.view(
            "audio",
            &ArtisanTheme::resolved(),
            Some(&fetcher),
            &assets,
            &player,
            &state,
            &glyph,
            &invalidate,
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
    let failed = "Couldn't load intro.ogg.";
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
    assert!(
        !said(&app)
            .iter()
            .any(|text| text.contains("did not answer") || text.contains("tracks/")),
        "the cause and the source path are details, not painted text: {:?}",
        said(&app)
    );
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
        let missing = app.tree().emit_semantics().iter().any(|node| {
            node.description
                .as_deref()
                .is_some_and(|detail| detail.contains("the file does not exist"))
        });
        if missing && said(&app).iter().any(|text| text == failed) {
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

fn node(app: &aqua::UiApp, role: Role, label: &str) -> aqua_core::SemanticNode {
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
    let glyphs: GlyphFactory =
        Rc::new(|_, size, _| ui::Box::new().width(size).height(size).build());
    let button = |shown: &Shown| {
        let count = Rc::clone(&count);
        play_button(
            "row",
            &ArtisanTheme::resolved(),
            shown,
            "intro.ogg",
            false,
            &glyphs,
            Rc::new(|_| {}),
            Rc::new(move || count.set(count.get() + 1)),
        )
    };
    let playing = mounted(button(&Shown::Playing {
        position: Duration::ZERO,
        duration: Duration::from_secs(1),
    }));
    node(&playing, Role::Button, "Pause intro.ogg");
    let mut app = mounted(button(&Shown::Idle));
    let play = node(&app, Role::Button, "Play intro.ogg");
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
    let slider = node(&app, Role::Slider, "Seek intro.ogg");
    assert!(slider.focusable);
    assert_eq!(
        slider.numeric_range,
        Some(NumericRangeSemantics {
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
    let closed = node(&app, Role::Button, "Show transcript");
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
    let open = node(&app, Role::Button, "Hide transcript");
    assert_eq!(open.expanded, Some(true));
    assert!(
        app.tree()
            .emit_semantics()
            .iter()
            .any(|node| node.label == "Hello there")
    );
}

fn snapshot(state: PlaybackState) -> Snapshot {
    Snapshot {
        track: Some("track.wav".into()),
        state,
        position: Duration::from_secs(75),
        duration: Some(Duration::from_secs(100)),
    }
}

fn shown(snapshot: &Snapshot, track: &str) -> (Shown, f32, Option<String>) {
    let shown = Shown::of(snapshot, Path::new(track));
    let (progress, time) = (shown.progress(), shown.time());
    (shown, progress, time)
}

#[test]
fn shared_player_renders_only_its_track_and_clamps_position() {
    let mut value = snapshot(PlaybackState::Playing);
    assert_eq!(shown(&value, "other.wav"), (Shown::Idle, 0.0, None));
    assert_eq!(
        shown(&value, "track.wav"),
        (
            Shown::Playing {
                position: Duration::from_secs(75),
                duration: Duration::from_secs(100),
            },
            0.75,
            Some("1:15 / 1:40".into())
        )
    );
    value.position = Duration::from_secs(200);
    assert_eq!(
        shown(&value, "track.wav"),
        (
            Shown::Playing {
                position: Duration::from_secs(100),
                duration: Duration::from_secs(100),
            },
            1.0,
            Some("1:40 / 1:40".into())
        )
    );
}

#[test]
fn loading_ended_and_missing_duration_keep_source_time_semantics() {
    let mut value = snapshot(PlaybackState::Loading);
    assert_eq!(
        shown(&value, "track.wav"),
        (Shown::Loading, 0.0, Some("Loading…".into()))
    );
    value.state = PlaybackState::Ended;
    value.duration = Some(Duration::from_secs(3725));
    assert_eq!(
        shown(&value, "track.wav"),
        (
            Shown::Ended {
                duration: Duration::from_secs(3725),
            },
            1.0,
            Some("1:02:05".into())
        )
    );
    value.state = PlaybackState::Paused;
    value.duration = None;
    assert_eq!(
        shown(&value, "track.wav"),
        (
            Shown::Paused {
                position: Duration::ZERO,
                duration: Duration::ZERO,
            },
            0.0,
            Some("0:00 / 0:00".into())
        )
    );
}

#[test]
fn data_identity_uses_payload_bytes_and_rejects_remote_url() {
    let track = |source: &str| track_path(&MediaSource::from(source.to_owned()), None, "test");
    let first = track("data:audio/wav;base64,SGVsbG8=").expect("a data track has an identity");
    assert_eq!(Some(first.clone()), track("data:audio/wav;base64,SGVsbG8="));
    assert_ne!(Some(first), track("data:audio/wav;base64,V29ybGQ="));
    assert_eq!(track("https://example.invalid/a.wav"), None);
}
