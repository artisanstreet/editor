use crate::conversation::controller::Conversation;
use crate::conversation::phase::Phase;
use crate::conversation::thread::ThreadView;
use aqua_core::{LiveRegion, Role};
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::Style;
use artisan_api::{
    ContentId, ContentRange, ContentRef, ItemId, ItemKind, LiveTarget, Run, RunOutcome, RunState,
    TextContent, ThreadId, Timestamp,
};
use editor_core::client::RangeError;
use tokio_util::task::AbortOnDropHandle;

pub const ANNOUNCER_KEY: &str = "conversation-announcer";
const REPLY_TAIL_CHARS: usize = 4096;
const MAX_CHAR_BYTES: usize = 4;
const REPLY_TAIL_BYTES: u32 = 16 * 1024;
const CHAR_ALIGNMENT_BYTES: u32 = 3;
const _: () = assert!(
    REPLY_TAIL_BYTES as usize == REPLY_TAIL_CHARS * MAX_CHAR_BYTES,
    "the reply tail holds the last characters even when every one takes four bytes"
);
const _: () = assert!(
    REPLY_TAIL_BYTES + CHAR_ALIGNMENT_BYTES <= artisan_api::CONTENT_RANGE_BYTES,
    "the reply tail must fit one content range"
);
const SETTLED_RECENTLY_MS: Timestamp = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Turn {
    Idle,
    Working { engine: &'static str },
    WaitingOnYou { engine: &'static str },
    Replying { engine: &'static str, text: String },
    Finished { text: String },
    Finishing,
    Stopped,
    Failed { error: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Announcement {
    text: String,
    busy: bool,
}

#[must_use]
fn announcement(turn: &Turn) -> Announcement {
    let text = match turn {
        Turn::Idle | Turn::Finishing => String::new(),
        Turn::Working { engine } => format!("{engine} is working"),
        Turn::WaitingOnYou { engine } => format!("{engine} is waiting for you"),
        Turn::Replying { engine, text } => {
            latest_paragraph(text).map_or_else(|| format!("{engine} is replying"), str::to_owned)
        }
        Turn::Finished { text } => unread_paragraph(text).map_or_else(
            || "Reply finished".to_owned(),
            |paragraph| format!("{paragraph}\n\nReply finished"),
        ),
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
        busy: matches!(
            turn,
            Turn::Working { .. } | Turn::Replying { .. } | Turn::Finishing
        ),
    }
}

fn unread_paragraph(text: &str) -> Option<&str> {
    let paragraph = text.rsplit_once("\n\n").map_or(text, |(_, tail)| tail);
    Some(paragraph.trim()).filter(|paragraph| !paragraph.is_empty())
}

fn latest_paragraph(text: &str) -> Option<&str> {
    let (complete, _) = text.rsplit_once("\n\n")?;
    let complete = complete.trim_end();
    let paragraph = complete
        .rsplit_once("\n\n")
        .map_or(complete, |(_, tail)| tail);
    Some(paragraph.trim()).filter(|paragraph| !paragraph.is_empty())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredReply {
    item: ItemId,
    source: ContentRef,
}

impl StoredReply {
    pub(crate) fn tail_query(&self) -> (u64, u32) {
        let offset = self
            .source
            .bytes
            .saturating_sub(u64::from(REPLY_TAIL_BYTES));
        (offset, REPLY_TAIL_BYTES + CHAR_ALIGNMENT_BYTES)
    }

    pub(crate) fn source(&self) -> &ContentRef {
        &self.source
    }
}

pub(crate) enum ReplyTail {
    Reading {
        reply: StoredReply,
        _task: AbortOnDropHandle<()>,
    },
    Read {
        reply: StoredReply,
        tail: String,
    },
    Unread {
        reply: StoredReply,
    },
}

impl ReplyTail {
    pub(crate) fn reply(&self) -> &StoredReply {
        match self {
            Self::Reading { reply, .. } | Self::Read { reply, .. } | Self::Unread { reply } => {
                reply
            }
        }
    }

    pub(crate) fn settle(
        thread: &ThreadId,
        reply: StoredReply,
        result: Result<ContentRange, RangeError>,
    ) -> Self {
        match read_tail(thread, &reply, result) {
            Ok(tail) => Self::Read { reply, tail },
            Err(error) => {
                tracing::warn!(
                    error = %::diagnostics::chain(&error),
                    "couldn't read the end of a finished reply to announce it"
                );
                Self::Unread { reply }
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadReplyTailError {
    #[error("read the end of reply {item} ({content}) in thread {thread}")]
    Read {
        thread: ThreadId,
        item: ItemId,
        content: ContentId,
        #[source]
        source: Box<RangeError>,
    },
    #[error(
        "the end of reply {item} ({content}) in thread {thread} stopped at byte {end} of {total}"
    )]
    ShortOfEnd {
        thread: ThreadId,
        item: ItemId,
        content: ContentId,
        end: u64,
        total: u64,
    },
}

fn read_tail(
    thread: &ThreadId,
    reply: &StoredReply,
    result: Result<ContentRange, RangeError>,
) -> Result<String, ReadReplyTailError> {
    let range = result.map_err(|source| ReadReplyTailError::Read {
        thread: thread.clone(),
        item: reply.item.clone(),
        content: reply.source.id.clone(),
        source: Box::new(source),
    })?;
    if range.end != range.total {
        return Err(ReadReplyTailError::ShortOfEnd {
            thread: thread.clone(),
            item: reply.item.clone(),
            content: reply.source.id.clone(),
            end: range.end,
            total: range.total,
        });
    }
    Ok(last_chars(&range.text))
}

fn last_chars(text: &str) -> String {
    let skipped = text.chars().count().saturating_sub(REPLY_TAIL_CHARS + 1);
    text.chars().skip(skipped).collect()
}

enum ReplyText {
    Known(String),
    Stored(StoredReply),
}

fn latest_run(view: &ThreadView) -> Option<&Run> {
    view.runs
        .values()
        .max_by_key(|run| (run.started_at, &run.id))
}

fn settled_long_ago(run: &Run, now: Timestamp) -> bool {
    matches!(&run.state, RunState::Finished { ended_at, .. } if now - ended_at > SETTLED_RECENTLY_MS)
}

#[must_use]
pub(crate) fn finished_stored_reply(view: &ThreadView, now: Timestamp) -> Option<StoredReply> {
    let run = latest_run(view)?;
    if settled_long_ago(run, now) {
        return None;
    }
    match (&run.state, reply_text(view, run)) {
        (
            RunState::Finished {
                outcome: RunOutcome::Done,
                ..
            },
            ReplyText::Stored(reply),
        ) => Some(reply),
        (
            RunState::Finished { .. } | RunState::Running,
            ReplyText::Known(_) | ReplyText::Stored(_),
        ) => None,
    }
}

#[must_use]
fn turn(conversation: &Conversation, now: Timestamp) -> Turn {
    let Some(view) = conversation.view.as_ref() else {
        return Turn::Idle;
    };
    let Some(run) = latest_run(view) else {
        return Turn::Idle;
    };
    if settled_long_ago(run, now) {
        return Turn::Idle;
    }
    let engine = run.engine.display_name();
    match &run.state {
        RunState::Finished {
            outcome: RunOutcome::Done,
            ..
        } => match reply_text(view, run) {
            ReplyText::Known(text) => Turn::Finished { text },
            ReplyText::Stored(reply) => match conversation
                .reply_tail
                .as_ref()
                .filter(|tail| *tail.reply() == reply)
            {
                Some(ReplyTail::Read { tail, .. }) => Turn::Finished { text: tail.clone() },
                Some(ReplyTail::Unread { .. }) => Turn::Finished {
                    text: String::new(),
                },
                Some(ReplyTail::Reading { .. }) | None => Turn::Finishing,
            },
        },
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
                text: match reply_text(view, run) {
                    ReplyText::Known(text) => text,
                    ReplyText::Stored(_) => String::new(),
                },
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

fn reply_text(view: &ThreadView, run: &Run) -> ReplyText {
    let Some(item) = view.items.values().rev().find(|item| {
        item.parent_id.is_none()
            && item.run_id.as_ref() == Some(&run.id)
            && matches!(item.kind, ItemKind::AssistantMessage { .. })
    }) else {
        return ReplyText::Known(String::new());
    };
    let target = LiveTarget::Text {
        item_id: item.id.clone(),
    };
    match (view.live.get(&target), &item.kind) {
        (Some(rope), _) => {
            let length = rope.len_chars();
            ReplyText::Known(
                rope.slice(length.saturating_sub(REPLY_TAIL_CHARS)..)
                    .to_string(),
            )
        }
        (None, ItemKind::AssistantMessage { text, .. }) => match text {
            TextContent::Inline(text) => ReplyText::Known(last_chars(text)),
            TextContent::Stored(source) => ReplyText::Stored(StoredReply {
                item: item.id.clone(),
                source: source.clone(),
            }),
        },
        (None, _) => ReplyText::Known(String::new()),
    }
}

#[must_use]
pub fn current(conversation: &Conversation) -> Announcement {
    announcement(&turn(conversation, artisan_api::now()))
}

#[must_use]
pub(crate) fn announced(transcript: Element, announcement: &Announcement) -> Element {
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
    if announcement.text.is_empty() {
        region.aria_hidden()
    } else {
        region
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/announce.rs"]
mod tests;
