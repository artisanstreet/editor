use super::*;
use artisan_api::{Collection, FileChange, TextContent};

fn shell(command: &str) -> ItemKind {
    ItemKind::Tool {
        tool: ToolKind::Shell {
            command: TextContent::Inline(command.to_owned()),
            cwd: None,
            background: false,
            watch: false,
        },
        title: None,
        output: None,
        exit_code: Some(0),
        approval: None,
    }
}

fn edit(paths: &[&str]) -> ItemKind {
    ItemKind::Tool {
        tool: ToolKind::Edit {
            changes: Collection::Inline(
                paths
                    .iter()
                    .map(|path| FileChange {
                        path: (*path).to_owned(),
                        op: FileOp::Modify,
                        diff: None,
                    })
                    .collect(),
            ),
        },
        title: None,
        output: None,
        exit_code: None,
        approval: None,
    }
}

fn read() -> ItemKind {
    ItemKind::Tool {
        tool: ToolKind::Read {
            path: "src/lib.rs".to_owned(),
            range: None,
        },
        title: None,
        output: None,
        exit_code: None,
        approval: None,
    }
}

#[test]
fn changes_make_a_note_due_and_looking_around_does_not() {
    let mut looking = Work::default();
    for _ in 0..40 {
        looking.finished(&read(), Status::Done);
    }
    looking.finished(&shell("cargo test"), Status::Done);
    let mut attention = Attention::default();
    attention.add(&looking);
    assert!(!attention.due(), "an investigation is not due");

    let mut changing = Work::default();
    changing.finished(&shell("sqlx migrate run"), Status::Done);
    changing.finished(&edit(&["db/schema.sql"]), Status::Done);
    changing.finished(&edit(&["src/orders/store.rs"]), Status::Done);
    let mut attention = Attention::default();
    attention.add(&changing);
    assert!(attention.due(), "a migration and edits are due");
}

#[test]
fn the_bar_backs_off_after_nothing_and_resets_after_a_note() {
    let mut work = Work::default();
    work.finished(
        &edit(&["a/x.rs", "a/y.rs", "a/z.rs", "a/w.rs"]),
        Status::Done,
    );
    let mut attention = Attention::default();
    attention.add(&work);
    assert!(attention.due());
    attention.checked(false);
    attention.add(&work);
    assert!(!attention.due(), "the same work again is no longer enough");
    attention.add(&work);
    assert!(attention.due());
    attention.checked(true);
    attention.add(&work);
    assert!(
        attention.due(),
        "after a note the bar is back to where it started"
    );
}

#[test]
fn a_title_is_read_clean() {
    let answer = Answer::parse(
        "{\"title\": \" \\\"Fix brittle generated titles.\\\" \", \"suggestion\": null}",
    );
    assert_eq!(
        answer.title.as_deref(),
        Some("Fix brittle generated titles")
    );
    assert_eq!(clean_title("  \n**Title**\nmore"), Some("Title".to_owned()));
    assert_eq!(clean_title(" \"\" "), None);
    assert_eq!(
        clean_title(&"x".repeat(200)).map(|t| t.chars().count()),
        Some(80)
    );
}

#[test]
fn the_prompt_asks_only_for_its_parts() {
    let only_title = prompt(Parts {
        suggest: false,
        note: false,
        title: true,
    });
    assert!(only_title.contains("## Title"));
    assert!(only_title.ends_with("{\"title\": string}\n"));
    let all = prompt(Parts {
        suggest: true,
        note: true,
        title: true,
    });
    assert!(all.contains("\"title\": string, \"suggestion\": string or null, \"note\""));
    let no_title = prompt(Parts {
        suggest: true,
        note: false,
        title: false,
    });
    assert!(!no_title.contains("title"));
}

#[test]
fn a_reply_is_read_wherever_its_json_sits() {
    let answer = Answer::parse(
        "```json\n{\"suggestion\": \" Run the migration \", \"note\": {\"tag\": \"Heads up\", \"text\": \" Deleting a customer deletes their orders. \"}}\n```",
    );
    assert_eq!(answer.suggestion.as_deref(), Some("Run the migration"));
    assert_eq!(
        answer.note,
        Some(Note {
            heads_up: true,
            text: "Deleting a customer deletes their orders.".to_owned(),
        })
    );
    assert_eq!(answer.title, None, "no title unless one was asked for");
    assert_eq!(Answer::parse("I can't do that."), Answer::default());
    assert_eq!(
        Answer::parse("{\"suggestion\": null, \"note\": null}"),
        Answer::default()
    );
}
