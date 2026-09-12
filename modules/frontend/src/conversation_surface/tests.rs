#![expect(
    clippy::float_cmp,
    reason = "test assertions compare the exact pixel arithmetic the UI performs; an epsilon would weaken the regression coverage"
)]
use super::*;
use artisan_domain::{ConversationLifecycle, ItemId, TurnId};
use artisan_ui::theme::ThemeMode;
use gpui::{
    Entity, Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase,
    VisualTestContext, point, px, size,
};

use crate::conversation_scene::{
    AssistantPhase, ConversationScene, ItemProvenance, SceneDisclosure, SceneItem, SceneItemKind,
    SceneTurn, TurnFooterBlock, TurnNarration, TurnNarrationEntry,
};

fn scene_id(value: &str) -> SceneId {
    SceneId::parse(value).expect("scene id is valid")
}

fn turn_id(value: &str) -> TurnId {
    TurnId::parse(value).expect("turn id is valid")
}

fn item(
    id: &str,
    ordinal: u64,
    kind: SceneItemKind,
    disclosure: Option<SceneDisclosure>,
) -> SceneItem {
    SceneItem::new(scene_id(id), turn_id("turn_a"), ordinal, kind, disclosure)
        .expect("scene item is valid")
}

fn body() -> String {
    (0..12)
        .map(|line| format!("Transcript line {line} keeps the viewport measurable."))
        .collect::<Vec<_>>()
        .join("\n")
}

fn scene(items: Vec<SceneItem>) -> ConversationScene {
    ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Completed,
        )],
        items,
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::Quiet,
        )],
        Vec::new(),
    )
    .expect("conversation scene is valid")
}

fn scroll_target_scene(disclosure: SceneDisclosure) -> ConversationScene {
    let mut items = (0..8)
        .map(|index| {
            item(
                &format!("user-{index}"),
                index + 1,
                SceneItemKind::UserMessage { body: body() },
                None,
            )
        })
        .collect::<Vec<_>>();
    items.extend([
        item(
            "work-first",
            9,
            SceneItemKind::ReasoningSummary { body: body() },
            Some(disclosure),
        ),
        item(
            "work-target",
            10,
            SceneItemKind::Activity {
                body: body(),
                kind: None,
                detail: None,
            },
            Some(disclosure),
        )
        .with_provenance(ItemProvenance {
            run_id: None,
            lifecycle: Some(ConversationLifecycle::Active),
        }),
    ]);
    scene(items)
}

fn settle(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.run_until_parked();
}

fn offset(
    surface: &Entity<ConversationSurface>,
    cx: &mut VisualTestContext,
) -> gpui::Point<gpui::Pixels> {
    cx.update(|_, app| surface.read(app).scroll_handle().offset())
}

/// Builds one work group holding a single kinded terminal activity.
fn activity_chain_scene(
    turn_lifecycle: ConversationLifecycle,
    narration: TurnNarration,
    disclosure: SceneDisclosure,
) -> ConversationScene {
    ConversationScene::build(
        vec![SceneTurn::new(turn_id("turn_a"), 0, turn_lifecycle)],
        vec![item(
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
            Some(disclosure),
        )],
        vec![TurnNarrationEntry::new(turn_id("turn_a"), narration)],
        Vec::new(),
    )
    .expect("activity chain scene is valid")
}

/// Builds one live work-group scene whose disclosure is `disclosure`.
fn disclosure_flight_scene(disclosure: SceneDisclosure) -> ConversationScene {
    ConversationScene::build(
        vec![SceneTurn::new(
            turn_id("turn_a"),
            0,
            ConversationLifecycle::Active,
        )],
        vec![item(
            "work-a",
            1,
            SceneItemKind::Activity {
                body: body(),
                kind: None,
                detail: None,
            },
            Some(disclosure),
        )],
        vec![TurnNarrationEntry::new(
            turn_id("turn_a"),
            TurnNarration::Working,
        )],
        Vec::new(),
    )
    .expect("disclosure flight scene is valid")
}

/// Waits out real motion time, then delivers the pending animation frame.
///
/// GPUI's element animation clock is the wall clock, which the test
/// harness cannot advance through its dispatcher. A real wait guarantees
/// at least that much elapsed motion before the next frame samples it.
fn pump_animation_frame_after(cx: &mut VisualTestContext, wait: Duration) {
    std::thread::sleep(wait);
    cx.update(|window, app| {
        window.simulate_next_frame(app);
    });
    settle(cx);
}

fn tall_navigator_scene() -> ConversationScene {
    // Turn ordinals (0, 1) share the global ordinal namespace with
    // items, so item ordinals continue after them.
    let long_body = (0..40)
        .map(|line| format!("Navigator line {line} fills the viewport for tracking."))
        .collect::<Vec<_>>()
        .join("\n");
    ConversationScene::build(
        vec![
            SceneTurn::new(turn_id("turn_a"), 0, ConversationLifecycle::Completed),
            SceneTurn::new(turn_id("turn_b"), 1, ConversationLifecycle::Completed),
        ],
        vec![
            SceneItem::new(
                scene_id("nav-first"),
                turn_id("turn_a"),
                2,
                SceneItemKind::UserMessage {
                    body: long_body.clone(),
                },
                None,
            )
            .expect("first navigator item is valid"),
            SceneItem::new(
                scene_id("nav-first-reply"),
                turn_id("turn_a"),
                3,
                SceneItemKind::AssistantMessage {
                    body: "first reply".to_owned(),
                    phase: AssistantPhase::Final,
                },
                None,
            )
            .expect("first reply is valid"),
            SceneItem::new(
                scene_id("nav-second"),
                turn_id("turn_b"),
                4,
                SceneItemKind::UserMessage { body: long_body },
                None,
            )
            .expect("second navigator item is valid"),
            SceneItem::new(
                scene_id("nav-second-reply"),
                turn_id("turn_b"),
                5,
                SceneItemKind::AssistantMessage {
                    body: "second reply".to_owned(),
                    phase: AssistantPhase::Final,
                },
                None,
            )
            .expect("second reply is valid"),
        ],
        vec![
            TurnNarrationEntry::new(turn_id("turn_a"), TurnNarration::Quiet),
            TurnNarrationEntry::new(turn_id("turn_b"), TurnNarration::Quiet),
        ],
        Vec::new(),
    )
    .expect("tall navigator scene is valid")
}

#[path = "tests/approve_submit.rs"]
mod approve_submit;

#[path = "tests/answer_receipts.rs"]
mod answer_receipts;

#[path = "tests/status.rs"]
mod status;

#[path = "tests/transcript.rs"]
mod transcript;

#[path = "tests/windowing.rs"]
mod windowing;

#[path = "tests/work_groups.rs"]
mod work_groups;
