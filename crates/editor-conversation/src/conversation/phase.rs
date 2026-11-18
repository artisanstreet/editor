use artisan_api::{Item, ItemKind, LiveTarget, Run, RunOutcome, RunState, Status, ToolKind};
use std::collections::HashMap;
use std::hash::BuildHasher;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    Unknown,
    Thinking,
    Working,
    Replying,
    Compacting,
    WaitingOnYou,
    Settled(RunOutcome),
}

#[must_use]
pub(crate) fn turn_phase<S: BuildHasher>(
    run: &Run,
    items: &[&Item],
    live: &HashMap<LiveTarget, std::sync::Arc<ropey::Rope>, S>,
) -> Phase {
    if let RunState::Finished { outcome, .. } = &run.state {
        return Phase::Settled(*outcome);
    }
    if items.iter().any(|item| item.status == Status::AwaitingUser) {
        return Phase::WaitingOnYou;
    }
    if items.iter().any(|item| {
        item.parent_id.is_none()
            && item.status == Status::Running
            && matches!(item.kind, ItemKind::Compaction { .. })
    }) {
        return Phase::Compacting;
    }
    let latest = items
        .iter()
        .filter(|item| item.parent_id.is_none())
        .max_by_key(|item| item.position);
    if let Some(item) = latest {
        match &item.kind {
            ItemKind::AssistantMessage { .. }
                if item.status == Status::Running
                    || live.contains_key(&LiveTarget::Text {
                        item_id: item.id.clone(),
                    }) =>
            {
                return Phase::Replying;
            }
            ItemKind::UserMessage { .. }
            | ItemKind::AssistantMessage { .. }
            | ItemKind::Tool { .. }
            | ItemKind::Agent { .. }
            | ItemKind::Question { .. }
            | ItemKind::Plan { .. }
            | ItemKind::Image { .. }
            | ItemKind::Compaction { .. }
            | ItemKind::Notice { .. }
            | ItemKind::Error { .. } => {}
        }
    }
    let running = |item: &&&Item| item.status == Status::Running;
    let foreground = items.iter().filter(running).any(|item| match &item.kind {
        ItemKind::Tool {
            tool: ToolKind::Shell { background, .. },
            ..
        }
        | ItemKind::Agent { background, .. } => !background,
        ItemKind::Tool { .. } => true,
        ItemKind::UserMessage { .. }
        | ItemKind::AssistantMessage { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => false,
    });
    if foreground {
        return Phase::Working;
    }
    let reasoning = live
        .get(&LiveTarget::Reasoning {
            run_id: run.id.clone(),
        })
        .is_some_and(|text| text.chars().any(|character| !character.is_whitespace()));
    if reasoning {
        return Phase::Thinking;
    }
    Phase::Thinking
}

#[must_use]
pub(crate) const fn status_copy(phase: Phase) -> Option<&'static str> {
    match phase {
        Phase::Thinking => Some("Thinking"),
        Phase::Working => Some("Working"),
        Phase::WaitingOnYou => Some("Waiting for you"),
        Phase::Unknown | Phase::Replying | Phase::Compacting | Phase::Settled(_) => None,
    }
}

#[must_use]
pub(crate) fn section_title(phase: Phase, worked: bool, elapsed_ms: u64) -> String {
    let elapsed = format_elapsed(elapsed_ms);
    match phase {
        Phase::Unknown if worked => "Worked".to_owned(),
        Phase::Unknown => "Thought".to_owned(),
        Phase::Settled(RunOutcome::Done) if worked => format!("Worked for {elapsed}"),
        Phase::Settled(RunOutcome::Done) => format!("Thought for {elapsed}"),
        Phase::Settled(RunOutcome::Failed | RunOutcome::Refused) => {
            format!("Failed after {elapsed}")
        }
        Phase::Settled(RunOutcome::Cancelled | RunOutcome::Interrupted) => {
            format!("Stopped after {elapsed}")
        }
        Phase::Thinking
        | Phase::Working
        | Phase::Replying
        | Phase::Compacting
        | Phase::WaitingOnYou
            if worked =>
        {
            format!("Working for {elapsed}")
        }
        Phase::Thinking
        | Phase::Working
        | Phase::Replying
        | Phase::Compacting
        | Phase::WaitingOnYou => format!("Thinking for {elapsed}"),
    }
}

#[must_use]
fn format_elapsed(millis: u64) -> String {
    let total = millis / 1_000;
    let (hours, minutes, seconds) = (total / 3_600, (total % 3_600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/phase.rs"]
mod tests;
