//! Footer access to a backend-measured visible-text streaming estimate.
use crate::conversation_scene::ConversationScene;
use artisan_domain::{
    ConversationItem, ConversationLifecycle, ConversationSnapshot, ReadRunUsage, RunUsageReport,
    TurnId,
};

pub(crate) fn footer_usage_query(
    snapshot: &ConversationSnapshot,
    _scene: &ConversationScene,
    turn_id: &TurnId,
) -> Option<ReadRunUsage> {
    let turn = snapshot
        .turns()
        .iter()
        .find(|turn| &turn.turn_id == turn_id)?;
    if turn.lifecycle != ConversationLifecycle::Completed {
        return None;
    }
    let message = snapshot.items().iter().find_map(|item| match item {
        ConversationItem::AssistantMessage(message) if &message.turn_id == turn_id => Some(message),
        _ => None,
    })?;
    Some(ReadRunUsage::new(
        snapshot.thread_id().clone(),
        message.run_id.clone(),
    ))
}

pub(crate) fn footer_speed(
    snapshot: &ConversationSnapshot,
    scene: &ConversationScene,
    turn_id: &TurnId,
    report: &RunUsageReport,
) -> Option<String> {
    let query = footer_usage_query(snapshot, scene, turn_id)?;
    if report.run_id() != &query.run_id || report.thread_id() != &query.thread_id {
        return None;
    }
    let rate = f64::from(report.streaming_millitokens_per_second()?) / 1000.0;
    Some(format!("{rate:.1} tok/s"))
}
