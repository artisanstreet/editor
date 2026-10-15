//! One current-thread projection shared by the transcript, composer and inspector.
//! Historical Items stay in Forge; changing observation releases this snapshot and task.

use super::{HostData, HostEvent};
use artisan_api::{Event, InspectorSummary, ItemKind, NoticeKind, Seq, ThreadId, ToolKind};
use gpui::{Context, Task};
use std::sync::Arc;

const MAX_SUMMARY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub struct ThreadSummary {
    pub value: Option<Arc<InspectorSummary>>,
    pub loading: bool,
    pub error: Option<String>,
    minimum_revision: Seq,
    dirty: bool,
    task: Option<Task<()>>,
}

pub(super) struct ObservedSummary {
    thread: ThreadId,
    state: ThreadSummary,
}

impl HostData {
    pub fn thread_summary(&self, thread: &ThreadId) -> Option<&ThreadSummary> {
        self.summary
            .as_ref()
            .filter(|summary| &summary.thread == thread)
            .map(|summary| &summary.state)
    }

    /// Retry is explicit after failure; all consumers share this request and snapshot.
    pub fn load_thread_summary(&mut self, thread: &ThreadId, retry: bool, cx: &mut Context<Self>) {
        if !self.is_live() {
            return;
        }
        let Some(summary) = self
            .summary
            .as_mut()
            .filter(|summary| &summary.thread == thread)
        else {
            return;
        };
        let state = &mut summary.state;
        if state.loading
            || (!retry && (state.error.is_some() || (!state.dirty && state.value.is_some())))
        {
            return;
        }
        state.loading = true;
        state.error = None;
        let minimum = state.minimum_revision;
        let client = self.client.clone();
        let thread = thread.clone();
        // The task is the request: a new observation or connection drops it.
        state.task = Some(cx.spawn(async move |this, cx| {
            let result = client.inspector(&thread).await;
            let _ = this.update(cx, |this, cx| {
                this.finish_thread_summary(thread, minimum, result, cx);
            });
        }));
        cx.emit(HostEvent::ThreadSummaryChanged {
            thread_id: summary.thread.clone(),
        });
        cx.notify();
    }

    fn finish_thread_summary(
        &mut self,
        thread: ThreadId,
        minimum: Seq,
        result: anyhow::Result<InspectorSummary>,
        cx: &mut Context<Self>,
    ) {
        let Some(summary) = self
            .summary
            .as_mut()
            .filter(|summary| summary.thread == thread)
        else {
            return;
        };
        let state = &mut summary.state;
        state.loading = false;
        state.error = None;
        state.task = None;
        match result {
            Ok(value) if value.revision < state.minimum_revision => {
                if state.minimum_revision > minimum {
                    // A newer event arrived during the request; fetch its projection once.
                    self.load_thread_summary(&thread, true, cx);
                    return;
                }
                state.error = Some(
                    "Forge returned thread details older than its latest update. Retry to refresh."
                        .into(),
                );
            }
            Ok(value) if summary_bytes(&value) > MAX_SUMMARY_BYTES => {
                state.error = Some("Thread details exceed the 16 MiB display limit.".into());
            }
            Ok(value) => {
                if state
                    .value
                    .as_ref()
                    .is_none_or(|previous| value.revision >= previous.revision)
                {
                    state.minimum_revision = value.revision;
                    state.value = Some(Arc::new(value));
                    state.dirty = false;
                }
            }
            Err(error) => state.error = Some(format!("Couldn't load thread details: {error:#}")),
        }
        cx.emit(HostEvent::ThreadSummaryChanged { thread_id: thread });
        cx.notify();
    }

    pub(super) fn reset_thread_summary(&mut self, cx: &mut Context<Self>) {
        self.summary = self
            .observed
            .borrow()
            .clone()
            .map(|thread| ObservedSummary {
                thread,
                state: ThreadSummary {
                    dirty: true,
                    ..ThreadSummary::default()
                },
            });
        if let Some(thread) = self.summary.as_ref().map(|summary| summary.thread.clone()) {
            self.load_thread_summary(&thread, false, cx);
        }
    }

    pub(super) fn summary_connection_changed(&mut self, live: bool, cx: &mut Context<Self>) {
        let Some(summary) = self.summary.as_mut() else {
            return;
        };
        summary.state.task = None;
        summary.state.loading = false;
        summary.state.dirty = true;
        if live {
            let thread = summary.thread.clone();
            self.load_thread_summary(&thread, true, cx);
        }
    }

    pub(super) fn invalidate_thread_summary(
        &mut self,
        event: &Event,
        seq: Option<Seq>,
        cx: &mut Context<Self>,
    ) {
        let relevant = match event {
            Event::Run { .. } | Event::ItemState { .. } => true,
            Event::Item { item } => matches!(
                item.kind,
                ItemKind::Agent { .. }
                    | ItemKind::Plan { .. }
                    | ItemKind::Question { .. }
                    | ItemKind::Notice {
                        kind: NoticeKind::Goal { .. },
                        ..
                    }
                    | ItemKind::Tool {
                        tool: ToolKind::Shell { .. } | ToolKind::Edit { .. },
                        ..
                    }
            ),
            _ => false,
        };
        if !relevant {
            return;
        }
        let Some(thread) = event.thread_id() else {
            return;
        };
        let Some(summary) = self
            .summary
            .as_mut()
            .filter(|summary| &summary.thread == thread)
        else {
            return;
        };
        let state = &mut summary.state;
        state.minimum_revision = state.minimum_revision.max(seq.unwrap_or(0));
        if state
            .value
            .as_ref()
            .is_some_and(|value| value.revision >= state.minimum_revision)
        {
            return;
        }
        state.dirty = true;
        // A new canonical event permits another attempt after a prior failure.
        state.error = None;
        self.load_thread_summary(thread, false, cx);
    }
}

fn summary_bytes(summary: &InspectorSummary) -> usize {
    use crate::{CollectionBytes as _, ResidentBytes as _};
    fn question_heap(question: &artisan_api::Question) -> usize {
        question.id.capacity()
            + question.header.as_ref().map_or(0, String::capacity)
            + question.text.resident_bytes()
            + question.options.resident_bytes(|option| {
                option.id.capacity()
                    + option.label.capacity()
                    + option
                        .description
                        .as_ref()
                        .map_or(0, artisan_api::TextContent::resident_bytes)
            })
    }
    std::mem::size_of_val(summary)
        + summary
            .plan
            .resident_bytes(|step| step.text.resident_bytes())
        + summary.questions.capacity() * std::mem::size_of::<artisan_api::ThreadQuestionnaire>()
        + summary
            .questions
            .iter()
            .map(|questionnaire| {
                questionnaire.id.0.capacity()
                    + questionnaire.questions.resident_bytes(question_heap)
            })
            .sum::<usize>()
        + summary.terminals.capacity() * std::mem::size_of::<artisan_api::InspectorTerminal>()
        + summary
            .terminals
            .iter()
            .map(|terminal| {
                terminal.id.0.capacity()
                    + terminal.command.resident_bytes()
                    + terminal.cwd.as_ref().map_or(0, String::capacity)
            })
            .sum::<usize>()
        + summary.agents.capacity() * std::mem::size_of::<artisan_api::InspectorAgent>()
        + summary
            .agents
            .iter()
            .map(|agent| {
                agent.id.0.capacity() + agent.label.capacity() + agent.description.resident_bytes()
            })
            .sum::<usize>()
        + summary.goal.as_ref().map_or(0, |goal| {
            goal.objective.resident_bytes() + goal.summary.capacity()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_question_text_counts_toward_the_summary_budget() {
        let summary = InspectorSummary {
            questions: vec![artisan_api::ThreadQuestionnaire {
                id: "question".into(),
                questions: vec![artisan_api::Question {
                    id: "choice".into(),
                    header: None,
                    text: "Choose".into(),
                    multi_select: false,
                    options: vec![artisan_api::QuestionOption {
                        id: "option-1".into(),
                        label: "item".into(),
                        description: Some("x".repeat(MAX_SUMMARY_BYTES).into()),
                    }]
                    .into(),
                }]
                .into(),
            }],
            ..InspectorSummary::default()
        };
        assert!(summary_bytes(&summary) > MAX_SUMMARY_BYTES);
    }
    fn host(cx: &mut gpui::TestAppContext) -> gpui::Entity<HostData> {
        use gpui::AppContext as _;
        let identity = artisan_api::tls::Identity::generate("summary-test").unwrap();
        let fingerprint =
            artisan_api::tls::fingerprint(identity.certificate_der().unwrap().as_ref());
        let client = crate::ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
        cx.new(|cx| {
            let mut host = HostData::new(crate::HostId("test".into()), "test".into(), client, cx);
            host._connection.abort();
            host._follow = gpui::Task::ready(());
            host.summary = Some(ObservedSummary {
                thread: "current".into(),
                state: ThreadSummary {
                    minimum_revision: 20,
                    ..Default::default()
                },
            });
            host
        })
    }
    #[gpui::test]
    fn a_summary_for_another_thread_or_older_revision_does_not_replace_the_current_one(
        cx: &mut gpui::TestAppContext,
    ) {
        let host = host(cx);
        host.update(cx, |host, cx| {
            host.finish_thread_summary(
                "current".into(),
                20,
                Ok(InspectorSummary {
                    revision: 20,
                    ..Default::default()
                }),
                cx,
            );
            assert_eq!(
                host.thread_summary(&"current".into())
                    .unwrap()
                    .value
                    .as_ref()
                    .unwrap()
                    .revision,
                20
            );
            host.finish_thread_summary(
                "previous".into(),
                20,
                Ok(InspectorSummary {
                    revision: 99,
                    ..Default::default()
                }),
                cx,
            );
            assert_eq!(
                host.thread_summary(&"current".into())
                    .unwrap()
                    .value
                    .as_ref()
                    .unwrap()
                    .revision,
                20
            );
            host.finish_thread_summary(
                "current".into(),
                20,
                Ok(InspectorSummary {
                    revision: 19,
                    ..Default::default()
                }),
                cx,
            );
            let summary = host.thread_summary(&"current".into()).unwrap();
            assert!(summary.error.is_some());
            assert_eq!(summary.value.as_ref().unwrap().revision, 20);
        });
    }
    #[gpui::test]
    fn summary_failure_stays_retryable_and_success_clears_it(cx: &mut gpui::TestAppContext) {
        let host = host(cx);
        host.update(cx, |host, cx| {
            host.finish_thread_summary(
                "current".into(),
                20,
                Err(anyhow::anyhow!("temporary read failure")),
                cx,
            );
            let summary = host.thread_summary(&"current".into()).unwrap();
            assert!(summary.error.is_some());
            assert!(!summary.loading);
            // The same production completion path receives the explicit retry's reply.
            host.finish_thread_summary(
                "current".into(),
                20,
                Ok(InspectorSummary {
                    revision: 21,
                    ..Default::default()
                }),
                cx,
            );
            let summary = host.thread_summary(&"current".into()).unwrap();
            assert!(summary.error.is_none());
            assert_eq!(summary.value.as_ref().unwrap().revision, 21);
        });
    }
}
