use super::*;

#[test]
fn work_group_header_has_no_generic_fallback() {
    use crate::conversation_scene::WorkGroupLabel;
    assert_eq!(work_group_header_copy(None), None);
    assert_eq!(
        work_group_header_copy(Some(WorkGroupLabel::WorkedFor { millis: 65_000 })),
        Some("Worked for 1m 5s".to_owned())
    );
    assert_eq!(
        work_group_header_copy(Some(WorkGroupLabel::ThoughtFor { millis: 5_000 })),
        Some("Thought for 5s".to_owned())
    );
}

#[gpui::test]
fn group_detail_rows_paint_in_durable_order(cx: &mut TestAppContext) {
    // Mounted order proof to go with the pure merge test: two legacy
    // activity rows must paint top-to-bottom in vec order.
    let detail_scene = ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        vec![
            item(
                "work-a",
                1,
                SceneItemKind::Activity {
                    body: "first".to_owned(),
                    kind: None,
                    detail: None,
                },
                None,
            ),
            item(
                "work-b",
                2,
                SceneItemKind::Activity {
                    body: "second".to_owned(),
                    kind: None,
                    detail: None,
                },
                None,
            ),
        ],
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::Working,
        )],
        Vec::new(),
    )
    .expect("conversation scene is valid");
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        let mut surface = ConversationSurface::new(detail_scene, ThemeMode::Dark, surface_cx);
        surface
            .trace_groups_open
            .get_mut()
            .insert("work-a".to_owned(), true);
        surface
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    let first = cx
        .debug_bounds("artisan-conversation-surface-turn-turn_a-block-work-work-a-detail-0")
        .expect("first detail must paint");
    let second = cx
        .debug_bounds("artisan-conversation-surface-turn-turn_a-block-work-work-a-detail-1")
        .expect("second detail must paint");
    assert!(first.origin.y < second.origin.y);
    assert!(first.size.height > px(0.0));
    cx.update(|_, app| {
        surface.update(app, |surface, _| {
            for action in surface.take_actions() {
                assert!(
                    matches!(
                        action,
                        ConversationSurfaceAction::ViewportObserved(_)
                            | ConversationSurfaceAction::ViewportExtentChanged
                    ),
                    "only legitimate viewport observations may precede assertions, got {action:?}"
                );
            }
        });
        assert!(surface.read(app).pending_actions().is_empty());
    });
}

#[gpui::test]
fn settled_activity_chain_starts_closed_and_toggles_open(cx: &mut TestAppContext) {
    const TRIGGER: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-a-trace-0-trigger";
    const DETAIL: &str = "artisan-conversation-surface-turn-turn_a-block-work-work-a-detail-0";
    const RAIL: &str = "artisan-conversation-surface-turn-turn_a-block-work-work-a-trace-0-rail";
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            activity_chain_scene(
                ConversationLifecycle::Completed,
                TurnNarration::Quiet,
                SceneDisclosure::Open,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    assert!(
        cx.debug_bounds(TRIGGER).is_some(),
        "the chain header paints even when closed"
    );
    assert!(
        cx.debug_bounds(DETAIL).is_none(),
        "a settled chain starts closed"
    );

    let trigger = cx.debug_bounds(TRIGGER).expect("chain trigger paints");
    let center = point(trigger.origin.x + px(4.0), trigger.origin.y + px(4.0));
    cx.simulate_mouse_down(center, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(center, gpui::MouseButton::Left, Modifiers::default());
    settle(cx);
    let detail = cx
        .debug_bounds(DETAIL)
        .expect("toggling the header mounts the rows");
    let rail = cx.debug_bounds(RAIL).expect("the left rail paints");
    assert!(
        rail.size.height >= detail.size.height,
        "the rail spans the mounted rows, got {rail:?} over {detail:?}"
    );
    // The toggle is surface-local: it never emits a scene disclosure
    // action for a chain identity the scene does not own.
    cx.update(|_, app| {
        let actions = surface.read(app).pending_actions().to_vec();
        assert!(
            actions.iter().all(|action| matches!(
                action,
                ConversationSurfaceAction::ViewportObserved(_)
                    | ConversationSurfaceAction::ViewportExtentChanged
            )),
            "the chain toggle stays surface-local, got {actions:?}"
        );
    });
}

#[gpui::test]
fn live_activity_chain_starts_closed(cx: &mut TestAppContext) {
    const DETAIL: &str = "artisan-conversation-surface-turn-turn_a-block-work-work-a-detail-0";
    let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            activity_chain_scene(
                ConversationLifecycle::Active,
                TurnNarration::Working,
                SceneDisclosure::Open,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    assert!(
        cx.debug_bounds(DETAIL).is_none(),
        "a live chain starts closed"
    );
}

#[gpui::test]
fn failed_activity_chain_starts_closed(cx: &mut TestAppContext) {
    const DETAIL: &str = "artisan-conversation-surface-turn-turn_a-block-work-work-a-detail-0";
    let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            activity_chain_scene(
                ConversationLifecycle::Failed,
                TurnNarration::Quiet,
                SceneDisclosure::Open,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    assert!(
        cx.debug_bounds(DETAIL).is_none(),
        "a failed chain starts closed"
    );
}

#[gpui::test]
fn session_activity_chain_carries_kind_and_detail(cx: &mut TestAppContext) {
    const DETAIL: &str = "artisan-conversation-surface-turn-turn_a-block-work-turn_a-detail-1";
    let run = artisan_domain::RunId::parse("run-a").expect("fixture run id is valid");
    let session_scene = ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        vec![
            item(
                "work-a",
                1,
                SceneItemKind::Activity {
                    body: "cargo test --locked".to_owned(),
                    kind: Some("terminal_activity".to_owned()),
                    detail: Some(
                        r#""C:\Program Files\WindowsApps\pwsh.exe" -Command "cargo test --locked""#
                            .to_owned(),
                    ),
                },
                None,
            )
            .with_provenance(ItemProvenance {
                run_id: Some(run),
                lifecycle: Some(ConversationLifecycle::Active),
            }),
        ],
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::Working,
        )],
        Vec::new(),
    )
    .expect("session activity scene is valid");
    let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
        let mut surface = ConversationSurface::new(session_scene, ThemeMode::Dark, surface_cx);
        surface
            .trace_groups_open
            .get_mut()
            .insert("work-a".to_owned(), true);
        surface
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    assert!(
        cx.debug_bounds(DETAIL).is_some(),
        "a live session chain mounts its kinded row"
    );
}

#[gpui::test]
fn unresolved_assistant_links_queue_one_bounded_resolve_request(cx: &mut TestAppContext) {
    let link_scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Completed,
            )],
            vec![item(
                "reply",
                1,
                SceneItemKind::AssistantMessage {
                    body: "See [the docs](https://Example.com/page#section) and [again](https://example.com/page)."
                        .to_owned(),
                    phase: AssistantPhase::Final,
                },
                None,
            )],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Quiet,
            )],
            Vec::new(),
        )
        .expect("conversation scene is valid");
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(link_scene, ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);

    // One render pass observes both links; the fragment variant and the
    // exact duplicate collapse into one canonical resolve URL.
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            let urls = surface
                .take_actions()
                .into_iter()
                .filter_map(|action| match action {
                    ConversationSurfaceAction::ResolveRichLinks { urls } => Some(urls),
                    _ => None,
                })
                .flatten()
                .collect::<Vec<_>>();
            assert_eq!(urls, vec!["https://example.com/page".to_owned()]);

            // A resolved title repaints without another resolve request.
            surface.set_rich_link_title(
                "https://example.com/page",
                &SharedString::from("Resolved Docs"),
                4_000_000_000_000,
                cx,
            );
        });
    });
    settle(cx);
    cx.update(|_, app| {
        surface.update(app, |surface, _| {
            assert_eq!(
                surface
                    .rich_link_titles
                    .lookup("https://example.com/page#section"),
                Some(SharedString::from("Resolved Docs"))
            );
            assert!(
                !surface.take_actions().iter().any(|action| matches!(
                    action,
                    ConversationSurfaceAction::ResolveRichLinks { .. }
                )),
                "a fresh title must not queue another resolve"
            );
        });
    });

    // A failed resolution keeps the authored label and never retries.
    cx.update(|_, app| {
        surface.update(app, |surface, cx| {
            surface.set_rich_link_failure("https://example.com/page", cx);
        });
    });
    settle(cx);
    cx.update(|_, app| {
        surface.update(app, |surface, _| {
            assert!(
                surface
                    .rich_link_titles
                    .lookup("https://example.com/page")
                    .is_none(),
                "a failed resolution keeps the authored label"
            );
            assert!(
                !surface.take_actions().iter().any(|action| matches!(
                    action,
                    ConversationSurfaceAction::ResolveRichLinks { .. }
                )),
                "a failed resolution must not be retried"
            );
        });
    });
}

#[gpui::test]
fn group_disclosure_toggle_emits_typed_action(cx: &mut TestAppContext) {
    // Clicking the group's disclosure trigger must emit exactly one
    // typed toggle request; the scene stays authoritative afterwards.
    let open_scene = ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        vec![item(
            "work-a",
            1,
            SceneItemKind::Activity {
                body: "first".to_owned(),
                kind: None,
                detail: None,
            },
            Some(SceneDisclosure::Open),
        )],
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::Working,
        )],
        Vec::new(),
    )
    .expect("conversation scene is valid");
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(open_scene, ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    // Trigger bounds key follows the disclosure `-trigger` suffix
    // convention on the group disclosure selector.
    let trigger = cx
        .debug_bounds(
            "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-trigger",
        )
        .expect("disclosure trigger must paint");
    let center = point(trigger.origin.x + px(4.0), trigger.origin.y + px(4.0));
    cx.simulate_mouse_down(center, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(center, gpui::MouseButton::Left, Modifiers::default());
    settle(cx);
    cx.update(|_, app| {
        let actions = surface.read(app).pending_actions().to_vec();
        for action in &actions {
            assert!(
                matches!(
                    action,
                    ConversationSurfaceAction::ViewportObserved(_)
                        | ConversationSurfaceAction::ViewportExtentChanged
                        | ConversationSurfaceAction::DisclosureToggleRequested { .. }
                ),
                "only viewport observations and the toggle may be pending, got {action:?}"
            );
        }
        let toggles: Vec<(_, _)> = actions
            .iter()
            .filter_map(|action| match action {
                ConversationSurfaceAction::DisclosureToggleRequested { id, requested_open } => {
                    Some((id.clone(), *requested_open))
                }
                _ => None,
            })
            .collect();
        assert_eq!(toggles.len(), 1, "exactly one toggle, got {actions:?}");
        let (id, requested_open) = toggles.into_iter().next().expect("one toggle");
        assert_eq!(id.as_str(), "work-a");
        assert!(!requested_open, "an open group toggles closed");
    });
}

#[gpui::test]
fn open_flight_paints_a_partial_height_before_settling_full(cx: &mut TestAppContext) {
    const PANEL: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-panel";
    const CONTENT: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-content";
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            disclosure_flight_scene(SceneDisclosure::Closed),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    // Settled closed history keeps its rows unmounted (the reference
    // `details_mounted` policy) and its panel measured at zero.
    assert!(
        cx.debug_bounds(CONTENT).is_none(),
        "closed details stay unmounted"
    );
    assert_eq!(
        cx.debug_bounds(PANEL).expect("panel paints").size.height,
        px(0.0)
    );

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.replace_scene(disclosure_flight_scene(SceneDisclosure::Open), surface_cx);
        });
    });
    settle(cx);
    // The flip frame mounts the rows at the flight's zero-height start and
    // measures the full target during prepaint.
    let flip = cx
        .debug_bounds(PANEL)
        .expect("panel paints on the flip frame");
    let full = cx
        .debug_bounds(CONTENT)
        .expect("the open flight mounts its rows");
    assert!(
        f32::from(flip.size.height) < f32::from(full.size.height),
        "the flip frame starts the flight at zero, got {flip:?}"
    );

    let full_height = f32::from(full.size.height);
    pump_animation_frame_after(cx, Duration::from_millis(60));
    let mid_height = f32::from(
        cx.debug_bounds(PANEL)
            .expect("panel paints mid-flight")
            .size
            .height,
    );
    assert!(
        mid_height > 0.0 && mid_height < full_height,
        "the open flight must pass through a partial height, got {mid_height} of {full_height}"
    );

    pump_animation_frame_after(cx, Duration::from_millis(300));
    let settled = f32::from(
        cx.debug_bounds(PANEL)
            .expect("panel paints settled")
            .size
            .height,
    );
    assert!(
        (settled - full_height).abs() < 0.5,
        "the open flight settles at the measured content height, got {settled} of {full_height}"
    );
}

#[gpui::test]
fn collapse_flight_animates_then_settles_unmounted(cx: &mut TestAppContext) {
    const PANEL: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-panel";
    const CONTENT: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-content";
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            disclosure_flight_scene(SceneDisclosure::Open),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    let full_height = f32::from(
        cx.debug_bounds(CONTENT)
            .expect("an open group mounts its rows")
            .size
            .height,
    );
    let opened = f32::from(cx.debug_bounds(PANEL).expect("panel paints").size.height);
    assert!(
        (opened - full_height).abs() < 0.5,
        "a mounted open group rests at its content height"
    );

    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.replace_scene(disclosure_flight_scene(SceneDisclosure::Closed), surface_cx);
        });
    });
    settle(cx);
    // The collapse starts from the displayed height rather than snapping.
    let start = f32::from(
        cx.debug_bounds(PANEL)
            .expect("panel paints at the collapse start")
            .size
            .height,
    );
    assert!(
        start > full_height * 0.9 && start <= full_height + 0.5,
        "the collapse flight starts at the displayed height, got {start} of {full_height}"
    );

    pump_animation_frame_after(cx, Duration::from_millis(60));
    let mid_height = f32::from(
        cx.debug_bounds(PANEL)
            .expect("panel paints mid-collapse")
            .size
            .height,
    );
    assert!(
        mid_height > 0.0 && mid_height < full_height,
        "the collapse flight must pass through a partial height, got {mid_height} of {full_height}"
    );

    pump_animation_frame_after(cx, Duration::from_millis(300));
    assert_eq!(
        cx.debug_bounds(PANEL)
            .expect("panel paints settled")
            .size
            .height,
        px(0.0)
    );
    // The settled collapse disarms the flight and unmounts the rows again.
    pump_animation_frame_after(cx, Duration::from_millis(20));
    assert!(
        cx.debug_bounds(CONTENT).is_none(),
        "a settled collapse unmounts its rows"
    );
}

#[gpui::test]
fn closing_history_never_reexpands_when_late_tool_rows_arrive(cx: &mut TestAppContext) {
    const PANEL: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-panel";
    const CONTENT: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-content";
    fn updated_scene(rows: u64, disclosure: SceneDisclosure) -> ConversationScene {
        ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            (0..rows)
                .map(|index| {
                    let id = if index == 0 {
                        "work-a".to_owned()
                    } else {
                        format!("work-{index}")
                    };
                    item(
                        &id,
                        index + 1,
                        SceneItemKind::Activity {
                            body: format!("Command {index}"),
                            kind: Some("terminal_activity".to_owned()),
                            detail: Some("cargo check --locked".to_owned()),
                        },
                        Some(disclosure),
                    )
                })
                .collect(),
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Working,
            )],
            Vec::new(),
        )
        .unwrap()
    }
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        let mut surface = ConversationSurface::new(
            updated_scene(1, SceneDisclosure::Open),
            ThemeMode::Dark,
            surface_cx,
        );
        surface
            .trace_groups_open
            .get_mut()
            .insert("work-a".to_owned(), true);
        surface
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    let mut previous_height = f32::from(cx.debug_bounds(PANEL).unwrap().size.height);
    assert!(previous_height > 0.0);
    for frame in 0..12 {
        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                surface.replace_scene(
                    updated_scene(if frame % 2 == 0 { 1 } else { 16 }, SceneDisclosure::Closed),
                    surface_cx,
                );
            });
        });
        pump_animation_frame_after(cx, Duration::from_millis(8));
        let height = f32::from(cx.debug_bounds(PANEL).unwrap().size.height);
        assert!(
            height <= previous_height + 0.5,
            "closing history expanded on frame {frame}: {previous_height} -> {height}"
        );
        previous_height = height;
    }
    pump_animation_frame_after(cx, Duration::from_millis(300));
    for _ in 0..20 {
        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                surface.replace_scene(updated_scene(16, SceneDisclosure::Closed), surface_cx);
            });
        });
        settle(cx);
        assert_eq!(cx.debug_bounds(PANEL).unwrap().size.height, px(0.0));
        assert!(
            cx.debug_bounds(CONTENT).is_none(),
            "settled history must not remount or flash"
        );
    }
}

#[gpui::test]
fn reduced_motion_jumps_the_disclosure(cx: &mut TestAppContext) {
    const PANEL: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-panel";
    const CONTENT: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-content";
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            disclosure_flight_scene(SceneDisclosure::Closed),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    cx.update(|_, app| {
        app.set_reduce_motion(true);
    });

    // The system signal: an open flip lands at full height on its frame.
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.replace_scene(disclosure_flight_scene(SceneDisclosure::Open), surface_cx);
        });
    });
    settle(cx);
    let full_height = f32::from(
        cx.debug_bounds(CONTENT)
            .expect("reduced motion mounts the rows")
            .size
            .height,
    );
    let opened = f32::from(cx.debug_bounds(PANEL).expect("panel paints").size.height);
    assert!(
        (opened - full_height).abs() < 0.5,
        "reduced motion jumps the open flip, got {opened} of {full_height}"
    );
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.replace_scene(disclosure_flight_scene(SceneDisclosure::Closed), surface_cx);
        });
    });
    settle(cx);
    assert_eq!(
        cx.debug_bounds(PANEL).expect("panel paints").size.height,
        px(0.0),
        "reduced motion jumps the close flip"
    );
    assert!(
        cx.debug_bounds(CONTENT).is_none(),
        "reduced motion never mounts settled rows"
    );
    cx.update(|_, app| {
        app.set_reduce_motion(false);
    });

    // The stored explicit override wins over the live system signal.
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.set_status_motion(MotionPolicy::Reduced, surface_cx);
            surface.replace_scene(disclosure_flight_scene(SceneDisclosure::Open), surface_cx);
        });
    });
    settle(cx);
    let opened = f32::from(cx.debug_bounds(PANEL).expect("panel paints").size.height);
    assert!(
        (opened - full_height).abs() < 0.5,
        "an explicit reduced preference jumps the open flip, got {opened} of {full_height}"
    );

    // A policy switch midway through a flight settles at the final state
    // and drops the clock: the close must never replay once motion returns.
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.set_status_motion(MotionPolicy::Full, surface_cx);
            surface.replace_scene(disclosure_flight_scene(SceneDisclosure::Closed), surface_cx);
        });
    });
    settle(cx);
    cx.update(|_, app| {
        app.set_reduce_motion(true);
    });
    settle(cx);
    assert_eq!(
        cx.debug_bounds(PANEL).expect("panel paints").size.height,
        px(0.0),
        "a reduced-motion switch settles the in-flight close"
    );
    cx.update(|_, app| {
        app.set_reduce_motion(false);
    });
    pump_animation_frame_after(cx, Duration::from_millis(60));
    assert_eq!(
        cx.debug_bounds(PANEL).expect("panel paints").size.height,
        px(0.0),
        "a settled reduced-motion close never replays"
    );
}

#[test]
fn legacy_group_details_render_in_durable_order() {
    // Legacy positional groups (no provenance) keep vec order with
    // reasoning stripped but its positional slot retained, so surviving
    // rows keep their original ordinals; this holds under both scene
    // generations because session mode never carries these inputs.
    let scene = ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        vec![
            item(
                "work-a",
                1,
                SceneItemKind::Activity {
                    body: "first".to_owned(),
                    kind: None,
                    detail: None,
                },
                None,
            ),
            item(
                "work-r",
                2,
                SceneItemKind::ReasoningSummary {
                    body: "hidden thought.".to_owned(),
                },
                None,
            ),
            item(
                "work-b",
                3,
                SceneItemKind::Activity {
                    body: "second".to_owned(),
                    kind: None,
                    detail: None,
                },
                None,
            ),
        ],
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::Working,
        )],
        Vec::new(),
    )
    .expect("conversation scene is valid");
    let turn = scene.turn_scene(&turn_id("turn_a")).expect("turn present");
    let group = turn
        .blocks()
        .iter()
        .find_map(|block| match block {
            TurnBlock::WorkGroup(group) => Some(group),
            _ => None,
        })
        .expect("work group present");
    let rows = ordered_detail_rows(group);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 0);
    assert!(matches!(rows[0].1, DetailRow::Activity { .. }));
    assert_eq!(rows[1].0, 2);
    assert!(matches!(rows[1].1, DetailRow::Activity { .. }));
}

#[test]
fn session_details_sort_by_stable_ordinal() {
    use crate::conversation_scene::{ProgressPhase, SessionDetail};
    let group = WorkGroupBlock {
        items: Vec::new(),
        label: None,
        disclosure: None,
        session: Some(scene_id("session-turn_a")),
        session_run: None,
        superseded: false,
        reasoning_summary: None,
        progress: ProgressPhase::Work,
        transition: None,
        session_details: vec![
            SessionDetail::Activity {
                id: scene_id("act-2"),
                body: "second".to_owned(),
                kind: None,
                detail: None,
                lifecycle: None,
                ordinal: 4,
                disclosure: None,
            },
            SessionDetail::Assistant {
                id: scene_id("asst-1"),
                body: "first".to_owned(),
                phase: AssistantPhase::Commentary,
                ordinal: 2,
                provenance: None,
                disclosure: None,
            },
            SessionDetail::NativeFact {
                id: scene_id("fact-3"),
                text: "third".to_owned(),
                ordinal: 6,
                disclosure: None,
            },
        ],
    };
    let rows = ordered_detail_rows(&group);
    assert_eq!(
        rows.iter().map(|(ordinal, _)| *ordinal).collect::<Vec<_>>(),
        vec![2, 4, 6]
    );
    assert!(matches!(rows[0].1, DetailRow::Assistant { .. }));
    assert!(matches!(rows[1].1, DetailRow::Activity { .. }));
    assert!(matches!(rows[2].1, DetailRow::NativeFact { .. }));
}

#[test]
fn footer_paints_only_with_settlement() {
    let without = TurnFooterBlock {
        turn_id: turn_id("turn_a"),
        settlement: None,
    };
    assert!(!footer_has_content(&without));
    assert!(footer_settlement(&without).is_none());
}

#[test]
fn footer_keys_are_stable_per_turn() {
    assert_eq!(footer_key(&turn_id("turn_a")), "turn-footer:turn_a");
    assert_ne!(
        footer_key(&turn_id("turn_a")),
        footer_key(&turn_id("turn_b"))
    );
}

#[test]
fn footer_hover_reveal_survives_the_trip_from_the_turn() {
    let mut revealed = None;
    // The pointer arrives over the footer after the turn group's own hover
    // has already dropped.
    assert!(footer_hover_transition(
        &mut revealed,
        "turn-footer:turn_a",
        true
    ));
    assert_eq!(revealed.as_deref(), Some("turn-footer:turn_a"));
    // A repeat observation is not a change.
    assert!(!footer_hover_transition(
        &mut revealed,
        "turn-footer:turn_a",
        true
    ));
    // Another turn's footer leaving must not clear the retained key.
    assert!(!footer_hover_transition(
        &mut revealed,
        "turn-footer:turn_b",
        false
    ));
    assert_eq!(revealed.as_deref(), Some("turn-footer:turn_a"));
    // Leaving the footer releases it.
    assert!(footer_hover_transition(
        &mut revealed,
        "turn-footer:turn_a",
        false
    ));
    assert!(revealed.is_none());
}

#[test]
fn scene_block_order_is_preserved_with_conditional_paint() {
    let scene = ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Completed,
        )],
        vec![
            item(
                "user-a",
                1,
                SceneItemKind::UserMessage {
                    body: "hi".to_owned(),
                },
                None,
            ),
            item(
                "assistant-a",
                2,
                SceneItemKind::AssistantMessage {
                    body: "hello".to_owned(),
                    phase: AssistantPhase::Final,
                },
                None,
            ),
        ],
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::Working,
        )],
        Vec::new(),
    )
    .expect("conversation scene is valid");
    assert_eq!(
        ordered_block_kinds(&scene),
        vec![
            RenderedBlockKind::UserMessage,
            RenderedBlockKind::AssistantMessage,
            RenderedBlockKind::TurnStatus,
            RenderedBlockKind::TurnFooter,
        ]
    );
}

#[test]
fn present_shell_command_recovers_the_wrapped_command() {
    // WindowsApps pwsh with `-Command` and a quoted inner script.
    assert_eq!(
        present_shell_command(
            r#""C:\Program Files\WindowsApps\Microsoft.PowerShell\pwsh.exe" -Command "cargo test --locked""#,
        ),
        "cargo test --locked"
    );
    // PowerShell's short flag, single-quoted body.
    assert_eq!(
        present_shell_command(r"pwsh -c 'git status --short'"),
        "git status --short"
    );
    // bash -lc.
    assert_eq!(
        present_shell_command(r#"bash -lc "cargo check -p artisan-frontend""#),
        "cargo check -p artisan-frontend"
    );
    // cmd /c and /k read case-insensitively with `.exe` stripped.
    assert_eq!(present_shell_command(r#"cmd.exe /C "dir /b""#), "dir /b");
    assert_eq!(
        present_shell_command(r"C:\Windows\System32\cmd.exe /K ver"),
        "ver"
    );
}

#[test]
fn present_shell_command_keeps_what_it_cannot_recognise() {
    // Unrecognised input is returned collapsed, never guessed at.
    assert_eq!(
        present_shell_command("cargo   test\n--locked"),
        "cargo test --locked"
    );
    assert_eq!(
        present_shell_command(r#""C:\tools\runner.exe" --work"#),
        r#""C:\tools\runner.exe" --work"#
    );
    // A recognised wrapper with a different flag stays whole.
    assert_eq!(
        present_shell_command("bash --login -c script.sh"),
        "bash --login -c script.sh"
    );
    // An empty quoted body falls back to the collapsed invocation.
    assert_eq!(
        present_shell_command(r#"pwsh -Command """#),
        r#"pwsh -Command """#
    );
    // An unterminated quote still yields the wrapper's body, exactly
    // like the reference unquote (which only drops a matching pair).
    assert_eq!(
        present_shell_command(r#"pwsh -Command "unterminated"#),
        "\"unterminated"
    );
}

#[test]
fn activity_chain_clauses_describe_composition() {
    assert_eq!(activity_chain_clause(["bash"].into_iter()), "Ran a command");
    assert_eq!(
        activity_chain_clause(["read", "read"].into_iter()),
        "Read 2 files"
    );
    assert_eq!(
        activity_chain_clause(["bash", "read", "read"].into_iter()),
        "Ran a command, read 2 files"
    );
    // A kind-less legacy row counts as generic tool work.
    assert_eq!(activity_chain_clause(["tool"].into_iter()), "Used a tool");
    assert_eq!(activity_chain_clause(std::iter::empty()), "");
}

#[test]
fn activity_chain_icons_follow_the_reference_category_map() {
    assert_eq!(
        activity_chain_icon(["bash"].into_iter()),
        AssetId::TABLER_TERMINAL_2
    );
    assert_eq!(
        activity_chain_icon(["read"].into_iter()),
        AssetId::TABLER_FILE_TEXT
    );
    assert_eq!(
        activity_chain_icon(["edit"].into_iter()),
        AssetId::TABLER_FILE_PENCIL
    );
    assert_eq!(
        activity_chain_icon(["file.delete"].into_iter()),
        AssetId::TABLER_FILE_X
    );
    assert_eq!(
        activity_chain_icon(["grep"].into_iter()),
        AssetId::TABLER_FILE_SEARCH
    );
    assert_eq!(
        activity_chain_icon(["search"].into_iter()),
        AssetId::TABLER_WORLD_SEARCH
    );
    assert_eq!(
        activity_chain_icon(["mcp"].into_iter()),
        AssetId::TABLER_TOOL
    );
    assert_eq!(
        activity_chain_icon(["bash", "read"].into_iter()),
        AssetId::TABLER_LIST_DETAILS
    );
    // Two distinct kinds sharing one category stay homogeneous.
    assert_eq!(
        activity_chain_icon(["read", "file"].into_iter()),
        AssetId::TABLER_FILE_TEXT
    );
}

#[test]
fn activity_detail_text_normalizes_terminal_and_falls_back_to_labels() {
    assert_eq!(
        activity_detail_text(
            "terminal_activity",
            Some(r#""C:\Program Files\WindowsApps\pwsh.exe" -Command "cargo test""#),
            None,
        ),
        "cargo test"
    );
    assert_eq!(
        activity_detail_text(
            "read",
            Some("src/main.rs"),
            Some(ConversationLifecycle::Completed)
        ),
        "src/main.rs"
    );
    assert_eq!(
        activity_detail_text("read", None, Some(ConversationLifecycle::Completed)),
        "Read a file"
    );
    assert_eq!(
        activity_detail_text("read", None, Some(ConversationLifecycle::Active)),
        "Reading a file"
    );
    assert_eq!(
        activity_detail_text(
            "terminal_activity",
            None,
            Some(ConversationLifecycle::Failed)
        ),
        "Command failed"
    );
    // A terminal detail that normalizes to nothing falls back.
    assert_eq!(
        activity_detail_text(
            "terminal_activity",
            Some("   "),
            Some(ConversationLifecycle::Completed),
        ),
        "Ran a command"
    );
}

#[gpui::test]
fn work_group_header_row_wraps_the_disclosure_trigger(cx: &mut TestAppContext) {
    const HEADER: &str = "artisan-conversation-surface-turn-turn_a-block-work-work-first-header";
    const TRIGGER: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-first-disclosure-trigger";
    let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scroll_target_scene(SceneDisclosure::Open),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    let header = cx
        .debug_bounds(HEADER)
        .expect("controlled header row must paint");
    let trigger = cx
        .debug_bounds(TRIGGER)
        .expect("disclosure trigger must paint");
    assert!(
        trigger.origin.y >= header.origin.y
            && trigger.origin.y + trigger.size.height <= header.origin.y + header.size.height,
        "the trigger lives inside the shared header row"
    );
}

#[gpui::test]
fn work_group_header_row_paints_without_a_disclosure_wrapper(cx: &mut TestAppContext) {
    const HEADER: &str = "artisan-conversation-surface-turn-turn_a-block-work-plain-first-header";
    let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(vec![
                item(
                    "plain-first",
                    1,
                    SceneItemKind::Activity {
                        body: body(),
                        kind: None,
                        detail: None,
                    },
                    None,
                ),
                item(
                    "plain-target",
                    2,
                    SceneItemKind::Activity {
                        body: body(),
                        kind: None,
                        detail: None,
                    },
                    None,
                ),
            ]),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    assert!(
        cx.debug_bounds(HEADER).is_some(),
        "the plain header row paints without a disclosure wrapper"
    );
}

#[gpui::test]
fn reasoning_only_group_has_no_working_disclosure(cx: &mut TestAppContext) {
    // Disclosure is registered but the group holds no visible trace
    // content: reasoning is stripped from visible details. The disabled
    // wrapper keeps stable ancestry (its selector still paints), but no
    // chevron control exists and neither click nor Enter may emit a
    // toggle — this is the no-empty-collapse requirement, evidenced by
    // behavior rather than selector absence.
    const TRIGGER: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-thinking-only-disclosure-trigger";
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            scene(vec![item(
                "thinking-only",
                1,
                SceneItemKind::ReasoningSummary {
                    body: "stripped".to_owned(),
                },
                Some(SceneDisclosure::Open),
            )]),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    let trigger = cx
        .debug_bounds(TRIGGER)
        .expect("disabled wrapper keeps stable ancestry");
    cx.simulate_click(trigger.center(), Modifiers::none());
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.update(|_, app| {
        let toggles: Vec<ConversationSurfaceAction> =
            surface.update(app, |surface, _| surface.take_actions());
        assert!(
            toggles.iter().all(|action| !matches!(
                action,
                ConversationSurfaceAction::DisclosureToggleRequested { .. }
            )),
            "an empty group must never emit a disclosure toggle, got {toggles:?}"
        );
    });
}

#[gpui::test]
fn provider_wait_paints_without_group_or_disclosure(cx: &mut TestAppContext) {
    // Pre-response state: user plus wait narration, no assistant and no
    // work group. The status row still narrates the wait, and with no
    // group there is no header, trigger, or fabricated detail anywhere.
    const STATUS: &str = "artisan-conversation-surface-turn-turn_a-status";
    const NO_GROUP_TRIGGER: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-turn_a-disclosure-trigger";
    let scene = ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        vec![item(
            "user-a",
            1,
            SceneItemKind::UserMessage {
                body: "hi".to_owned(),
            },
            None,
        )],
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::ProviderWait,
        )],
        Vec::new(),
    )
    .expect("waiting scene is valid");
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(scene, ThemeMode::Dark, surface_cx)
    });
    cx.simulate_resize(size(px(720.0), px(240.0)));
    settle(cx);
    assert!(
        cx.debug_bounds(STATUS).is_some(),
        "the wait line narrates before any work exists"
    );
    assert!(
        cx.debug_bounds(NO_GROUP_TRIGGER).is_none(),
        "no disclosure control without a work group"
    );
    cx.update(|_, app| {
        assert!(
            !ordered_block_kinds(surface.read(app).scene()).contains(&RenderedBlockKind::WorkGroup),
            "no fabricated work detail"
        );
    });
}

#[test]
fn copy_confirmation_enters_holds_and_returns_with_reduced_motion_endpoints() {
    let progress = |ms, motion| copy_feedback_progress(Duration::from_millis(ms), motion);
    assert_eq!(progress(0, MotionPolicy::Full), 0.0);
    assert!((progress(125, MotionPolicy::Full) - 0.5).abs() < 0.01);
    assert_eq!(progress(250, MotionPolicy::Full), 1.0);
    assert_eq!(progress(1400, MotionPolicy::Full), 1.0);
    assert!((progress(1625, MotionPolicy::Full) - 0.5).abs() < 0.01);
    assert_eq!(progress(1750, MotionPolicy::Full), 0.0);
    assert_eq!(progress(0, MotionPolicy::Reduced), 1.0);
    assert_eq!(progress(1500, MotionPolicy::Reduced), 0.0);
}
