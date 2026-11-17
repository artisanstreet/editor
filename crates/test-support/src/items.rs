use artisan_api::{Item, ItemId, ItemKind, Phase, Seq, Status, TextContent, ThreadId, ToolKind};

pub fn item(id: impl Into<ItemId>, thread: impl Into<ThreadId>, seq: Seq, kind: ItemKind) -> Item {
    Item {
        id: id.into(),
        thread_id: thread.into(),
        position: seq,
        seq,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind,
    }
}

pub fn assistant_reply(text: impl Into<String>) -> ItemKind {
    ItemKind::AssistantMessage {
        text: TextContent::Inline(text.into()),
        phase: Phase::Final,
    }
}

pub fn user_message(text: impl Into<String>) -> ItemKind {
    ItemKind::UserMessage {
        text: TextContent::Inline(text.into()),
        images: Vec::new(),
        steer: false,
        from_agent: None,
        sender: None,
    }
}

pub fn read_tool(path: impl Into<String>) -> ItemKind {
    ItemKind::Tool {
        tool: ToolKind::Read {
            path: path.into(),
            range: None,
        },
        title: None,
        output: None,
        exit_code: None,
        approval: None,
    }
}
