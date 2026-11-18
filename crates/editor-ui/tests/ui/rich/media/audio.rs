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
            Rc::new(|element, _| element),
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
    let failed = "Couldn't load tracks/intro.ogg: the host did not answer";
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
