//! What the Editor's copies of Forge data cost in memory: the bytes their
//! strings and vectors own, for the resident-window budgets. Allocator
//! overhead is not counted.
use artisan_api::{
    Approval, ApprovalRequest, BlobHash, Collection, Decision, FileOp, Item, ItemKind, NoticeKind,
    QuestionAnswer, TextContent, ToolKind,
};

/// Owned bytes of a value Forge sent.
pub trait ResidentBytes {
    fn resident_bytes(&self) -> usize;
}

/// Owned bytes of a collection, counting each inline record with
/// `record_bytes`. A stored collection owns only its descriptor.
pub trait CollectionBytes<T> {
    fn resident_bytes(&self, record_bytes: impl Fn(&T) -> usize) -> usize;
}

impl<T> CollectionBytes<T> for Collection<T> {
    fn resident_bytes(&self, record_bytes: impl Fn(&T) -> usize) -> usize {
        match self {
            Self::Inline(records) => {
                records.capacity() * size_of::<T>()
                    + records.iter().map(record_bytes).sum::<usize>()
            }
            Self::Stored(reference) => reference.source.id.0.capacity(),
        }
    }
}

impl ResidentBytes for TextContent {
    fn resident_bytes(&self) -> usize {
        match self {
            Self::Inline(text) => text.capacity(),
            Self::Stored(source) => source.id.0.capacity(),
        }
    }
}

fn optional(value: Option<&String>) -> usize {
    value.map_or(0, String::capacity)
}

fn payload(value: Option<&TextContent>) -> usize {
    value.map_or(0, TextContent::resident_bytes)
}

fn approval(value: Option<&Approval>) -> usize {
    value.map_or(0, |approval| {
        payload(approval.reason.as_ref())
            + approval.options.capacity() * size_of::<Decision>()
            + match &approval.request {
                ApprovalRequest::Command { command, cwd } => {
                    command.resident_bytes() + optional(cwd.as_ref())
                }
                ApprovalRequest::Action { description } => description.resident_bytes(),
                ApprovalRequest::FileChange => 0,
            }
    })
}

impl ResidentBytes for ToolKind {
    fn resident_bytes(&self) -> usize {
        match self {
            Self::Shell { command, cwd, .. } => command.resident_bytes() + optional(cwd.as_ref()),
            Self::Read { path, .. } | Self::ViewImage { path } => path.capacity(),
            Self::Edit { changes } => changes.resident_bytes(|change| {
                change.path.capacity()
                    + payload(change.diff.as_ref())
                    + match &change.op {
                        FileOp::Move { from } => from.capacity(),
                        _ => 0,
                    }
            }),
            Self::Search { pattern, path, .. } => pattern.capacity() + optional(path.as_ref()),
            Self::WebFetch { url } => url.capacity(),
            Self::WebSearch { query } => query.capacity(),
            Self::Mcp {
                server,
                tool,
                input,
            } => server.capacity() + tool.capacity() + payload(input.as_ref()),
            Self::AgentMessage {
                recipient,
                target,
                summary,
                text,
                ..
            } => {
                recipient.capacity()
                    + target.as_ref().map_or(0, |target| target.0.capacity())
                    + optional(summary.as_ref())
                    + text.resident_bytes()
            }
            Self::Agents {
                addresses, targets, ..
            } => {
                addresses.capacity() * size_of::<String>()
                    + addresses.iter().map(String::capacity).sum::<usize>()
                    + targets.capacity() * size_of::<artisan_api::ItemId>()
                    + targets
                        .iter()
                        .map(|target| target.0.capacity())
                        .sum::<usize>()
            }
            Self::Other { name, input } => name.capacity() + payload(input.as_ref()),
        }
    }
}

impl ResidentBytes for Item {
    fn resident_bytes(&self) -> usize {
        let content = match &self.kind {
            ItemKind::UserMessage { text, images, .. } => {
                text.resident_bytes()
                    + images.capacity() * size_of::<BlobHash>()
                    + images.iter().map(|hash| hash.0.capacity()).sum::<usize>()
            }
            ItemKind::AssistantMessage { text, .. } => text.resident_bytes(),
            ItemKind::Tool {
                tool,
                title,
                output,
                approval: request,
                ..
            } => {
                tool.resident_bytes()
                    + optional(title.as_ref())
                    + approval(request.as_ref())
                    + output
                        .as_ref()
                        .map_or(0, |output| output.text.resident_bytes())
            }
            ItemKind::Agent {
                name,
                description,
                prompt,
                agent_id,
                model,
                result,
                label,
                ..
            } => {
                description.resident_bytes()
                    + payload(prompt.as_ref())
                    + payload(result.as_ref())
                    + [name, agent_id, model, label]
                        .into_iter()
                        .map(Option::as_ref)
                        .map(optional)
                        .sum::<usize>()
            }
            ItemKind::Question {
                questions,
                answers,
                answer_text,
                ..
            } => {
                questions.resident_bytes(|question| {
                    question.id.capacity()
                        + optional(question.header.as_ref())
                        + question.text.resident_bytes()
                        + question.options.resident_bytes(|option| {
                            option.id.capacity()
                                + option.label.capacity()
                                + payload(option.description.as_ref())
                        })
                }) + payload(answer_text.as_ref())
                    + answers.as_ref().map_or(0, |answers| {
                        answers.resident_bytes(|answer| {
                            answer.as_ref().map_or(0, |answer| match answer {
                                QuestionAnswer::Choices { ids } => {
                                    ids.resident_bytes(String::capacity)
                                }
                                QuestionAnswer::Text { text } => text.capacity(),
                            })
                        })
                    })
            }
            ItemKind::Plan {
                steps,
                proposal,
                approval: request,
            } => {
                steps.resident_bytes(|step| step.text.resident_bytes())
                    + payload(proposal.as_ref())
                    + approval(request.as_ref())
            }
            ItemKind::Image { blob, caption } => blob.0.capacity() + payload(caption.as_ref()),
            ItemKind::Compaction { .. } => 0,
            ItemKind::Notice { kind, text } => {
                text.resident_bytes()
                    + match kind {
                        NoticeKind::Goal { state } => state.capacity(),
                        NoticeKind::Reroute { model } => model.capacity(),
                        NoticeKind::Safety | NoticeKind::Info => 0,
                    }
            }
            ItemKind::Error { message, .. } => message.resident_bytes(),
        };
        size_of::<Self>()
            + self.id.0.capacity()
            + self.thread_id.0.capacity()
            + self.run_id.as_ref().map_or(0, |id| id.0.capacity())
            + self.parent_id.as_ref().map_or(0, |id| id.0.capacity())
            + content
    }
}
