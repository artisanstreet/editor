//! Conservative request-throughput estimates from durable run usage.
//! UI chunk arrival times and character-based token guesses are never used.

use crate::conversation_scene::{ConversationScene, TurnBlock};
use artisan_domain::{
    ConversationItem, ConversationLifecycle, ConversationSnapshot, ReadRunUsage, RunUsageBasis,
    RunUsageReport, TurnId,
};

/// Only a completed, single-response turn without work or waiting is eligible.
pub(crate) fn footer_usage_query(
    snapshot: &ConversationSnapshot,
    scene: &ConversationScene,
    turn_id: &TurnId,
) -> Option<ReadRunUsage> {
    let turn = snapshot
        .turns()
        .iter()
        .find(|turn| &turn.turn_id == turn_id)?;
    if turn.lifecycle != ConversationLifecycle::Completed
        || turn
            .updated_at
            .as_millis()
            .saturating_sub(turn.created_at.as_millis())
            < 5_000
    {
        return None;
    }
    if scene.turn_scene(turn_id)?.blocks().iter().any(|block| {
        !matches!(
            block,
            TurnBlock::UserMessage(_)
                | TurnBlock::AssistantMessage(_)
                | TurnBlock::TurnStatus(_)
                | TurnBlock::TurnFooter(_)
        )
    }) {
        return None;
    }
    let mut messages = snapshot.items().iter().filter_map(|item| match item {
        ConversationItem::AssistantMessage(message) if &message.turn_id == turn_id => Some(message),
        _ => None,
    });
    let message = messages.next()?;
    if messages.next().is_some() || message.lifecycle != ConversationLifecycle::Completed {
        return None;
    }
    Some(ReadRunUsage::new(
        snapshot.thread_id().clone(),
        message.run_id.clone(),
    ))
}

/// Includes request setup, prefill and waiting for the first token. This is an
/// estimate of request throughput, deliberately not a claim about decode speed.
pub(crate) fn footer_speed(
    snapshot: &ConversationSnapshot,
    scene: &ConversationScene,
    turn_id: &TurnId,
    report: &RunUsageReport,
) -> Option<String> {
    let query = footer_usage_query(snapshot, scene, turn_id)?;
    if report.thread_id() != &query.thread_id || report.run_id() != &query.run_id {
        return None;
    }
    // Cumulative counters may include earlier requests in the provider session.
    // Without a durable baseline, even plausible-looking totals are unsafe.
    if report.basis() != RunUsageBasis::Delta {
        return None;
    }
    let turn = snapshot
        .turns()
        .iter()
        .find(|turn| &turn.turn_id == turn_id)?;
    let started = turn.created_at.as_millis();
    let ended = turn.updated_at.as_millis();
    if report.observed_at().as_millis() < started || report.observed_at().as_millis() > ended {
        return None;
    }
    format_speed(report.output_tokens()?, ended.checked_sub(started)?)
}

fn format_speed(tokens: u64, elapsed_ms: i64) -> Option<String> {
    // Require at least 128 reported tokens and five seconds. Tiny or coarsely
    // timestamped responses do not carry enough evidence for a useful rate.
    if tokens < 128 || elapsed_ms < 5_000 {
        return None;
    }
    // Bound before conversion, avoiding precision loss on corrupt huge totals.
    let tokens = u32::try_from(tokens).ok()?;
    let elapsed_ms = u32::try_from(elapsed_ms).ok()?;
    Some(format!(
        "{:.1} tok/s",
        f64::from(tokens) * 1000.0 / f64::from(elapsed_ms)
    ))
}

#[cfg(test)]
mod tests {
    use super::format_speed;

    #[test]
    fn short_or_invalid_samples_have_no_rate() {
        for (tokens, elapsed) in [
            (10, 50),
            (127, 10_000),
            (256, 4_999),
            (256, 0),
            (256, -1),
            (u64::MAX, 10_000),
        ] {
            assert_eq!(format_speed(tokens, elapsed), None);
        }
    }

    #[test]
    fn request_duration_formats_one_decimal_without_a_speed_cap() {
        assert_eq!(format_speed(512, 10_000).as_deref(), Some("51.2 tok/s"));
        assert_eq!(format_speed(1000, 5_000).as_deref(), Some("200.0 tok/s"));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::conversation_scene::{SceneTurn, TurnNarration, TurnNarrationEntry};
    use artisan_domain::*;

    fn fixture() -> (ConversationSnapshot, ConversationScene, RunUsageReportInput) {
        let thread = ThreadId::parse("thread").unwrap();
        let turn = TurnId::parse("turn").unwrap();
        let run = RunId::parse("run").unwrap();
        let snapshot = ConversationSnapshot::new(
            thread.clone(),
            ConversationCursor::new(0),
            vec![ConversationTurn {
                turn_id: turn.clone(),
                ordinal: TurnOrdinal::new(0),
                revision: Revision::new(0),
                lifecycle: ConversationLifecycle::Completed,
                created_at: UnixMillis::from_millis(1000),
                updated_at: UnixMillis::from_millis(11000),
            }],
            vec![ConversationItem::AssistantMessage(AssistantMessageItem {
                item_id: ItemId::parse("reply").unwrap(),
                turn_id: turn.clone(),
                run_id: run.clone(),
                ordinal: ItemOrdinal::new(1),
                revision: Revision::new(0),
                lifecycle: ConversationLifecycle::Completed,
                body: AssistantBody::parse("reply").unwrap(),
                phase: AssistantMessagePhase::Final,
                created_at: UnixMillis::from_millis(2000),
                updated_at: UnixMillis::from_millis(11000),
            })],
            UnixMillis::from_millis(11000),
        )
        .unwrap();
        let scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn.clone(),
                0,
                ConversationLifecycle::Completed,
            )],
            vec![],
            vec![TurnNarrationEntry::new(turn, TurnNarration::Quiet)],
            vec![],
        )
        .unwrap();
        let usage = RunUsageReportInput {
            run_id: run,
            thread_id: thread,
            provider_session_id: "session".into(),
            source_sequence: 1,
            model_id: EngineModelId::parse("model").unwrap(),
            provider_route_id: EngineRouteId::parse("route").unwrap(),
            variant_id: None,
            basis: RunUsageBasis::Delta,
            provider_turn_id: None,
            input_tokens: None,
            cached_input_tokens: None,
            output_tokens: Some(512),
            context_tokens: None,
            context_window_tokens: None,
            observed_at: UnixMillis::from_millis(10500),
        };
        (snapshot, scene, usage)
    }

    #[test]
    fn activity_and_multiple_replies_suppress_the_estimate() {
        use crate::conversation_scene::{SceneId, SceneItem, SceneItemKind};
        let (snapshot, scene, input) = fixture();
        let turn = TurnId::parse("turn").unwrap();
        let busy = ConversationScene::build(
            vec![SceneTurn::new(
                turn.clone(),
                0,
                ConversationLifecycle::Completed,
            )],
            vec![
                SceneItem::new(
                    SceneId::parse("tool").unwrap(),
                    turn.clone(),
                    1,
                    SceneItemKind::Activity {
                        body: "tool".into(),
                        kind: None,
                        detail: None,
                    },
                    None,
                )
                .unwrap(),
            ],
            vec![TurnNarrationEntry::new(turn.clone(), TurnNarration::Quiet)],
            vec![],
        )
        .unwrap();
        assert!(footer_usage_query(&snapshot, &busy, &turn).is_none());
        let mut items = snapshot.items().to_vec();
        let ConversationItem::AssistantMessage(mut second) = items[0].clone() else {
            unreachable!()
        };
        second.item_id = ItemId::parse("second").unwrap();
        second.ordinal = ItemOrdinal::new(2);
        items.push(ConversationItem::AssistantMessage(second));
        let multiple = ConversationSnapshot::new(
            snapshot.thread_id().clone(),
            snapshot.cursor(),
            snapshot.turns().to_vec(),
            items,
            snapshot.updated_at(),
        )
        .unwrap();
        assert!(
            footer_speed(
                &multiple,
                &scene,
                &turn,
                &RunUsageReport::new(input).unwrap()
            )
            .is_none()
        );
    }

    #[test]
    fn estimates_only_matching_delta_counts_inside_durable_turn_interval() {
        let (snapshot, scene, input) = fixture();
        let turn = TurnId::parse("turn").unwrap();
        assert_eq!(
            footer_speed(
                &snapshot,
                &scene,
                &turn,
                &RunUsageReport::new(input.clone()).unwrap()
            )
            .as_deref(),
            Some("51.2 tok/s")
        );
        for basis in [RunUsageBasis::Cumulative, RunUsageBasis::Unknown] {
            let mut bad = input.clone();
            bad.basis = basis;
            assert_eq!(
                footer_speed(&snapshot, &scene, &turn, &RunUsageReport::new(bad).unwrap()),
                None
            );
        }
        for at in [999, 11001] {
            let mut bad = input.clone();
            bad.observed_at = UnixMillis::from_millis(at);
            assert_eq!(
                footer_speed(&snapshot, &scene, &turn, &RunUsageReport::new(bad).unwrap()),
                None
            );
        }
        let mut wrong = input;
        wrong.run_id = RunId::parse("other").unwrap();
        assert_eq!(
            footer_speed(
                &snapshot,
                &scene,
                &turn,
                &RunUsageReport::new(wrong).unwrap()
            ),
            None
        );
    }
}
