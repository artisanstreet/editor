//! Read acknowledgements: what the reader actually saw of the observed
//! thread, coalesced into one writer that retries until Forge has it.
use super::{HostData, HostEvent};
use artisan_api::{Seq, ThreadId, UpdateThread};
use gpui::{Context, Task};
use std::collections::BTreeMap;
use std::time::Duration;

/// Threads with an unsent acknowledgement at most.
const PENDING: usize = 128;

#[derive(Default)]
pub(super) struct ReadAcks {
    /// The newest seq seen per thread, not yet written.
    pub(super) pending: BTreeMap<ThreadId, Seq>,
    /// Evidence from this observation, independent of mutable unread marks.
    observed: Option<Seq>,
    writer: Option<Task<()>>,
}

impl ReadAcks {
    /// Another observation starts: its first paint counts as new evidence.
    pub(super) fn observation_changed(&mut self) {
        self.observed = None;
    }
}

impl HostData {
    /// The reader saw the observed thread up to `observed_seq`.
    pub fn acknowledge_read(
        &mut self,
        thread: ThreadId,
        observed_seq: Seq,
        cx: &mut Context<Self>,
    ) {
        if self.observed.borrow().as_ref() != Some(&thread) {
            return;
        }
        let Some(summary) = self.threads.get(&thread) else {
            return;
        };
        let seq = observed_seq.min(summary.activity_seq);
        let acks = &mut self.read_acks;
        // Painting the same content again must not undo an explicit Mark Unread.
        // Revisiting a thread starts a new observation; new activity may also advance it.
        if !new_read_evidence(acks.observed, seq) {
            return;
        }
        // An offline connection can accumulate acknowledgements while navigating.
        // Keep admission bounded and retry from the next visible frame when it drains.
        if !acks.pending.contains_key(&thread) && acks.pending.len() >= PENDING {
            return;
        }
        acks.observed = Some(seq);
        if seq <= summary.read_seq && !summary.marked_unread {
            return;
        }
        acks.pending
            .entry(thread)
            .and_modify(|pending| *pending = (*pending).max(seq))
            .or_insert(seq);
        if acks.writer.is_some() {
            return;
        }
        let client = self.client.clone();
        acks.writer = Some(cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |host, _| {
                    host.read_acks
                        .pending
                        .first_key_value()
                        .map(|(id, seq)| (id.clone(), *seq))
                });
                let Ok(Some((id, seq))) = next else {
                    let _ = this.update(cx, |host, _| host.read_acks.writer = None);
                    break;
                };
                let update = UpdateThread {
                    read_seq: Some(seq),
                    ..Default::default()
                };
                match client.update_thread(&id, &update).await {
                    Ok(thread) => {
                        let written = this.update(cx, |host, cx| {
                            if host
                                .read_acks
                                .pending
                                .get(&id)
                                .is_some_and(|pending| *pending <= seq)
                            {
                                host.read_acks.pending.remove(&id);
                            }
                            if host
                                .threads
                                .get(&id)
                                .is_none_or(|old| old.seq <= thread.seq)
                            {
                                host.remember_thread(thread);
                                cx.emit(HostEvent::ThreadChanged {
                                    thread_id: id.clone(),
                                });
                                cx.notify();
                            }
                        });
                        if written.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::debug!(%id, "read acknowledgement will retry: {error:#}");
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                    }
                }
            }
        }));
    }
}

/// Only new visible evidence can clear an explicit unread mark during an observation.
fn new_read_evidence(previous: Option<Seq>, next: Seq) -> bool {
    previous.is_none_or(|previous| next > previous)
}

#[cfg(test)]
mod tests {
    use super::new_read_evidence;

    #[test]
    fn repaint_cannot_clear_manual_unread_but_new_activity_and_revisit_can() {
        assert!(new_read_evidence(None, 10));
        // The unread preference may change after this paint, without new content.
        assert!(!new_read_evidence(Some(10), 10));
        assert!(!new_read_evidence(Some(10), 9));
        assert!(new_read_evidence(Some(10), 11));
        // Changing observations resets the evidence, allowing an intentional revisit.
        assert!(new_read_evidence(None, 10));
    }
}
