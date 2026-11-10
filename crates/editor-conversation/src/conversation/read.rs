use super::adapter::read_boundary;
use super::controller::Conversation;
use super::fold::{Row, RowKey};
use editor_core::{Core, HostId, HostInstance};
use aqua_core::Rect;
use artisan_api::{Item, Seq, ThreadId};

pub struct ReadCandidate {
    host: HostId,
    instance: HostInstance,
    thread: ThreadId,
    agent: Option<artisan_api::ItemId>,
    row: RowKey,
    seq: Seq,
}

impl ReadCandidate {
    #[cfg(feature = "qualification")]
    pub fn observed_seq(&self) -> Seq {
        self.seq
    }
}

pub fn evidence(
    conversation: &Conversation,
    core: &Core,
    row: &Row,
    prepared: impl FnOnce(&Row, &Item) -> bool,
) -> Option<Seq> {
    let Row::Text { id, .. } = row.semantic() else {
        return None;
    };
    let item = conversation.view.as_ref()?.item(id)?;
    if !prepared(row, item) {
        return None;
    }
    Some(
        read_boundary(conversation, core)
            .filter(|(boundary, _)| boundary == id)
            .map_or(item.seq, |(_, seq)| seq),
    )
}

pub fn capture(
    conversation: &Conversation,
    core: &Core,
    row: &Row,
    prepared: impl FnOnce(&Row, &Item) -> bool,
) -> Option<ReadCandidate> {
    if !conversation.host_is_current(core) {
        return None;
    }
    let (host, thread) = conversation.open.as_ref()?;
    Some(ReadCandidate {
        host: host.clone(),
        instance: conversation.host_instance()?,
        thread: thread.clone(),
        agent: conversation.agent.clone(),
        row: row.key(),
        seq: evidence(conversation, core, row, prepared)?,
    })
}

pub fn painted(bounds: Rect, clip: Rect, window_active: bool, revealed: bool) -> bool {
    window_active
        && revealed
        && bounds.h > 0.0
        && bounds.w > 0.0
        && clip.w > 0.0
        && clip.h > 0.0
        && bounds.x < clip.right()
        && bounds.right() > clip.x
        && bounds.y < clip.bottom()
        && bounds.bottom() > clip.y
        && bounds.bottom() <= clip.bottom()
}

pub fn acknowledge(
    candidate: ReadCandidate,
    conversation: &Conversation,
    core: &mut Core,
    prepared: impl FnOnce(&Row, &Item) -> bool,
) -> bool {
    if conversation.open.as_ref() != Some(&(candidate.host.clone(), candidate.thread.clone()))
        || conversation.host_instance() != Some(candidate.instance)
        || conversation.agent != candidate.agent
    {
        return false;
    }
    let Some((row, _)) = conversation
        .row_position(&candidate.row)
        .and_then(|index| conversation.rows.get(index))
    else {
        return false;
    };
    if evidence(conversation, core, row, prepared) != Some(candidate.seq) {
        return false;
    }
    let Some(host) = core.host_mut_if_instance(&candidate.host, candidate.instance) else {
        return false;
    };
    host.acknowledge_read(candidate.thread, candidate.seq);
    super::open_timing::acknowledged();
    true
}
