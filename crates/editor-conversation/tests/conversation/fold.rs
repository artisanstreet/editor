use super::{Item, ItemKind, PlainSection};
use artisan_api::{ContentId, ContentRef, ItemId, Status, TextContent, ThreadId};

fn agent(name: &str, description: TextContent, prompt: Option<TextContent>) -> Item {
    Item {
        id: ItemId("native-agent".into()),
        thread_id: ThreadId("thread".into()),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::Running,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::Agent {
            name: Some(name.into()),
            description,
            prompt,
            agent_id: None,
            context_used: None,
            model: None,
            background: false,
            result: None,
            label: None,
            engine: None,
            thread: None,
        },
    }
}

#[test]
fn native_agent_addresses_without_a_prompt_show_the_encryption_fallback() {
    for name in ["/root", "/root/reviewer"] {
        let item = agent(name, name.into(), None);
        assert_eq!(
            PlainSection::AgentTask.inline_text(&item),
            Some("[This message is encrypted.]")
        );
    }
    for (name, description) in [
        ("/root/reviewer", "Review the patch"),
        ("reviewer", "reviewer"),
        ("/rooted", "/rooted"),
    ] {
        let item = agent(name, description.into(), None);
        assert_eq!(
            PlainSection::AgentTask.inline_text(&item),
            Some(description)
        );
    }
}

#[test]
fn agent_task_uses_the_prompt_and_keeps_stored_prompts_out_of_inline_fallbacks() {
    let item = agent("/root", "/root".into(), Some("Inspect the decoder".into()));
    assert_eq!(
        PlainSection::AgentTask.inline_text(&item),
        Some("Inspect the decoder")
    );
    let reference = ContentRef {
        id: ContentId("a".repeat(64)),
        bytes: 100,
        lines: 2,
    };
    let item = agent(
        "/root",
        "/root".into(),
        Some(TextContent::Stored(reference.clone())),
    );
    assert_eq!(PlainSection::AgentTask.inline_text(&item), None);
    assert_eq!(
        PlainSection::AgentTask.text(&item),
        Some(&TextContent::Stored(reference))
    );
}

#[test]
fn missing_native_task_text_has_an_explanation() {
    let item = agent("reviewer", "".into(), None);
    assert_eq!(
        PlainSection::AgentTask.inline_text(&item),
        Some("Task message not provided by the harness.")
    );
}

#[test]
fn empty_agent_messages_use_the_summary_without_replacing_reported_text() {
    use artisan_api::{MessageDirection, ToolKind};

    let mut item = agent("reviewer", "Review".into(), None);
    for (text, expected) in [("", "Encrypted handoff"), ("The findings", "The findings")] {
        item.kind = ItemKind::Tool {
            tool: ToolKind::AgentMessage {
                recipient: "/root/reviewer".into(),
                target: None,
                summary: Some("Encrypted handoff".into()),
                text: text.into(),
                direction: MessageDirection::Sent,
            },
            title: None,
            output: None,
            exit_code: None,
            approval: None,
        };
        assert_eq!(
            PlainSection::AgentMessage.inline_text(&item),
            Some(expected)
        );
    }
}
