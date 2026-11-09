use editor_aqua::ui::phrase::{PhraseMotion, WordMotion};

#[cfg(feature = "capture")]
#[test]
fn work_clock_rolls_on_the_render_thread_without_requesting_application_animation() {
    use aqua_core::DisplayItem;
    use aqua_protocol::{Instant, Machine, RenderInput, RuntimeInput, ShellEvent, WindowId};
    use aqua_runtime::{RenderSide, Runtime};
    use aqua_text::{SharedFonts, SharedRuns};
    use editor_aqua::conversation::view::motion::RowMotion;
    use editor_aqua::ui::theme::ArtisanTheme;
    let theme = ArtisanTheme::resolved();
    let mut motion = RowMotion::default();
    let mut app = Runtime::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(400.0, 80.0, 1.0);
    app.mount(|_| motion.phrase("clock", "Working for 1s".into(), theme.colors.text_muted.to_reference_text_color(), theme, 0.0, false).with_key("clock")).unwrap();
    let frame = |time| RuntimeInput::Shell(ShellEvent::Frame { window: WindowId::new(1).unwrap(), presentation: Instant::from_secs_f64(time) });
    let mut render = RenderSide::new();
    let initial = app.step(frame(0.0));
    render.step(RenderInput { commands: initial.render, feedback: Vec::new(), present_at: Instant::from_secs_f64(0.0) });
    app.refresh(vec![motion.phrase("clock", "Working for 2s".into(), theme.colors.text_muted.to_reference_text_color(), theme, 1.0, false).with_key("clock")]).unwrap();
    let updated = app.step(frame(1.0));
    render.step(RenderInput { commands: updated.render, feedback: Vec::new(), present_at: Instant::from_secs_f64(1.0) });
    assert!(!motion.animating());
    assert!(!render.tracks().is_idle());
    let translations = |render: &RenderSide| render.scene().ordered().filter_map(|(_, item)| match item {
        DisplayItem::PushTransform(transform) => Some(transform.ty),
        _ => None,
    }).collect::<Vec<_>>();
    let initial = translations(&render);
    render.step(RenderInput { commands: Vec::new(), feedback: Vec::new(), present_at: Instant::from_secs_f64(1.25) });
    assert_ne!(initial, translations(&render));
    render.step(RenderInput { commands: Vec::new(), feedback: Vec::new(), present_at: Instant::from_secs_f64(2.0) });
    assert!(render.tracks().is_idle());
}

#[test]
fn phrase_tracks_number_rolls_extensions_and_settling() {
    let mut phrase = PhraseMotion::default();
    let (initial, animating) = phrase.words("a task", 0.0, false);
    assert!(!animating);
    assert!(
        initial
            .unwrap()
            .iter()
            .all(|word| word.motion == WordMotion::Still)
    );
    let (updated, animating) = phrase.words("2 tasks", 1.0, false);
    assert!(animating);
    let updated = updated.unwrap();
    assert_eq!(
        updated[0].motion,
        WordMotion::Roll {
            from: "a".to_owned(),
            up: true
        }
    );
    assert_eq!(updated[1].motion, WordMotion::Extend { kept: 4 });
    let (settled, animating) = phrase.words("2 tasks", 1.7, false);
    assert!(!animating);
    assert!(
        settled
            .unwrap()
            .iter()
            .all(|word| word.motion == WordMotion::Still)
    );
    let (reduced, animating) = phrase.words("3 tasks", 2.0, true);
    assert!(reduced.is_none());
    assert!(!animating);
    let (restored, animating) = phrase.words("3 tasks", 2.1, false);
    assert!(!animating);
    assert!(
        restored
            .unwrap()
            .iter()
            .all(|word| word.motion == WordMotion::Still)
    );
}
