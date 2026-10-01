use super::*;

#[gpui::test]
fn a_group_header_is_never_a_bare_chevron_live_or_settled(cx: &mut TestAppContext) {
    // A live group without a status line reads the turn's own elapsed line
    // with no chevron at all (a live section cannot collapse), and a turn
    // that ends failed titles its group `Failed` beside its chevron: the
    // header always carries words, never a bare chevron.
    const HEADER: &str = "artisan-conversation-surface-turn-turn_a-block-work-work-a-header";
    const LABEL: &str = "artisan-conversation-surface-turn-turn_a-block-work-work-a-header-label";
    const CHEVRON: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-a-header-chevron";
    // The 16 px chevron plus the 4 px gap: anything wider carries words.
    const CHEVRON_ONLY: f32 = 20.0;
    // `StreamingSuppression` is live work that carries no live header copy of
    // its own; `Failed` settles with no terminal label.
    let (_live, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            activity_chain_scene(
                ConversationLifecycle::Active,
                TurnNarration::StreamingSuppression,
                SceneDisclosure::Open,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    let live = cx
        .debug_bounds(LABEL)
        .expect("the live chain header paints its words")
        .size
        .width;
    assert!(
        live > px(CHEVRON_ONLY),
        "the live header is titled: {live:?}"
    );
    assert!(
        cx.debug_bounds(CHEVRON).is_none(),
        "a live header carries no chevron"
    );
    let (_settled, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            activity_chain_scene(
                ConversationLifecycle::Failed,
                TurnNarration::Failed,
                SceneDisclosure::Open,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    // The failed turn's outcome titles the header, and the separate red
    // status row stands down behind it.
    let settled = cx
        .debug_bounds(HEADER)
        .expect("the settled chain header paints")
        .size
        .width;
    assert!(
        settled > px(CHEVRON_ONLY),
        "the settled header is titled: {settled:?}"
    );
    assert!(
        cx.debug_bounds(CHEVRON).is_some(),
        "a settled header carries its chevron beside the words"
    );
}

struct HeaderProbe {
    label: String,
    controlled: bool,
}

impl Render for HeaderProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .child(ConversationSurface::work_group_header_row(
                Some(self.label.clone()),
                None,
                0.0,
                self.controlled,
                "probe",
                MotionPolicy::Reduced,
                false,
                &ArtisanTheme::for_mode(ThemeMode::Dark),
            ))
    }
}

#[gpui::test]
fn a_long_header_label_ellipsizes_inside_the_header(cx: &mut TestAppContext) {
    // A Claude thinking label wider than the column stays one line inside
    // it, ending in an ellipsis, instead of running past the edge.
    let label = "I'm tracing why the composer keeps the stop glyph while a draft is typed \
                 mid-run and checking every steer path for the closed-stdin race · 13s"
        .to_owned();
    for controlled in [true, false] {
        let (_probe, cx) = cx.add_window_view(|_, _| HeaderProbe {
            label: label.clone(),
            controlled,
        });
        cx.simulate_resize(size(px(360.0), px(200.0)));
        settle(cx);
        let header = cx.debug_bounds("probe-header").expect("the header paints");
        assert!(header.right() <= px(360.0), "{header:?}");
        // One 24 px line plus the 8 px divider seam beneath it.
        assert!(
            header.size.height <= px(32.0),
            "the label wrapped: {header:?}"
        );
    }
}

#[gpui::test]
fn an_unlabeled_header_keeps_the_row_seam_above_its_first_row(cx: &mut TestAppContext) {
    // The header divider and the first row keep the 8 px seam rows keep
    // between each other, even when the header carries no label.
    const HEADER: &str = "artisan-conversation-surface-turn-turn_a-block-work-work-a-header";
    const CHAIN: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-work-a-trace-0-trigger";
    let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            activity_chain_scene(
                ConversationLifecycle::Active,
                TurnNarration::StreamingSuppression,
                SceneDisclosure::Open,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(480.0)));
    settle(cx);
    let header = cx
        .debug_bounds(HEADER)
        .expect("the unlabeled header paints");
    let chain = cx.debug_bounds(CHAIN).expect("the first chain paints");
    assert_eq!(chain.top() - header.bottom(), px(8.0));
}

const STEER_LEAD_HEADER: &str = "artisan-conversation-surface-turn-turn_a-block-work-turn_a-header";
const STEER_LEAD_CHEVRON: &str =
    "artisan-conversation-surface-turn-turn_a-block-work-turn_a-header-chevron";
const STEER_LEAD_CHAIN: &str =
    "artisan-conversation-surface-turn-turn_a-block-work-turn_a-trace-0-trigger";
const STEER_LEAD_TRIGGER: &str =
    "artisan-conversation-surface-turn-turn_a-block-work-turn_a-disclosure-trigger";
const STEER_CONTINUATION_CHAIN: &str =
    "artisan-conversation-surface-turn-turn_a-block-work-session-turn_a.1-trace-0-trigger";
const STEER_CONTINUATION_HEADER: &str =
    "artisan-conversation-surface-turn-turn_a-block-work-session-turn_a.1-header";
const STEER_CONTINUATION_TRIGGER: &str =
    "artisan-conversation-surface-turn-turn_a-block-work-session-turn_a.1-disclosure-trigger";
const STEER_BUBBLE: &str = "artisan-conversation-surface-turn-turn_a-block-user-steer";

/// One run-attributed item of the steered turn.
fn run_item(id: &str, ordinal: u64, kind: SceneItemKind) -> SceneItem {
    item(id, ordinal, kind, None).with_provenance(ItemProvenance {
        run_id: Some(artisan_domain::RunId::parse("run_a").expect("run id is valid")),
        lifecycle: Some(ConversationLifecycle::Completed),
    })
}

fn run_command(id: &str, ordinal: u64) -> SceneItem {
    run_item(
        id,
        ordinal,
        SceneItemKind::Activity {
            body: "cargo test".to_owned(),
            kind: Some("terminal_activity".to_owned()),
            detail: Some("cargo test --locked".to_owned()),
        },
    )
}

fn run_prose(id: &str, ordinal: u64, body: &str) -> SceneItem {
    run_item(
        id,
        ordinal,
        SceneItemKind::AssistantMessage {
            body: body.to_owned(),
            phase: AssistantPhase::Commentary,
        },
    )
}

/// A session turn steered mid-run: commentary and a command, the steer,
/// one more command before the acknowledging prose, then one after it.
/// `extra` appends further commands after the acknowledgement.
fn steered_section_scene(
    lifecycle: ConversationLifecycle,
    narration: TurnNarration,
    disclosure: SceneDisclosure,
    extra: u64,
) -> ConversationScene {
    let mut items = vec![
        item(
            "prompt",
            1,
            SceneItemKind::UserMessage {
                body: "why is the footer wrong".to_owned(),
            },
            None,
        ),
        run_prose("likely", 2, "Likely cause found"),
        run_command("cmd-a", 3),
        item(
            "steer",
            4,
            SceneItemKind::UserMessage {
                body: "also check the tray".to_owned(),
            },
            None,
        ),
        run_command("cmd-b", 5),
        run_prose("confirmed", 6, "Footer cause confirmed"),
        run_command("cmd-c", 7),
    ];
    for index in 0..extra {
        items.push(run_command(&format!("cmd-extra-{index}"), 8 + index));
    }
    ConversationScene::build(
        vec![SceneTurn::new(turn_id("turn_a"), 0, lifecycle)],
        items,
        vec![
            TurnNarrationEntry::new(turn_id("turn_a"), narration)
                .with_session_disclosure(disclosure),
        ],
        Vec::new(),
    )
    .expect("steered section scene is valid")
}

fn toggle_requests(surface: &Entity<ConversationSurface>, cx: &mut VisualTestContext) -> usize {
    cx.update(|_, app| {
        surface
            .read(app)
            .pending_actions()
            .iter()
            .filter(|action| {
                matches!(
                    action,
                    ConversationSurfaceAction::DisclosureToggleRequested { .. }
                )
            })
            .count()
    })
}

#[gpui::test]
fn a_live_section_has_no_chevron_no_toggle_and_stays_open(cx: &mut TestAppContext) {
    // A section cannot be collapsed before its turn settles: even with a
    // stored Closed value the live header carries its words without a
    // chevron, clicking it requests nothing, and both segments show rows.
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            steered_section_scene(
                ConversationLifecycle::Active,
                TurnNarration::Working,
                SceneDisclosure::Closed,
                0,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(720.0)));
    settle(cx);
    let header = cx
        .debug_bounds(STEER_LEAD_HEADER)
        .expect("the live header paints");
    assert!(
        cx.debug_bounds(STEER_LEAD_CHEVRON).is_none(),
        "a live section header carries no chevron"
    );
    let first_chain = cx
        .debug_bounds(STEER_LEAD_CHAIN)
        .expect("the forced-open first segment paints its chain");
    let continuation_chain = cx
        .debug_bounds(STEER_CONTINUATION_CHAIN)
        .expect("the forced-open continuation paints its chain");
    assert!(
        cx.debug_bounds(STEER_CONTINUATION_HEADER).is_none(),
        "a continuation paints no header of its own"
    );
    let bubble = cx
        .debug_bounds(STEER_BUBBLE)
        .expect("the steer bubble paints");
    assert!(header.bottom() <= first_chain.top());
    assert!(first_chain.bottom() <= bubble.top());
    assert!(bubble.bottom() <= continuation_chain.top());

    let trigger = cx
        .debug_bounds(STEER_LEAD_TRIGGER)
        .expect("the header wrapper stays mounted");
    cx.simulate_click(trigger.center(), Modifiers::none());
    settle(cx);
    assert_eq!(
        toggle_requests(&surface, cx),
        0,
        "a live section cannot toggle"
    );

    // The live header is owned by the first segment for the whole turn:
    // work arriving below the bubble neither moves it nor re-titles it.
    cx.update(|_, app| {
        let scene = surface.read(app).scene();
        assert_eq!(owning_group_index(&scene.turn_scenes()[0]), Some(1));
    });
    cx.update(|_, app| {
        surface.update(app, |surface, surface_cx| {
            surface.replace_scene(
                steered_section_scene(
                    ConversationLifecycle::Active,
                    TurnNarration::Working,
                    SceneDisclosure::Closed,
                    2,
                ),
                surface_cx,
            );
        });
    });
    settle(cx);
    assert_eq!(
        cx.debug_bounds(STEER_LEAD_HEADER),
        Some(header),
        "the live header stays put as work arrives"
    );
    cx.update(|_, app| {
        let scene = surface.read(app).scene();
        assert_eq!(owning_group_index(&scene.turn_scenes()[0]), Some(1));
    });
}

#[gpui::test]
fn a_collapsed_settled_section_keeps_the_steer_bubble_visible(cx: &mut TestAppContext) {
    // Settled and collapsed: the header (with its chevron) and the steer
    // bubble stay visible, while the one toggle hides the rows on both
    // sides of the bubble.
    let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            steered_section_scene(
                ConversationLifecycle::Completed,
                TurnNarration::WorkedFor { millis: 111_000 },
                SceneDisclosure::Closed,
                0,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(720.0)));
    settle(cx);
    let header = cx
        .debug_bounds(STEER_LEAD_HEADER)
        .expect("the settled header paints");
    assert!(
        cx.debug_bounds(STEER_LEAD_CHEVRON).is_some(),
        "a settled section header carries its chevron"
    );
    let bubble = cx
        .debug_bounds(STEER_BUBBLE)
        .expect("the steer bubble stays visible in a collapsed section");
    assert!(bubble.size.height > px(0.0));
    assert!(header.bottom() <= bubble.top());
    for chain in [STEER_LEAD_CHAIN, STEER_CONTINUATION_CHAIN] {
        assert!(
            cx.debug_bounds(chain).is_none(),
            "{chain} is hidden by the collapsed section"
        );
    }
}

#[gpui::test]
fn a_settled_section_toggles_through_its_first_segment_only(cx: &mut TestAppContext) {
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            steered_section_scene(
                ConversationLifecycle::Completed,
                TurnNarration::WorkedFor { millis: 111_000 },
                SceneDisclosure::Open,
                0,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(720.0)));
    settle(cx);
    assert!(
        cx.debug_bounds(STEER_CONTINUATION_CHAIN).is_some(),
        "an open settled section shows the continuation rows"
    );
    let trigger = cx
        .debug_bounds(STEER_LEAD_TRIGGER)
        .expect("the settled trigger paints");
    cx.simulate_click(trigger.center(), Modifiers::none());
    settle(cx);
    cx.update(|_, app| {
        let toggles: Vec<(String, bool)> = surface
            .read(app)
            .pending_actions()
            .iter()
            .filter_map(|action| match action {
                ConversationSurfaceAction::DisclosureToggleRequested { id, requested_open } => {
                    Some((id.as_str().to_owned(), *requested_open))
                }
                _ => None,
            })
            .collect();
        assert_eq!(toggles, vec![("session-turn_a".to_owned(), false)]);
    });
    // The continuation's wrapper is inert.
    let continuation_trigger = cx
        .debug_bounds(STEER_CONTINUATION_TRIGGER)
        .expect("the continuation keeps its inert wrapper mounted");
    assert_eq!(continuation_trigger.size.height, px(0.0));
}

#[test]
fn a_section_is_titled_by_its_turn_when_no_label_or_live_line_reaches_it() {
    // The header names what the turn is doing or how it ended; no generic
    // title exists. A streaming reply carries no Thinking/Working line, so
    // the live header counts on the turn's own basis instead.
    let title = |lifecycle, narration: TurnNarrationEntry, now_ms| {
        let scene = ConversationScene::build(
            vec![SceneTurn::new(turn_id("turn_a"), 0, lifecycle)],
            vec![item(
                "work-a",
                1,
                SceneItemKind::Activity {
                    body: body(),
                    kind: None,
                    detail: None,
                },
                Some(SceneDisclosure::Open),
            )],
            vec![narration],
            Vec::new(),
        )
        .expect("scene is valid");
        turn_section_title(
            scene.turn_scene(&turn_id("turn_a")).expect("turn present"),
            now_ms,
        )
    };
    let entry = |narration| TurnNarrationEntry::new(turn_id("turn_a"), narration);
    assert_eq!(
        title(
            ConversationLifecycle::Active,
            entry(TurnNarration::StreamingSuppression).with_active_started_at_ms(1_000),
            Some(89_000),
        ),
        "Working for 1m 28s"
    );
    assert_eq!(
        title(
            ConversationLifecycle::Active,
            entry(TurnNarration::Quiet),
            Some(89_000),
        ),
        "Working"
    );
    // A settled turn whose narration never settled still reads its outcome.
    for (lifecycle, expected) in [
        (ConversationLifecycle::Completed, "Worked"),
        (ConversationLifecycle::Failed, "Failed"),
        (ConversationLifecycle::Interrupted, "Interrupted"),
        (ConversationLifecycle::Cancelled, "Cancelled"),
    ] {
        assert_eq!(
            title(lifecycle, entry(TurnNarration::Quiet), None),
            expected
        );
    }
}

#[gpui::test]
fn a_live_continuation_chain_toggles_without_disturbing_the_header(cx: &mut TestAppContext) {
    // The tool-call chain below a steer bubble opens like any other chain
    // while the turn is live: the click stays surface-local, requests no
    // section toggle, and leaves the section header where it was.
    const RAIL: &str =
        "artisan-conversation-surface-turn-turn_a-block-work-session-turn_a.1-trace-0-rail";
    let (surface, cx) = cx.add_window_view(|_, surface_cx| {
        ConversationSurface::new(
            steered_section_scene(
                ConversationLifecycle::Active,
                TurnNarration::Working,
                SceneDisclosure::Open,
                0,
            ),
            ThemeMode::Dark,
            surface_cx,
        )
    });
    cx.simulate_resize(size(px(720.0), px(720.0)));
    settle(cx);
    let header = cx
        .debug_bounds(STEER_LEAD_HEADER)
        .expect("the live header paints");
    assert!(
        cx.debug_bounds(RAIL).is_none(),
        "a live chain starts closed"
    );
    let chain = cx
        .debug_bounds(STEER_CONTINUATION_CHAIN)
        .expect("the continuation chain paints");
    let center = point(chain.origin.x + px(4.0), chain.origin.y + px(4.0));
    cx.simulate_mouse_down(center, gpui::MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(center, gpui::MouseButton::Left, Modifiers::default());
    settle(cx);
    assert!(
        cx.debug_bounds(RAIL).is_some(),
        "clicking the continuation chain opens its rows"
    );
    assert_eq!(toggle_requests(&surface, cx), 0);
    assert_eq!(
        cx.debug_bounds(STEER_LEAD_HEADER),
        Some(header),
        "the section header stays put"
    );
}
