//! What the inspector shows, derived from the open thread's items and its
//! latest run. Pure: the same inputs always give the same model.

use artisan_api::{FileOp, Item, ItemId, ItemKind, Output, PlanStep, Run, Status, ToolKind};
use std::borrow::Cow;

/// Everything the model is derived from, borrowed for one render.
pub(crate) struct Sources<'a> {
    pub items: Vec<&'a Item>,
    pub run: Option<&'a Run>,
}

#[derive(Default)]
pub(crate) struct Model<'a> {
    /// Shells still running.
    pub terminals: Vec<Terminal<'a>>,
    pub agents: Vec<Agent<'a>>,
    /// The latest plan's steps.
    pub checklist: &'a [PlanStep],
    pub changed_files: Vec<ChangedFile>,
}

pub(crate) struct Terminal<'a> {
    pub id: &'a ItemId,
    pub command: &'a str,
}

pub(crate) struct Agent<'a> {
    pub id: &'a ItemId,
    /// Artisan's name for it.
    pub name: String,
    /// The task it was given.
    pub task: &'a str,
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChangedFile {
    pub path: String,
    pub op: FileOp,
    pub added: u64,
    pub removed: u64,
}

impl Model<'_> {
    /// Lines added and removed across the changed files.
    pub fn change_summary(&self) -> Option<(u64, u64)> {
        (!self.changed_files.is_empty()).then(|| {
            self.changed_files
                .iter()
                .fold((0, 0), |(added, removed), file| {
                    (added + file.added, removed + file.removed)
                })
        })
    }
}

pub(crate) fn derive<'a>(sources: &Sources<'a>) -> Model<'a> {
    let run_id = sources.run.map(|run| &run.id);
    let in_latest_run = |item: &Item| run_id.is_none() || item.run_id.as_ref() == run_id;

    let mut model = Model::default();
    let mut files: Vec<ChangedFile> = Vec::new();
    for &item in &sources.items {
        let live = !item.status.is_terminal();
        match &item.kind {
            // Being listed means it runs: finished shells leave.
            ItemKind::Tool {
                tool: ToolKind::Shell { command, .. },
                ..
            } if live => {
                model.terminals.push(Terminal {
                    id: &item.id,
                    command,
                });
            }
            ItemKind::Agent { description, .. } if live || in_latest_run(item) => {
                model.agents.push(Agent {
                    id: &item.id,
                    name: item.agent_label().unwrap_or_default(),
                    task: description,
                    status: item.status,
                });
            }
            ItemKind::Plan { steps, .. } if !steps.is_empty() => {
                model.checklist = steps;
            }
            ItemKind::Tool {
                tool: ToolKind::Edit { changes },
                ..
            } if item.status == Status::Done => {
                for change in changes {
                    let (added, removed) = change.diff.as_deref().map_or((0, 0), diff_stat);
                    match files.iter_mut().find(|file| file.path == change.path) {
                        Some(file) => {
                            file.added += added;
                            file.removed += removed;
                            // A file added in this thread stays "added" through later edits.
                            if !(file.op == FileOp::Add && change.op == FileOp::Modify) {
                                file.op = change.op.clone();
                            }
                        }
                        None => files.push(ChangedFile {
                            path: change.path.clone(),
                            op: change.op.clone(),
                            added,
                            removed,
                        }),
                    }
                }
            }
            _ => {}
        }
    }
    // Newest first.
    model.terminals.reverse();
    model.changed_files = files;
    model
}

/// A shell's raw output: the live stream while it has one, else the stored
/// head and tail.
pub(crate) fn shell_output<'a>(
    streamed: Option<&'a str>,
    stored: Option<&'a Output>,
) -> Cow<'a, str> {
    match (streamed, stored) {
        (Some(text), _) => Cow::Borrowed(text),
        (None, Some(output)) if output.truncated_bytes == 0 => Cow::Borrowed(&output.head),
        (None, Some(output)) => Cow::Owned(format!(
            "{}\n… {} bytes not kept …\n{}",
            output.head, output.truncated_bytes, output.tail
        )),
        (None, None) => Cow::Borrowed(""),
    }
}

/// Added and removed lines of a unified diff.
fn diff_stat(diff: &str) -> (u64, u64) {
    diff.lines().fold((0, 0), |(added, removed), line| {
        if line.starts_with('+') && !line.starts_with("+++") {
            (added + 1, removed)
        } else if line.starts_with('-') && !line.starts_with("---") {
            (added, removed + 1)
        } else {
            (added, removed)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_api::{FileChange, RunId, ThreadId};

    fn item(id: &str, status: Status, kind: ItemKind) -> Item {
        Item {
            id: ItemId::from(id),
            thread_id: ThreadId::from("t"),
            position: 0,
            seq: 0,
            run_id: Some(RunId::from("r")),
            parent_id: None,
            native_id: None,
            status,
            created_at: 0,
            updated_at: 0,
            kind,
        }
    }

    fn shell(
        id: &str,
        status: Status,
        command: &str,
        background: bool,
        exit_code: Option<i32>,
    ) -> Item {
        let tool = ToolKind::Shell {
            command: command.into(),
            cwd: None,
            background,
        };
        item(
            id,
            status,
            ItemKind::Tool {
                tool,
                title: None,
                output: None,
                exit_code,
                approval: None,
            },
        )
    }

    fn edit(id: &str, path: &str, diff: &str) -> Item {
        let changes = vec![FileChange {
            path: path.into(),
            op: FileOp::Modify,
            diff: Some(diff.into()),
        }];
        item(
            id,
            Status::Done,
            ItemKind::Tool {
                tool: ToolKind::Edit { changes },
                title: None,
                output: None,
                exit_code: None,
                approval: None,
            },
        )
    }

    /// A thread that runs a dev server in the background, ran a finished
    /// build and a failed test run, edited one file twice and started a
    /// subagent.
    #[test]
    fn the_inspector_shows_running_shells_changed_files_and_agents() {
        let mut agent = item(
            "agent",
            Status::Running,
            ItemKind::Agent {
                name: None,
                description: "Review the diff".into(),
                prompt: None,
                agent_id: None,
                background: false,
                result: None,
                label: Some("Maja".into()),
            },
        );
        agent.position = 5;
        let mut child = shell("child", Status::Done, "git diff", false, Some(0));
        child.parent_id = Some(ItemId::from("agent"));
        let items = [
            shell("build", Status::Done, "cargo build", false, Some(0)),
            shell("server", Status::Running, "npm run dev", true, None),
            shell("tests", Status::Failed, "npm test", true, Some(1)),
            edit(
                "e1",
                "src/app.rs",
                "--- a/src/app.rs\n+++ b/src/app.rs\n@@\n-old\n+new\n+more\n",
            ),
            edit("e2", "src/app.rs", "@@\n+again\n-gone\n-gone too\n"),
            agent,
            child,
        ];
        let sources = Sources {
            items: items.iter().collect(),
            run: None,
        };
        let model = derive(&sources);

        let terminals: Vec<_> = model.terminals.iter().map(|t| t.command).collect();
        assert_eq!(terminals, ["npm run dev"], "only what still runs is listed");

        assert_eq!(
            model.changed_files,
            [ChangedFile {
                path: "src/app.rs".into(),
                op: FileOp::Modify,
                added: 3,
                removed: 3
            }]
        );

        assert_eq!(model.change_summary(), Some((3, 3)));
        assert_eq!(model.agents.len(), 1);
        assert_eq!(
            (
                model.agents[0].name.as_str(),
                model.agents[0].task,
                model.agents[0].status
            ),
            ("Maja", "Review the diff", Status::Running)
        );
    }
}
