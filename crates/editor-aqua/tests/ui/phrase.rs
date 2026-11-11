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
    app.mount(|_| {
        motion
            .phrase(
                "clock",
                "Working for 1s".into(),
                theme.colors.text_muted.to_reference_text_color(),
                theme,
                0.0,
                false,
            )
            .with_key("clock")
    })
    .unwrap();
    let frame = |time| {
        RuntimeInput::Shell(ShellEvent::Frame {
            window: WindowId::new(1).unwrap(),
            presentation: Instant::from_secs_f64(time),
        })
    };
    let mut render = RenderSide::new();
    let initial = app.step(frame(0.0));
    render.step(RenderInput {
        commands: initial.render,
        feedback: Vec::new(),
        present_at: Instant::from_secs_f64(0.0),
    });
    app.refresh(vec![
        motion
            .phrase(
                "clock",
                "Working for 2s".into(),
                theme.colors.text_muted.to_reference_text_color(),
                theme,
                1.0,
                false,
            )
            .with_key("clock"),
    ])
    .unwrap();
    let updated = app.step(frame(1.0));
    render.step(RenderInput {
        commands: updated.render,
        feedback: Vec::new(),
        present_at: Instant::from_secs_f64(1.0),
    });
    assert!(!motion.animating());
    assert!(!render.tracks().is_idle());
    let translations = |render: &RenderSide| {
        render
            .scene()
            .ordered()
            .filter_map(|(_, item)| match item {
                DisplayItem::PushTransform(transform) => Some(transform.ty),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let initial = translations(&render);
    render.step(RenderInput {
        commands: Vec::new(),
        feedback: Vec::new(),
        present_at: Instant::from_secs_f64(1.25),
    });
    assert_ne!(initial, translations(&render));
    render.step(RenderInput {
        commands: Vec::new(),
        feedback: Vec::new(),
        present_at: Instant::from_secs_f64(2.0),
    });
    assert!(render.tracks().is_idle());
}

#[test]
fn an_overflowing_phrase_ends_with_an_ellipsis_after_its_last_whole_word() {
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::{Box as UiBox, Key};
    use aqua_ui::style::Length;
    use editor_aqua::conversation::view::motion::RowMotion;
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::cell::Cell;
    use std::rc::Rc;

    let theme = ArtisanTheme::resolved();
    let words = "Ran 22 commands, used 2 tools, took 2 actions in Artisan Editor";
    let count = words.split(' ').count();
    let refits = Rc::new(Cell::new(0));
    let counted = Rc::clone(&refits);
    let mut motion = RowMotion::default();
    motion.on_refit(Rc::new(move || counted.set(counted.get() + 1)));
    let mut render = || {
        UiBox::new()
            .layout("row")
            .width(Length::Px(260.0))
            .child(motion.phrase(
                "chain",
                words.to_owned(),
                theme.colors.text_muted.to_reference_text_color(),
                theme,
                0.0,
                false,
            ))
            .build()
            .with_key("row")
    };
    let fonts = SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = aqua::UiApp::new(fonts, SharedRuns::new());
    app.set_viewport(800.0, 80.0, 1.0);
    app.mount(|_| render()).unwrap();
    app.build_frame().unwrap();
    assert_eq!(
        refits.get(),
        1,
        "the first paint asks for the cut to be drawn"
    );
    app.refresh(vec![render()]).unwrap();
    app.build_frame().unwrap();
    assert_eq!(refits.get(), 1, "the drawn cut is stable");
    let node = |key: String| {
        let key = aqua_ui::tree::hash_key(Key::from(key));
        app.tree()
            .traverse()
            .into_iter()
            .find(|node| {
                app.tree()
                    .get(*node)
                    .is_some_and(|record| record.key == Some(key))
            })
            .unwrap()
    };
    let rect = |key: String| app.tree().rect(node(key)).unwrap();
    let shown = |key: String| {
        app.tree()
            .get(node(key))
            .unwrap()
            .paint()
            .opacity
            .is_none_or(|opacity| opacity > 0.0)
    };
    let row = rect("row".to_owned());
    let ellipsis = rect("ellipsis".to_owned());
    assert!(shown("ellipsis".to_owned()), "the ellipsis is painted");
    assert!(
        ellipsis.x + ellipsis.w <= row.x + row.w + 0.5,
        "the ellipsis stays inside the row: {ellipsis:?} in {row:?}"
    );
    let last = (0..count)
        .rev()
        .find(|index| shown(format!("word-{index}")))
        .unwrap();
    assert!(last + 1 < count, "the words past the edge are hidden");
    assert!(
        (0..=last).all(|index| shown(format!("word-{index}"))),
        "every word before the cut stays"
    );
    let word = rect(format!("word-{last}"));
    assert!(
        (word.x + word.w - ellipsis.x).abs() < 0.5,
        "the ellipsis follows the last whole word: {word:?} then {ellipsis:?}"
    );
    let next = rect(format!("word-{}", last + 1));
    assert!(
        next.x + next.w + ellipsis.w > row.x + row.w,
        "the next word would not have fit beside the ellipsis"
    );
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

#[cfg(feature = "capture")]
struct PaintedText {
    x: f32,
    y: f32,
    window: aqua_core::Rect,
}

#[cfg(feature = "capture")]
struct PhraseStage {
    app: aqua_runtime::Runtime,
    render: aqua_runtime::RenderSide,
    motion: editor_aqua::conversation::view::motion::RowMotion,
}

#[cfg(feature = "capture")]
impl PhraseStage {
    fn showing(text: &str) -> Self {
        use aqua_text::{SharedFonts, SharedRuns};
        let fonts = SharedFonts::new();
        editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
        let mut stage = Self {
            app: aqua_runtime::Runtime::new(fonts, SharedRuns::new()),
            render: aqua_runtime::RenderSide::new(),
            motion: editor_aqua::conversation::view::motion::RowMotion::default(),
        };
        stage.app.set_viewport(400.0, 80.0, 1.0);
        let element = stage.element(text, 0.0);
        stage.app.mount(|_| element).unwrap();
        stage.present(0.0, true);
        stage
    }

    fn element(&mut self, text: &str, now: f64) -> aqua_ui::element::Element {
        let theme = editor_aqua::ui::theme::ArtisanTheme::resolved();
        self.motion
            .phrase(
                "phrase",
                text.to_owned(),
                theme.colors.text_muted.to_reference_text_color(),
                theme,
                now,
                false,
            )
            .with_key("phrase")
    }

    fn change(&mut self, text: &str, now: f64) {
        let element = self.element(text, now);
        self.app.refresh(vec![element]).unwrap();
        self.present(now, true);
    }

    fn present(&mut self, now: f64, step_application: bool) {
        use aqua_protocol::{Instant, Machine, RenderInput, RuntimeInput, ShellEvent, WindowId};
        let commands = if step_application {
            self.app
                .step(RuntimeInput::Shell(ShellEvent::Frame {
                    window: WindowId::new(1).unwrap(),
                    presentation: Instant::from_secs_f64(now),
                }))
                .render
        } else {
            Vec::new()
        };
        self.render.step(RenderInput {
            commands,
            feedback: Vec::new(),
            present_at: Instant::from_secs_f64(now),
        });
    }

    fn painted(&self) -> Vec<PaintedText> {
        use aqua_core::DisplayItem;
        let mut windows = Vec::new();
        let mut shifts = Vec::new();
        let mut opacities = Vec::new();
        let mut painted = Vec::new();
        for (_, item) in self.render.scene().ordered() {
            match item {
                DisplayItem::PushGpuiContentMask { bounds } => windows.push(*bounds),
                DisplayItem::PopClip => {
                    windows.pop();
                }
                DisplayItem::PushTransform(transform) => shifts.push(transform.ty),
                DisplayItem::PopTransform => {
                    shifts.pop();
                }
                DisplayItem::PushOpacity(opacity) => opacities.push(*opacity),
                DisplayItem::PopOpacity => {
                    opacities.pop();
                }
                DisplayItem::Text { at, .. } if opacities.iter().all(|opacity| *opacity > 0.0) => {
                    let window = *windows.last().expect("phrase text paints inside its mask");
                    let y = at[1] + shifts.iter().sum::<f32>();
                    if (y - window.y).abs() < window.h {
                        painted.push(PaintedText {
                            x: at[0],
                            y,
                            window,
                        });
                    }
                }
                _ => {}
            }
        }
        painted
    }

    fn columns(&self) -> Vec<f32> {
        let mut columns: Vec<f32> = self.painted().iter().map(|text| text.x).collect();
        columns.dedup();
        columns
    }

    fn reel_offsets(&self) -> Vec<Vec<f32>> {
        let painted = self.painted();
        let row = painted.first().expect("phrase paints text").window;
        let mut reels: Vec<(aqua_core::Rect, Vec<f32>)> = Vec::new();
        for text in painted.iter().filter(|text| text.window.w < row.w) {
            match reels.last_mut() {
                Some((window, offsets)) if *window == text.window => {
                    offsets.push(text.y - window.y);
                }
                _ => reels.push((text.window, vec![text.y - text.window.y])),
            }
        }
        reels.into_iter().map(|(_, offsets)| offsets).collect()
    }
}

#[cfg(feature = "capture")]
#[test]
fn rolling_digits_scroll_from_the_old_digit_to_the_new_one() {
    let mut stage = PhraseStage::showing("Working for 8m 19s");
    stage.change("Working for 8m 20s", 1.0);
    let started = stage.reel_offsets();
    assert_eq!(started.len(), 2);
    assert!(
        started.iter().all(|reel| reel.contains(&0.0)),
        "{started:?}"
    );
    stage.present(1.08, false);
    let rolling = stage.reel_offsets();
    assert!(rolling.iter().all(|reel| reel.len() == 2), "{rolling:?}");
    assert!(
        rolling
            .iter()
            .all(|reel| reel.iter().any(|offset| *offset > 0.0 && *offset < 24.0)),
        "{rolling:?}"
    );
    stage.present(2.0, false);
    let landed = stage.reel_offsets();
    assert!(landed.iter().all(|reel| reel.contains(&0.0)), "{landed:?}");
}

#[cfg(feature = "capture")]
#[test]
fn settled_phrase_keeps_the_layout_it_animated_in() {
    for (before, after) in [
        ("Working for 8m 19s", "Working for 8m 20s"),
        (
            "Ran a command, read 9 files",
            "Ran 2 commands, read 10 files",
        ),
    ] {
        let mut stage = PhraseStage::showing(before);
        stage.change(after, 1.0);
        stage.present(2.0, false);
        let animated = stage.columns();
        stage.change(after, 3.0);
        stage.present(4.0, false);
        assert_eq!(animated, stage.columns(), "{after}");
    }
}
