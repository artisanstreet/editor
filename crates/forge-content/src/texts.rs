//! Every authored text and stored collection of an item, visited the same
//! way before storage and on Forge's reads. Identities and short display
//! labels stay metadata.
use artisan_api::{ApprovalRequest, Collection, ContentRef, ItemKind, TextContent, ToolKind};

macro_rules! approval_texts {
    ($approval:expr, $visit:ident $(,$mutable:tt)?) => {{
        let approval = $approval;
        for text in &$($mutable)? approval.reason { $visit(text)?; }
        match &$($mutable)? approval.request {
            ApprovalRequest::Command { command, .. } => $visit(command)?,
            ApprovalRequest::Action { description } => $visit(description)?,
            ApprovalRequest::FileChange => {}
        }
    }};
}
macro_rules! item_texts {
    ($kind:expr, $visit:ident $(,$mutable:tt)?) => {{
        match $kind {
            ItemKind::UserMessage { text, .. } | ItemKind::AssistantMessage { text, .. }
                | ItemKind::Notice { text, .. } => $visit(text)?,
            ItemKind::Error { message, .. } => $visit(message)?,
            ItemKind::Agent { description, prompt, result, .. } => {
                $visit(description)?;
                for text in prompt { $visit(text)?; }
                for text in result { $visit(text)?; }
            }
            ItemKind::Tool { tool, output, approval, .. } => {
                match tool {
                    ToolKind::AgentMessage { text, .. } => $visit(text)?,
                    ToolKind::Shell { command, .. } => $visit(command)?,
                    ToolKind::Mcp { input, .. } | ToolKind::Other { input, .. } => {
                        for text in input { $visit(text)?; }
                    }
                    ToolKind::Edit { changes: Collection::Inline(changes) } => {
                        for change in changes {
                            for text in &$($mutable)? change.diff { $visit(text)?; }
                        }
                    }
                    _ => {}
                }
                for output in output { $visit(&$($mutable)? output.text)?; }
                for approval in approval { approval_texts!(approval, $visit $(,$mutable)?); }
            }
            ItemKind::Question { questions, answer_text, .. } => {
                for text in answer_text { $visit(text)?; }
                if let Collection::Inline(questions) = questions {
                    for question in questions {
                        $visit(&$($mutable)? question.text)?;
                        if let Collection::Inline(options) = &$($mutable)? question.options {
                            for option in options {
                                for text in &$($mutable)? option.description { $visit(text)?; }
                            }
                        }
                    }
                }
            }
            ItemKind::Plan { steps, proposal, approval } => {
                if let Collection::Inline(steps) = steps {
                    for step in steps { $visit(&$($mutable)? step.text)?; }
                }
                for text in proposal { $visit(text)?; }
                for approval in approval { approval_texts!(approval, $visit $(,$mutable)?); }
            }
            ItemKind::Image { caption, .. } => { for text in caption { $visit(text)?; } }
            ItemKind::Compaction { .. } => {}
        }
        Ok(())
    }};
}

/// Traversals of an item's authored payload.
pub trait ItemTexts {
    fn try_visit_texts<E>(&self, visit: impl FnMut(&TextContent) -> Result<(), E>)
    -> Result<(), E>;
    fn try_visit_texts_mut<E>(
        &mut self,
        visit: impl FnMut(&mut TextContent) -> Result<(), E>,
    ) -> Result<(), E>;
    /// Stored collections, visited apart from resident text. A stored
    /// collection's own dependencies belong to its immutable source.
    fn try_visit_collection_sources<E>(
        &self,
        visit: impl FnMut(&ContentRef) -> Result<(), E>,
    ) -> Result<(), E>;
    /// Stores large collections by identity. Nested texts must already use
    /// the store's source descriptors, or the identity differs.
    fn compact_collections(&mut self) -> Result<(), serde_json::Error>;
}

impl ItemTexts for ItemKind {
    fn try_visit_texts<E>(
        &self,
        mut visit: impl FnMut(&TextContent) -> Result<(), E>,
    ) -> Result<(), E> {
        item_texts!(self, visit)
    }

    fn try_visit_texts_mut<E>(
        &mut self,
        mut visit: impl FnMut(&mut TextContent) -> Result<(), E>,
    ) -> Result<(), E> {
        item_texts!(self, visit, mut)
    }

    fn try_visit_collection_sources<E>(
        &self,
        mut visit: impl FnMut(&ContentRef) -> Result<(), E>,
    ) -> Result<(), E> {
        match self {
            Self::Plan {
                steps: Collection::Stored(value),
                ..
            }
            | Self::Tool {
                tool:
                    ToolKind::Edit {
                        changes: Collection::Stored(value),
                    },
                ..
            } => visit(&value.source)?,
            Self::Question {
                questions, answers, ..
            } => {
                match questions {
                    Collection::Stored(value) => visit(&value.source)?,
                    Collection::Inline(questions) => {
                        for question in questions {
                            if let Collection::Stored(value) = &question.options {
                                visit(&value.source)?;
                            }
                        }
                    }
                }
                match answers {
                    Some(Collection::Stored(value)) => visit(&value.source)?,
                    Some(Collection::Inline(answers)) => {
                        for answer in answers.iter().flatten() {
                            if let artisan_api::QuestionAnswer::Choices {
                                ids: Collection::Stored(value),
                            } = answer
                            {
                                visit(&value.source)?;
                            }
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn compact_collections(&mut self) -> Result<(), serde_json::Error> {
        match self {
            Self::Plan { steps, .. } => {
                crate::compact_collection(steps)?;
            }
            Self::Tool {
                tool: ToolKind::Edit { changes },
                ..
            } => {
                crate::compact_collection(changes)?;
            }
            Self::Question {
                questions, answers, ..
            } => {
                if let Collection::Inline(questions) = questions {
                    for question in questions {
                        crate::compact_collection(&mut question.options)?;
                    }
                }
                crate::compact_collection(questions)?;
                if let Some(answers) = answers {
                    if let Collection::Inline(answers) = answers {
                        for answer in answers.iter_mut().flatten() {
                            if let artisan_api::QuestionAnswer::Choices { ids } = answer {
                                crate::compact_collection(ids)?;
                            }
                        }
                    }
                    crate::compact_collection(answers)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
