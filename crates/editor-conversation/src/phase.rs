//! What a run is doing, computed from its items on demand, and the words
//! the transcript uses for it.

use artisan_api::{Item, ItemKind, LiveTarget, Run, RunStatus, Status, ToolKind};
use std::collections::HashMap;
use std::hash::BuildHasher;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// The page has no canonical Run metadata. Never infer its outcome from Items.
    Unknown,
    Thinking,
    Working,
    /// The reply is streaming.
    Replying,
    Compacting,
    /// An approval or question waits on the user.
    WaitingOnYou,
    /// Only background shells or agents are still running.
    Background,
    Settled(RunStatus),
}

/// The phase of `run` from its items (any order) and live text.
#[must_use]
pub fn turn_phase<S: BuildHasher>(
    run: &Run,
    items: &[&Item],
    live: &HashMap<LiveTarget, std::sync::Arc<ropey::Rope>, S>,
) -> Phase {
    if !run.status.is_live() {
        return Phase::Settled(run.status);
    }
    if items.iter().any(|item| item.status == Status::AwaitingUser) {
        return Phase::WaitingOnYou;
    }
    let latest = items
        .iter()
        .filter(|item| item.parent_id.is_none())
        .max_by_key(|item| item.position);
    if let Some(item) = latest {
        match &item.kind {
            ItemKind::Compaction { .. } if item.status == Status::Running => {
                return Phase::Compacting;
            }
            ItemKind::AssistantMessage { .. }
                if item.status == Status::Running
                    || live.contains_key(&LiveTarget::Text {
                        item_id: item.id.clone(),
                    }) =>
            {
                return Phase::Replying;
            }
            _ => {}
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
        _ => false,
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
    let background = items.iter().filter(running).any(|item| {
        matches!(
            &item.kind,
            ItemKind::Tool {
                tool: ToolKind::Shell {
                    background: true,
                    ..
                },
                ..
            } | ItemKind::Agent {
                background: true,
                ..
            }
        )
    });
    if background {
        return Phase::Background;
    }
    Phase::Thinking
}

/// The live status line; `None` while the reply itself shows progress.
#[must_use]
pub(crate) fn status_copy(phase: Phase) -> Option<&'static str> {
    match phase {
        Phase::Thinking => Some("Thinking"),
        Phase::Working => Some("Working"),
        Phase::Compacting => Some("Compacting the conversation…"),
        Phase::WaitingOnYou => Some("Waiting for you"),
        Phase::Background => Some("Waiting for background work…"),
        Phase::Unknown | Phase::Replying | Phase::Settled(_) => None,
    }
}

/// A work section's header: what the run is doing, or how it ended. Never
/// a generic title. The verb comes from what the run did, not from this
/// moment: a run that used a tool or an agent is "Working"/"Worked" all
/// through, so the header doesn't flip each time it goes back to thinking;
/// one that only thought is "Thinking"/"Thought".
#[must_use]
pub(crate) fn section_title(phase: Phase, worked: bool, elapsed_ms: u64) -> String {
    let elapsed = format_elapsed(elapsed_ms);
    match phase {
        Phase::Unknown if worked => "Work".to_owned(),
        Phase::Unknown => "Thought".to_owned(),
        Phase::WaitingOnYou => "Waiting for you".to_owned(),
        Phase::Settled(RunStatus::Done) if worked => format!("Worked for {elapsed}"),
        Phase::Settled(RunStatus::Done) => format!("Thought for {elapsed}"),
        Phase::Thinking
        | Phase::Working
        | Phase::Replying
        | Phase::Compacting
        | Phase::Background
        | Phase::Settled(RunStatus::Running)
            if worked =>
        {
            format!("Working for {elapsed}")
        }
        Phase::Thinking
        | Phase::Working
        | Phase::Replying
        | Phase::Compacting
        | Phase::Background
        | Phase::Settled(RunStatus::Running) => format!("Thinking for {elapsed}"),
        Phase::Settled(RunStatus::Failed) => "Failed".to_owned(),
        Phase::Settled(RunStatus::Cancelled) => "Cancelled".to_owned(),
        Phase::Settled(RunStatus::Interrupted) => "Interrupted".to_owned(),
        Phase::Settled(RunStatus::Refused) => "Refused".to_owned(),
    }
}

#[must_use]
pub(crate) fn format_elapsed(millis: u64) -> String {
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
