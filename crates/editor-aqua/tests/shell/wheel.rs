use aqua_core::{InputEvent, Point, ScrollDelta};
use aqua_text::{SharedFonts, SharedRuns};
use editor_aqua::conversation::scroll::{
    SmoothScroll, Step, apply_step, enter_tail, following_tail, pause_tail,
};
use editor_aqua::shell::wheel::{Surface, WheelGlides};
use std::time::{Duration, Instant};

const TRANSCRIPT: &str = "conversation-transcript";

struct Rows;

impl aqua_ui::collection::CollectionSource for Rows {
    fn len(&self) -> usize {
        20
    }
    fn key(&self, index: usize) -> aqua_ui::element::Key {
        index.into()
    }
    fn layout(&self, width: f32) -> aqua_ui::collection::CollectionLayout {
        aqua_ui::collection::CollectionLayout {
            content_width: width,
            column_widths: Vec::new(),
        }
    }
    fn header(
        &self,
        _: &aqua_ui::collection::CollectionLayout,
    ) -> Option<aqua_ui::element::Element> {
        None
    }
    fn row(
        &self,
        _: usize,
        _: &aqua_ui::collection::CollectionLayout,
    ) -> aqua_ui::element::Element {
        aqua_ui::element::Box::new()
            .style(
                aqua_ui::style::Style::new()
                    .height(aqua_ui::style::px(20.0))
                    .flex_shrink(0.0),
            )
            .build()
    }
    fn role(&self) -> aqua_ui::collection::CollectionRole {
        aqua_ui::collection::CollectionRole::List
    }
    fn validate(&self) -> Result<(), aqua_ui::collection::CollectionError> {
        Ok(())
    }
}

fn transcript() -> aqua::UiApp {
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(100.0, 100.0, 1.0);
    app.mount(|_| {
        aqua_ui::element::Element::collection(aqua_ui::collection::CollectionDescriptor::new(
            std::rc::Rc::new(Rows),
            vec![
                aqua_ui::style::Style::new()
                    .width(aqua_ui::style::px(100.0))
                    .height(aqua_ui::style::px(100.0)),
            ],
            true,
            20.0,
            2,
        ))
        .with_key(TRANSCRIPT)
    })
    .unwrap();
    app.build_frame().unwrap();
    app
}

fn offset(app: &aqua::UiApp) -> f32 {
    let root = app.tree().root().unwrap();
    app.tree().scroll_offset(root).unwrap().y
}

fn notch(lines: f32) -> InputEvent {
    InputEvent::Scroll {
        at: Point::new(50.0, 50.0),
        delta: ScrollDelta {
            x: 0.0,
            y: lines * 16.0,
        },
    }
}

fn glide_to_rest(app: &mut aqua::UiApp, glides: &mut WheelGlides) {
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(16));
        let advance = glides.advance(app);
        app.build_frame().unwrap();
        if !advance.active {
            return;
        }
    }
    panic!("the wheel glide never settled");
}

#[test]
fn a_wheel_notch_up_from_the_tail_leaves_the_transcript_where_the_glide_ends() {
    let mut app = transcript();
    let mut glides = WheelGlides::default();
    assert!(enter_tail(&mut app));
    app.build_frame().unwrap();
    assert!(following_tail(&app));
    assert_eq!(offset(&app), 300.0);

    assert!(pause_tail(&mut app));
    assert_eq!(glides.offer(&mut app, &notch(-3.0)), Some(Surface::Transcript));
    glide_to_rest(&mut app, &mut glides);

    let rested = offset(&app);
    assert!(rested < 300.0 - 40.0, "the notch moved the transcript up, got {rested}");
    assert!(!following_tail(&app));
    assert!(!glides.active());
}

#[test]
fn frames_after_the_glide_never_snap_the_transcript_back_to_the_tail() {
    let mut app = transcript();
    let mut glides = WheelGlides::default();
    let mut smooth = SmoothScroll::default();
    assert!(enter_tail(&mut app));
    app.build_frame().unwrap();
    assert!(pause_tail(&mut app));
    glides.offer(&mut app, &notch(-3.0));
    glide_to_rest(&mut app, &mut glides);
    let rested = offset(&app);

    for _ in 0..30 {
        let step = smooth.step(
            false,
            false,
            false,
            false,
            following_tail(&app),
            -rested,
            300.0,
            Instant::now(),
        );
        assert_eq!(step, Step::Idle);
        apply_step(&mut app, step, 0.0);
        app.build_frame().unwrap();
        assert_eq!(offset(&app), rested);
    }
}

#[test]
fn a_notch_down_from_a_scrolled_position_moves_toward_the_tail_without_reentering_it() {
    let mut app = transcript();
    let mut glides = WheelGlides::default();
    assert!(apply_step(
        &mut app,
        Step::Scroll {
            offset: -100.0,
            continue_animation: false
        },
        0.0
    ));
    app.build_frame().unwrap();
    assert_eq!(offset(&app), 100.0);

    assert_eq!(glides.offer(&mut app, &notch(1.0)), Some(Surface::Transcript));
    glide_to_rest(&mut app, &mut glides);

    let rested = offset(&app);
    assert!(rested > 100.0 && rested < 300.0, "got {rested}");
    assert!(!following_tail(&app));
}

#[test]
fn a_precise_wheel_gesture_cancels_the_glide_instead_of_queueing_it() {
    let mut app = transcript();
    let mut glides = WheelGlides::default();
    assert!(enter_tail(&mut app));
    app.build_frame().unwrap();
    pause_tail(&mut app);
    glides.offer(&mut app, &notch(-3.0));
    assert!(glides.active());
    let precise = InputEvent::Scroll {
        at: Point::new(50.0, 50.0),
        delta: ScrollDelta { x: 0.0, y: -7.5 },
    };
    assert_eq!(glides.offer(&mut app, &precise), None);
    assert!(!glides.active());
}
