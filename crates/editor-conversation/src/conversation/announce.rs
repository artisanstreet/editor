use crate::conversation::controller::Conversation;
use crate::conversation::phase::Phase;
use crate::conversation::thread::ThreadView;
use aqua_core::{LiveRegion, Role};
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::Style;
use artisan_api::{ItemKind, LiveTarget, Run, RunOutcome, RunState, Timestamp};

pub const ANNOUNCER_KEY: &str = "conversation-announcer";
const REPLY_TAIL_CHARS: usize = 4096;
const SETTLED_RECENTLY_MS: Timestamp = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Turn {
    Idle,
    Working { engine: &'static str },
    WaitingOnYou { engine: &'static str },
    Replying { engine: &'static str, text: String },
    Finished,
    Stopped,
    Failed { error: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announcement {
    pub text: String,
    pub busy: bool,
}

#[must_use]
pub fn announcement(turn: &Turn) -> Announcement {
    let text = match turn {
        Turn::Idle => String::new(),
        Turn::Working { engine } => format!("{engine} is working"),
        Turn::WaitingOnYou { engine } => format!("{engine} is waiting for you"),
        Turn::Replying { engine, text } => {
            latest_paragraph(text).map_or_else(|| format!("{engine} is replying"), str::to_owned)
        }
        Turn::Finished => "Reply finished".to_owned(),
        Turn::Stopped => "Reply stopped".to_owned(),
        Turn::Failed { error } => error
            .as_deref()
            .map(str::trim)
            .filter(|error| !error.is_empty())
            .map_or_else(
                || "Reply failed".to_owned(),
                |error| format!("Reply failed: {error}"),
            ),
    };
    Announcement {
        text,
        busy: matches!(turn, Turn::Working { .. } | Turn::Replying { .. }),
    }
}

fn latest_paragraph(text: &str) -> Option<&str> {
    let complete = text[..text.rfind("\n\n")?].trim_end();
    let start = complete.rfind("\n\n").map_or(0, |at| at + 2);
    Some(complete[start..].trim()).filter(|paragraph| !paragraph.is_empty())
}

#[must_use]
pub fn turn(conversation: &Conversation, now: Timestamp) -> Turn {
    let Some(view) = conversation.view.as_ref() else {
        return Turn::Idle;
    };
    let Some(run) = view
        .runs
        .values()
        .max_by_key(|run| (run.started_at, &run.id))
    else {
        return Turn::Idle;
    };
    let engine = run.engine.display_name();
    match &run.state {
        RunState::Finished { ended_at, .. } if now - ended_at > SETTLED_RECENTLY_MS => Turn::Idle,
        RunState::Finished {
            outcome: RunOutcome::Done,
            ..
        } => Turn::Finished,
        RunState::Finished {
            outcome: RunOutcome::Cancelled | RunOutcome::Interrupted,
            ..
        } => Turn::Stopped,
        RunState::Finished {
            outcome: RunOutcome::Failed | RunOutcome::Refused,
            error,
            ..
        } => Turn::Failed {
            error: error.clone(),
        },
        RunState::Running => match conversation.runs.get(&run.id).map(|summary| summary.phase) {
            Some(Phase::Replying) => Turn::Replying {
                engine,
                text: reply_tail(view, run),
            },
            Some(Phase::WaitingOnYou) => Turn::WaitingOnYou { engine },
            Some(
                Phase::Unknown
                | Phase::Thinking
                | Phase::Working
                | Phase::Compacting
                | Phase::Settled(_),
            )
            | None => Turn::Working { engine },
        },
    }
}

fn reply_tail(view: &ThreadView, run: &Run) -> String {
    let Some(item) = view.items.values().rev().find(|item| {
        item.parent_id.is_none()
            && item.run_id.as_ref() == Some(&run.id)
            && matches!(item.kind, ItemKind::AssistantMessage { .. })
    }) else {
        return String::new();
    };
    let target = LiveTarget::Text {
        item_id: item.id.clone(),
    };
    match (view.live.get(&target), &item.kind) {
        (Some(rope), _) => {
            let length = rope.len_chars();
            rope.slice(length.saturating_sub(REPLY_TAIL_CHARS)..)
                .to_string()
        }
        (None, ItemKind::AssistantMessage { text, .. }) => {
            let text = text.inline().unwrap_or_default();
            let start = text
                .char_indices()
                .rev()
                .nth(REPLY_TAIL_CHARS)
                .map_or(0, |(at, _)| at);
            text[start..].to_owned()
        }
        (None, _) => String::new(),
    }
}

#[must_use]
pub fn current(conversation: &Conversation) -> Announcement {
    announcement(&turn(conversation, artisan_api::now()))
}

#[must_use]
pub fn announced(transcript: Element, announcement: &Announcement) -> Element {
    transcript.aria_busy(announcement.busy)
}

#[must_use]
pub fn announcer(announcement: &Announcement) -> Element {
    let region = UiBox::new()
        .width(0.0)
        .height(0.0)
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), None, None))
        .build()
        .exclude_from_hit_testing()
        .with_key(ANNOUNCER_KEY)
        .role(Role::Text)
        .aria_label(announcement.text.clone())
        .aria_live(LiveRegion::Polite);
    match announcement.text.is_empty() {
        true => region.aria_hidden(),
        false => region,
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/announce.rs"]
mod tests;
