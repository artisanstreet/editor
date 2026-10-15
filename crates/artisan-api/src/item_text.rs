//! Every authored payload follows the same traversal before storage and on
//! explicitly admitted Forge reads. Identities and short display labels stay metadata.
use crate::{ApprovalRequest, Collection, ItemKind, TextContent, ToolKind};

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
                    ToolKind::Edit { changes } => {
                        if let Collection::Inline(changes) = changes {
                            for change in changes {
                                for text in &$($mutable)? change.diff { $visit(text)?; }
                            }
                        }
                    }
                    _ => {}
                }
                for output in output { $visit(&$($mutable)? output.text)?; }
                for approval in approval { approval_texts!(approval, $visit $(,$mutable)?); }
            }
            ItemKind::Approval { approval, .. } => approval_texts!(approval, $visit $(,$mutable)?),
            ItemKind::Question { questions,answer_text,.. } => {
                for text in answer_text {$visit(text)?;}
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
                if let Collection::Inline(steps) = steps { for step in steps { $visit(&$($mutable)? step.text)?; } }
                for text in proposal { $visit(text)?; }
                for approval in approval { approval_texts!(approval, $visit $(,$mutable)?); }
            }
            ItemKind::Image { caption, .. } => { for text in caption { $visit(text)?; } }
            ItemKind::Compaction { .. } => {}
        }
        Ok(())
    }};
}
impl ItemKind {
    pub fn try_visit_texts<E>(
        &self,
        mut visit: impl FnMut(&TextContent) -> Result<(), E>,
    ) -> Result<(), E> {
        item_texts!(self, visit)
    }
    pub fn try_visit_texts_mut<E>(
        &mut self,
        mut visit: impl FnMut(&mut TextContent) -> Result<(), E>,
    ) -> Result<(), E> {
        item_texts!(self, visit, mut)
    }
}

impl ItemKind {
    /// Collection owners are visited independently of resident text. A stored
    /// collection's transitive dependencies are owned by its immutable source.
    pub fn try_visit_collection_sources<E>(
        &self,
        mut visit: impl FnMut(&crate::ContentRef) -> Result<(), E>,
    ) -> Result<(), E> {
        match self {
            Self::Plan { steps, .. } => {
                if let Collection::Stored(value) = steps {
                    visit(&value.source)?;
                }
            }
            Self::Tool {
                tool: ToolKind::Edit { changes },
                ..
            } => {
                if let Collection::Stored(value) = changes {
                    visit(&value.source)?;
                }
            }
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
                if let Some(answers) = answers {
                    match answers {
                        Collection::Stored(value) => visit(&value.source)?,
                        Collection::Inline(answers) => {
                            for answer in answers.iter().flatten() {
                                if let crate::QuestionAnswer::Choices {
                                    ids: Collection::Stored(value),
                                } = answer
                                {
                                    visit(&value.source)?;
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl ItemKind {
    /// Pure retention conversion. Nested TextContent must already use the same
    /// source descriptors as Store before this method creates a collection hash.
    pub fn compact_collections(&mut self) -> Result<(), serde_json::Error> {
        match self {
            Self::Plan { steps, .. } => {
                steps.compact_if_large()?;
            }
            Self::Tool {
                tool: ToolKind::Edit { changes },
                ..
            } => {
                changes.compact_if_large()?;
            }
            Self::Question {
                questions, answers, ..
            } => {
                if let Collection::Inline(questions) = questions {
                    for question in questions {
                        question.options.compact_if_large()?;
                    }
                }
                questions.compact_if_large()?;
                if let Some(answers) = answers {
                    if let Collection::Inline(answers) = answers {
                        for answer in answers.iter_mut().flatten() {
                            if let crate::QuestionAnswer::Choices { ids } = answer {
                                ids.compact_if_large()?;
                            }
                        }
                    }
                    answers.compact_if_large()?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
