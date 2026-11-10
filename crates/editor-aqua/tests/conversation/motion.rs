use editor_aqua::conversation::compaction::{CompactionMotion, CompactionPhase};
use editor_aqua::conversation::navigator::tick_indices;
use editor_aqua::conversation::scroll::{SmoothScroll, Step, wants_older};
use std::time::{Duration, Instant};

#[test]
fn scroll_easing_uses_elapsed_time_and_preserves_tail_following() {
    let now = Instant::now();
    let mut scroll = SmoothScroll {
        destination: None,
        frame: Some(now),
    };
    let first = scroll.step(
        true,
        false,
        false,
        false,
        false,
        0.0,
        100.0,
        now + Duration::from_millis(90),
    );
    let Step::Scroll {
        offset,
        continue_animation,
    } = first
    else {
        panic!("expected eased scrolling")
    };
    assert!(continue_animation);
    assert!((offset - (-100.0 * (1.0 - (-1.0_f32).exp()))).abs() < 0.001);
    assert_eq!(
        scroll.step(true, true, false, false, false, offset, 100.0, now),
        Step::FollowTail
    );
    assert!(wants_older(500.0, -1199.0));
    assert!(!wants_older(500.0, -1200.0));
}

#[test]
fn compaction_completes_entrance_and_plays_checkpoint_after_settling() {
    let now = Instant::now();
    let mut motion = CompactionMotion::first_seen(CompactionPhase::Running, true, now);
    let (start, active) = motion.advance(CompactionPhase::Running, true, now);
    assert!(active);
    assert_eq!(start.open, 0.0);
    let (rest, _) = motion.advance(
        CompactionPhase::Running,
        true,
        now + Duration::from_millis(600),
    );
    assert_eq!(rest.open, 1.0);
    let (swap, _) = motion.advance(
        CompactionPhase::Done,
        true,
        now + Duration::from_millis(601),
    );
    assert_eq!(swap.swap, Some((CompactionPhase::Running, 0.0)));
    let (pulse, _) = motion.advance(
        CompactionPhase::Done,
        true,
        now + Duration::from_millis(1001),
    );
    assert!(pulse.pulse.is_some());
}

#[test]
fn navigator_keeps_the_active_turn_when_sampling_long_history() {
    let indices = tick_indices(10_000, 50, Some(6234));
    assert_eq!(indices.len(), 50);
    assert_eq!(indices.first(), Some(&0));
    assert_eq!(indices.last(), Some(&9999));
    assert!(indices.contains(&6234));
    assert!(indices.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(tick_indices(0, 0, None).is_empty());
}

#[test]
fn work_entrance_measures_unclipped_content_and_restarts_only_after_eviction() {
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::{Box as UiBox, Key};
    use aqua_ui::style::{Length, Style};
    use editor_aqua::conversation::fold::RowKey;
    use editor_aqua::conversation::view::motion::RowMotion;
    use editor_aqua::ui::motion::MotionCurve;
    use std::cell::Cell;
    use std::rc::Rc;

    let key = RowKey::Item(artisan_api::ItemId("work".into()));
    let now = Instant::now();
    let mut motion = RowMotion::default();
    let first = motion
        .entrance(key.clone(), true, true, false, now)
        .unwrap();
    assert!(first.settling);
    let invalidated = Rc::new(Cell::new(false));
    let notification = invalidated.clone();
    let element = first
        .wrap(
            UiBox::new()
                .style(
                    Style::new()
                        .width(Length::Px(120.0))
                        .height(Length::Px(32.0)),
                )
                .build(),
            8.0,
            Rc::new(move || notification.set(true)),
        )
        .with_key("entrance-test");
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(300.0, 200.0, 128.0);
    app.tree_mut().set_gpui_layout_scale(Some(128.0));
    app.mount(|_| element).unwrap();
    app.layout().unwrap();
    app.build_frame().unwrap();
    assert!(invalidated.get());

    let halfway = motion
        .entrance(
            key.clone(),
            true,
            true,
            false,
            now + Duration::from_millis(125),
        )
        .unwrap();
    assert!(halfway.settling);
    let element = halfway
        .wrap(
            UiBox::new()
                .style(
                    Style::new()
                        .width(Length::Px(120.0))
                        .height(Length::Px(32.0)),
                )
                .build(),
            8.0,
            Rc::new(|| {}),
        )
        .with_key("entrance-test");
    let mut next = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    next.set_viewport(300.0, 200.0, 128.0);
    next.tree_mut().set_gpui_layout_scale(Some(128.0));
    next.mount(|_| element).unwrap();
    next.layout().unwrap();
    let hash = aqua_ui::tree::hash_key(Key::from("entrance-test"));
    let node = next
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            next.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(hash))
        })
        .unwrap();
    let height = next.tree().pre_transform_rect(node).unwrap().h;
    assert!((height - 32.0 * MotionCurve::SmoothOut.sample_f32(0.5)).abs() < 0.01);

    let scale = 1.25_f32;
    let grow = MotionCurve::SmoothOut.sample_f32(0.5);
    let element = motion
        .entrance(
            key.clone(),
            true,
            true,
            false,
            now + Duration::from_millis(125),
        )
        .unwrap()
        .wrap(
            UiBox::new()
                .style(
                    Style::new()
                        .width(Length::Px(120.0))
                        .height(Length::Px(32.0)),
                )
                .build(),
            8.0,
            Rc::new(|| {}),
        )
        .with_key("entrance-scaled");
    let root = UiBox::new()
        .layout("column")
        .style(
            Style::new()
                .width(Length::Px(300.0))
                .height(Length::Px(200.0)),
        )
        .child(element)
        .build();
    let mut scaled = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    scaled.set_viewport(300.0, 200.0, scale);
    scaled.tree_mut().set_gpui_layout_scale(Some(scale));
    scaled.mount(|_| root).unwrap();
    scaled.layout().unwrap();
    let hash = aqua_ui::tree::hash_key(Key::from("entrance-scaled"));
    let node = scaled
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            scaled
                .tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(hash))
        })
        .unwrap();
    let bounds = scaled.tree().pre_transform_rect(node).unwrap();
    let snap = |value: f32| (value.abs() - 0.5).ceil().copysign(value);
    let origin = snap(-8.0 * (1.0 - grow) * scale);
    let extent = snap(32.0 * grow * scale);
    let expected = (snap(origin + extent) - snap(origin)) / scale;
    assert!((bounds.y - origin / scale).abs() < 0.01);
    assert!((bounds.h - expected).abs() < 0.01);
    assert!(
        motion
            .entrance(
                key.clone(),
                true,
                true,
                false,
                now + Duration::from_millis(401)
            )
            .is_none()
    );
    assert!(
        motion
            .entrance(
                key.clone(),
                true,
                true,
                false,
                now + Duration::from_millis(402)
            )
            .is_none()
    );
    assert!(
        motion
            .entrance(
                key.clone(),
                true,
                true,
                false,
                now + Duration::from_millis(403)
            )
            .is_none()
    );

    let history = RowKey::Item(artisan_api::ItemId("history".into()));
    assert!(
        motion
            .entrance(history.clone(), false, true, false, now)
            .is_none()
    );
    assert!(
        motion
            .entrance(
                history.clone(),
                true,
                true,
                false,
                now + Duration::from_millis(1)
            )
            .is_none()
    );
    let reduced = RowKey::Item(artisan_api::ItemId("reduced".into()));
    assert!(motion.entrance(reduced, true, true, true, now).is_none());
}

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
    use aqua_text::{SharedFonts, SharedRuns};
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
        .with_key("conversation-transcript")
    })
    .unwrap();
    app.build_frame().unwrap();
    app
}

#[test]
fn transcript_follows_the_native_tail_and_yields_to_glides_and_the_reader() {
    use aqua_ui::collection::FollowState;
    use editor_aqua::conversation::adapter::ScrollDirective;
    use editor_aqua::conversation::scroll::{
        apply_directive, apply_step, following_tail, pause_tail,
    };

    let mut app = transcript();
    let now = Instant::now();
    let mut scroll = SmoothScroll::default();
    assert!(!following_tail(&app));

    assert!(apply_step(&mut app, Step::FollowTail, 0.0));
    app.build_frame().unwrap();
    assert!(following_tail(&app));
    let root = app.tree().root().unwrap();
    assert_eq!(app.tree().scroll_offset(root).unwrap().y, 300.0);
    assert!(!apply_step(&mut app, Step::FollowTail, 0.0));

    assert_eq!(
        apply_directive(&mut app, &mut scroll, &ScrollDirective::EaseToEnd, now, 0.0),
        Some(true)
    );
    assert_eq!(
        app.collection_follow_state("conversation-transcript"),
        Some(FollowState::Normal)
    );
    assert_eq!(scroll.destination, None);
    assert_eq!(scroll.frame, Some(now));

    assert!(apply_step(
        &mut app,
        Step::Scroll {
            offset: -120.0,
            continue_animation: true
        },
        0.0
    ));
    app.build_frame().unwrap();
    assert_eq!(app.tree().scroll_offset(root).unwrap().y, 120.0);
    assert!(!following_tail(&app));

    assert_eq!(
        apply_directive(&mut app, &mut scroll, &ScrollDirective::FollowEnd, now, 0.0),
        Some(true)
    );
    app.build_frame().unwrap();
    assert!(following_tail(&app));
    assert_eq!(app.tree().scroll_offset(root).unwrap().y, 300.0);

    assert!(pause_tail(&mut app));
    assert_eq!(
        app.collection_follow_state("conversation-transcript"),
        Some(FollowState::Tail {
            is_following: false
        })
    );
    assert!(!pause_tail(&mut app));
}

#[test]
fn row_directives_move_the_transcript_off_the_tail() {
    use editor_aqua::conversation::adapter::ScrollDirective;
    use editor_aqua::conversation::scroll::{apply_directive, enter_tail, following_tail};

    let mut app = transcript();
    let now = Instant::now();
    let mut scroll = SmoothScroll::default();
    assert!(enter_tail(&mut app));
    app.build_frame().unwrap();
    assert_eq!(
        apply_directive(
            &mut app,
            &mut scroll,
            &ScrollDirective::ToRow {
                index: 2,
                animate: true
            },
            now,
            0.0
        ),
        Some(true)
    );
    assert!(!following_tail(&app));
    assert_eq!(scroll.destination, Some(-40.0));
    assert_eq!(
        apply_directive(
            &mut app,
            &mut scroll,
            &ScrollDirective::ToRow {
                index: 99,
                animate: false
            },
            now,
            0.0
        ),
        None
    );
}
