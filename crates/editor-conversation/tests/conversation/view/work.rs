use super::*;
use aqua_ui::element::Text;
use artisan_api::{ContentId, ContentRef, Output, ThreadId, ToolKind};
use editor_ui::theme::ThemeMode;
use std::cell::Cell;
use std::rc::Rc;

#[test]
fn stored_failures_open_sources_only_when_expanded() {
    let mut item = Item {
        id: ItemId("tool".into()),
        thread_id: ThreadId("thread".into()),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::Failed,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::Tool {
            tool: ToolKind::Shell {
                command: "command".into(),
                cwd: None,
                background: false,
                watch: false,
            },
            title: None,
            output: Some(Output {
                text: TextContent::Stored(ContentRef {
                    id: ContentId("a".repeat(64)),
                    bytes: 20000,
                    lines: 100,
                }),
                omitted_bytes: 0,
            }),
            exit_code: None,
            approval: None,
        },
    };
    for (status, expanded, expected) in [
        (Status::Failed, false, false),
        (Status::Failed, true, true),
        (Status::Done, true, false),
    ] {
        item.status = status;
        let opened = Cell::new(false);
        activity_row(
            &item,
            expanded,
            &HashMap::new(),
            *ArtisanTheme::for_mode(ThemeMode::Dark),
            |text, _, _| Text::new(text).build(),
            Rc::new(|_| {}),
            || {
                opened.set(true);
                Ok(Text::new("loaded failure").build())
            },
        )
        .expect("activity row");
        assert_eq!(opened.get(), expected);
    }
}
