//! The terminal dialog against a real in-process Forge: a running shell's
//! whole output, as the reader opens it, scrolls it and closes it.

#[path = "../../../editor-ui/test_support/forge.rs"]
mod forge_fixture;
#[path = "../../../editor-ui/test_support/server.rs"]
mod server_fixture;
use super::{Row, TERMINAL_DIALOG_SELECTOR, TerminalDialog};
use crate::Inspector;
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, Event, ItemId, ItemKind, LiveTarget,
    ProjectId, RunId, Status, ThreadId, ToolKind,
};
use editor_conversation::Conversation;
use editor_core::{Core, EditorFiles, Screen};
use forge_store::{NewItem, Store};
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ListOffset, Modifiers, ParentElement as _,
    Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, point,
    px,
};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// A Forge on its own runtime with one project and one thread.
struct Forge {
    store: Store,
    runtime: tokio::runtime::Runtime,
    address: String,
    thread: ThreadId,
    printed: std::cell::RefCell<std::collections::HashMap<ItemId, u64>>,
    _dir: tempfile::TempDir,
}

impl Forge {
    fn start() -> Self {
        let server_fixture::Server {
            store,
            runtime,
            address,
            directory: dir,
        } = server_fixture::start();
        let path = dir.path().join("project");
        std::fs::create_dir_all(&path).unwrap();
        let project = ProjectId::new();
        let thread = ThreadId::new();
        runtime.block_on(async {
            store
                .create_project(CreateProject {
                    id: project.clone(),
                    path: path.display().to_string(),
                    name: None,
                })
                .await
                .unwrap();
            store
                .create_thread(CreateThread {
                    id: thread.clone(),
                    project_id: project,
                    config: EngineConfig::new(EngineId::Claude),
                    title: None,
                    parent_thread_id: None,
                    spawned_by: None,
                })
                .await
                .unwrap();
        });
        Self {
            store,
            runtime,
            address,
            thread,
            printed: std::cell::RefCell::default(),
            _dir: dir,
        }
    }

    fn shell(&self, id: &ItemId, status: Status) {
        self.shell_output(id, status, None);
    }
    fn shell_output(&self, id: &ItemId, status: Status, output: Option<artisan_api::Output>) {
        self.runtime
            .block_on(self.store.upsert_item(NewItem {
                id: id.clone(),
                thread_id: self.thread.clone(),
                run_id: Some(RunId::from("run")),
                parent_id: None,
                native_id: None,
                status,
                kind: ItemKind::Tool {
                    tool: ToolKind::Shell {
                        command: "npm run dev\n# watch".into(),
                        cwd: Some("/home/me/app".into()),
                        background: true,
                        watch: false,
                    },
                    title: None,
                    output,
                    exit_code: None,
                    approval: None,
                },
            }))
            .unwrap();
    }

    /// Output the running shell printed.
    fn print(&self, id: &ItemId, text: String) {
        let total = {
            let mut printed = self.printed.borrow_mut();
            let bytes = printed.entry(id.clone()).or_default();
            *bytes += text.len() as u64;
            *bytes
        };
        self.store.publish_live(Event::Live {
            thread_id: self.thread.clone(),
            target: LiveTarget::Shell {
                item_id: id.clone(),
            },
            text,
            append: true,
            coverage: artisan_api::LiveCoverage { start: 0, total },
        });
    }

    /// Pairs an Editor data dir with this Forge, as the user would.
    fn pair(&self, editor: &std::path::Path) -> (EditorFiles, editor_core::HostId) {
        let (files, host) = forge_fixture::pair(&self.store, &self.runtime, &self.address, editor);
        (files, host.id)
    }
}

/// The inspector beside the conversation, and the dialog above both, as the
/// workspace lays them out.
struct Shell {
    inspector: Entity<Inspector>,
    conversation: Entity<Conversation>,
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .flex()
            .child(div().flex_1().child(self.conversation.clone()))
            .child(div().w(px(300.0)).child(self.inspector.clone()))
            .child(
                self.inspector
                    .read(cx)
                    .terminal_dialog()
                    .clone()
                    .cached(StyleRefinement::default().absolute().inset_0()),
            )
    }
}

fn wait(
    cx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
}

fn lines(range: std::ops::Range<usize>) -> String {
    use std::fmt::Write as _;
    range.fold(String::new(), |mut text, index| {
        writeln!(text, "\x1b[32mline\x1b[0m {index}").ok();
        text
    })
}

fn dialog_rows(dialog: &Entity<TerminalDialog>, cx: &VisualTestContext) -> usize {
    cx.read(|cx| dialog.read(cx).list.item_count())
}

/// The dialog opens on the newest output, follows what arrives while the
/// reader is at the bottom, stays put once they scroll up, reports when the
/// final output snapshot is missing, and closes on escape or a
/// click outside.
#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario from opening the dialog to closing it"
)]
fn the_terminal_dialog_shows_the_whole_output_and_follows_it(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let forge = Forge::start();
    let job = ItemId::new();
    forge.shell(&job, Status::Running);
    let dir = tempfile::tempdir().unwrap();
    let (files, host) = forge.pair(dir.path());
    let core = cx.new(|cx| Core::new(files, cx).unwrap());
    let shell_core = core.clone();
    let (shell, cx) = cx.add_window_view(|window, cx| {
        let conversation = cx.new(|cx| Conversation::new(shell_core.clone(), window, cx));
        let inspector =
            cx.new(|cx| Inspector::new(shell_core.clone(), conversation.clone(), window, cx));
        Shell {
            inspector,
            conversation,
        }
    });
    let (inspector, conversation) = cx.read(|cx| {
        (
            shell.read(cx).inspector.clone(),
            shell.read(cx).conversation.clone(),
        )
    });
    let dialog = cx.read(|cx| inspector.read(cx).terminal_dialog().clone());
    let live_host = host.clone();
    wait(cx, "the host to connect", |cx| {
        cx.read(|cx| {
            core.read(cx)
                .host(&live_host)
                .is_some_and(|host| host.read(cx).is_live())
        })
    });
    core.update(cx, |core, cx| {
        core.navigate(
            Screen::Thread {
                host: host.clone(),
                thread: forge.thread.clone(),
            },
            cx,
        );
    });
    wait(cx, "the shell item", |cx| {
        cx.read(|cx| conversation.read(cx).items().any(|item| item.id == job))
    });
    forge.print(&job, lines(0..400));
    let streamed_host = host.clone();
    let streamed_thread = forge.thread.clone();
    let target = LiveTarget::Shell {
        item_id: job.clone(),
    };
    wait(cx, "the output", |cx| {
        cx.read(|cx| {
            core.read(cx).host(&streamed_host).and_then(|host| {
                host.read(cx)
                    .live(&streamed_thread, &target)
                    .map(editor_core::Rope::len_bytes)
            }) == Some(lines(0..400).len())
        })
    });

    cx.update(|window, cx| {
        inspector.update(cx, |inspector, cx| {
            inspector.open_terminal(&job, window, cx);
        });
    });
    wait(cx, "the independent shell fetch", |cx| {
        dialog_rows(&dialog, cx) == 2 + 400 + 1
    });
    draw(cx);
    assert!(cx.debug_bounds(TERMINAL_DIALOG_SELECTOR).is_some());
    let top = cx.read(|cx| dialog.read(cx).list.logical_scroll_top().item_ix);
    assert!(
        top > 300,
        "the dialog opens at the newest output, not line {top}"
    );
    cx.read(|cx| {
        let job = dialog.read(cx).job.as_ref().expect("open");
        assert_eq!(job.cwd.as_deref(), Some("/home/me/app"));
    });

    // Two command lines, the output, and the running job's cursor.
    forge.print(&job, lines(400..410));
    wait(cx, "new output", |cx| {
        dialog_rows(&dialog, cx) == 2 + 410 + 1
    });
    draw(cx);
    assert!(cx.read(|cx| dialog.read(cx).list.is_following_tail()));

    // Scrolled up, new output leaves the reader where they are.
    cx.read(|cx| {
        dialog.read(cx).list.scroll_to(ListOffset {
            item_ix: 10,
            offset_in_item: px(0.0),
        });
    });
    draw(cx);
    forge.print(&job, lines(410..420));
    wait(cx, "more output", |cx| {
        dialog_rows(&dialog, cx) == 2 + 420 + 1
    });
    draw(cx);
    let top = cx.read(|cx| dialog.read(cx).list.logical_scroll_top().item_ix);
    assert_eq!(top, 10, "the reader stays where they scrolled to");

    forge.print(&job, "\x1b[1;1Hrewritten first line".into());
    wait(cx, "off-tail cursor rewrite", |cx| {
        cx.read(|cx| {
            dialog.read(cx).job.as_ref().is_some_and(|job| {
                job.shell
                    .screen
                    .plain_text()
                    .contains("rewritten first line")
            })
        })
    });
    forge.shell(&job, Status::Cancelled);
    wait(cx, "the stop", |cx| {
        cx.read(|cx| {
            dialog
                .read(cx)
                .job
                .as_ref()
                .is_some_and(|job| job.status == Status::Cancelled)
        })
    });
    cx.read(|cx| {
        let dialog = dialog.read(cx);
        let job = dialog.job.as_ref().expect("still open");
        assert_eq!(
            job.shell.screen.line_count(),
            420,
            "what streamed stays once the job stops"
        );
        assert_eq!(dialog.list.item_count(), 2 + 420 + 2);
        assert_eq!(Row::at(dialog.shown, 2), Some(Row::Missing));
        assert!(job.unverified);
        assert_eq!(Row::at(dialog.shown, 2 + 421), Some(Row::Status));
    });

    // The late snapshot enriches the cancelled item without reopening its terminal state.
    forge.shell_output(
        &job,
        Status::Cancelled,
        Some(artisan_api::Output::from_text(
            "authoritative final output\nmissing tail now present",
        )),
    );
    wait(cx, "authoritative final snapshot", |cx| {
        cx.read(|cx| {
            dialog.read(cx).job.as_ref().is_some_and(|job| {
                job.status == Status::Cancelled
                    && job.shell.screen.plain_text()
                        == "authoritative final output\nmissing tail now present"
            })
        })
    });
    cx.read(|cx| assert!(!dialog.read(cx).job.as_ref().unwrap().unverified));

    cx.simulate_keystrokes("escape");
    draw(cx);
    assert!(
        cx.debug_bounds(TERMINAL_DIALOG_SELECTOR).is_none(),
        "escape closes"
    );

    cx.update(|window, cx| {
        inspector.update(cx, |inspector, cx| {
            inspector.open_terminal(&job, window, cx);
        });
    });
    draw(cx);
    let card = cx
        .debug_bounds(TERMINAL_DIALOG_SELECTOR)
        .expect("open again");
    assert!(
        card.origin.x > px(100.0),
        "the card sits in the middle: {card:?}"
    );
    cx.simulate_click(point(px(4.0), px(4.0)), Modifiers::none());
    draw(cx);
    assert!(
        cx.debug_bounds(TERMINAL_DIALOG_SELECTOR).is_none(),
        "a click outside closes"
    );
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario across eviction and navigation"
)]
fn shared_thread_details_survive_transcript_eviction_and_release_on_navigation(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let forge = Forge::start();
    let goal = ItemId::new();
    let question = ItemId::new();
    let questions = vec![artisan_api::Question {
        id: "choice".into(),
        header: None,
        text: "Which option?".into(),
        multi_select: false,
        options: vec![artisan_api::QuestionOption {
            id: "first-choice".into(),
            label: "First".into(),
            description: None,
        }]
        .into(),
    }];
    let put = |id: ItemId, status, kind| {
        forge
            .runtime
            .block_on(forge.store.upsert_item(NewItem {
                id,
                thread_id: forge.thread.clone(),
                run_id: None,
                parent_id: None,
                native_id: None,
                status,
                kind,
            }))
            .unwrap();
    };
    put(
        goal.clone(),
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::Goal {
                state: "active".into(),
            },
            text: "Retain the current goal".into(),
        },
    );
    put(
        question.clone(),
        Status::AwaitingUser,
        ItemKind::Question {
            blocking: false,
            questions: questions.clone().into(),
            answer_text: None,
            answers: None,
        },
    );
    for index in 0..450 {
        put(
            ItemId::new(),
            Status::Done,
            ItemKind::AssistantMessage {
                text: format!("historical message {index}").into(),
                phase: artisan_api::Phase::Final,
            },
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let (files, host_id) = forge.pair(dir.path());
    let core = cx.new(|cx| Core::new(files, cx).unwrap());
    let shell_core = core.clone();
    let (shell, cx) = cx.add_window_view(|window, cx| {
        let conversation = cx.new(|cx| Conversation::new(shell_core.clone(), window, cx));
        let inspector =
            cx.new(|cx| Inspector::new(shell_core.clone(), conversation.clone(), window, cx));
        Shell {
            inspector,
            conversation,
        }
    });
    let (inspector, conversation) = cx.read(|cx| {
        (
            shell.read(cx).inspector.clone(),
            shell.read(cx).conversation.clone(),
        )
    });
    let host = cx.read(|cx| core.read(cx).host(&host_id).unwrap().clone());
    wait(cx, "paired Forge", |cx| {
        cx.read(|cx| host.read(cx).is_live())
    });
    core.update(cx, |core, cx| {
        core.navigate(
            Screen::Thread {
                host: host_id.clone(),
                thread: forge.thread.clone(),
            },
            cx,
        );
    });
    wait(cx, "shared current-thread details", |cx| {
        cx.read(|cx| {
            let projected = host
                .read(cx)
                .thread_summary(&forge.thread)
                .and_then(|state| state.value.as_ref());
            projected.is_some_and(|value| {
                value.goal.is_some()
                    && value.questions.len() == 1
                    && std::sync::Arc::ptr_eq(value, &inspector.read(cx).summary)
            }) && conversation.read(cx).items().next().is_some()
        })
    });
    cx.read(|cx| {
        assert!(
            !conversation
                .read(cx)
                .items()
                .any(|item| item.id == goal || item.id == question),
            "historical goal and question lie outside the latest transcript page"
        );
        let summary = host
            .read(cx)
            .thread_summary(&forge.thread)
            .unwrap()
            .value
            .as_ref()
            .unwrap();
        assert_eq!(summary.questions[0].id, question);
        assert_eq!(
            summary.goal.as_ref().unwrap().state,
            artisan_api::ThreadGoalState::Active
        );
    });
    put(
        goal.clone(),
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::Goal {
                state: "paused".into(),
            },
            text: "Retain the current goal".into(),
        },
    );
    put(
        question,
        Status::Done,
        ItemKind::Question {
            blocking: false,
            questions: questions.into(),
            answer_text: None,
            answers: Some(
                vec![Some(artisan_api::QuestionAnswer::Choices {
                    ids: vec!["first-choice".into()].into(),
                })]
                .into(),
            ),
        },
    );
    wait(cx, "coalesced goal and question updates", |cx| {
        cx.read(|cx| {
            host.read(cx)
                .thread_summary(&forge.thread)
                .and_then(|state| state.value.as_ref())
                .is_some_and(|value| {
                    value.questions.is_empty()
                        && value
                            .goal
                            .as_ref()
                            .is_some_and(|goal| goal.state == artisan_api::ThreadGoalState::Paused)
                        && std::sync::Arc::ptr_eq(value, &inspector.read(cx).summary)
                })
        })
    });
    // A goal the engine stopped on, blocked, stays on the strip.
    put(
        goal.clone(),
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::Goal {
                state: "blocked".into(),
            },
            text: "Retain the current goal".into(),
        },
    );
    wait(cx, "the blocked goal", |cx| {
        cx.read(|cx| {
            host.read(cx)
                .thread_summary(&forge.thread)
                .and_then(|state| state.value.as_ref())
                .and_then(|value| value.goal.as_ref())
                .is_some_and(|goal| goal.state == artisan_api::ThreadGoalState::Blocked)
        })
    });
    // Navigation cancels any pending refresh and releases the sole retained projection.
    put(
        goal,
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::Goal {
                state: "complete".into(),
            },
            text: "Retain the current goal".into(),
        },
    );
    core.update(cx, |core, cx| {
        core.navigate(
            Screen::NewThread {
                host: host_id,
                project: None,
            },
            cx,
        );
    });
    wait(cx, "summary lifetime ends with observation", |cx| {
        cx.read(|cx| {
            host.read(cx).thread_summary(&forge.thread).is_none()
                && inspector.read(cx).open.is_none()
        })
    });
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario that fills every section before measuring"
)]
#[expect(
    clippy::significant_drop_tightening,
    reason = "the inspector is still read inside the later update closures"
)]
fn large_inspector_sections_and_dialog_fit_small_viewport(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let forge = Forge::start();
    let put = |kind| {
        forge
            .runtime
            .block_on(forge.store.upsert_item(NewItem {
                id: ItemId::new(),
                thread_id: forge.thread.clone(),
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Running,
                kind,
            }))
            .unwrap()
    };
    for index in 0..160 {
        put(ItemKind::Agent {
            engine: None,
            thread: None,
            name: Some(format!("agent {index}")),
            description: format!("Task {index}").into(),
            prompt: None,
            agent_id: None,
            context_used: None,
            model: None,
            background: false,
            result: None,
            label: None,
        });
    }
    let steps = (0..500)
        .map(|index| artisan_api::PlanStep {
            text: format!("Checklist step {index}").into(),
            status: artisan_api::StepStatus::Pending,
        })
        .collect::<Vec<_>>();
    let plan = put(ItemKind::Plan {
        steps: steps.into(),
        proposal: None,
        approval: None,
    });
    assert!(
        matches!(
            plan.kind,
            ItemKind::Plan {
                steps: artisan_api::Collection::Stored(_),
                ..
            }
        ),
        "fixture must exercise real stored collection paging"
    );
    let command = (0..80).fold(String::new(), |mut command, index| {
        writeln!(command, "echo command-{index}").unwrap();
        command
    });
    let mut jobs = Vec::new();
    for index in 0..160 {
        jobs.push(
            put(ItemKind::Tool {
                tool: ToolKind::Shell {
                    command: if index == 0 {
                        command.clone().into()
                    } else {
                        format!("echo job-{index}").into()
                    },
                    cwd: None,
                    background: false,
                    watch: false,
                },
                title: None,
                output: None,
                exit_code: None,
                approval: None,
            })
            .id,
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let (files, host_id) = forge.pair(dir.path());
    let core = cx.new(|cx| Core::new(files, cx).unwrap());
    let shell_core = core.clone();
    let (shell, cx) = cx.add_window_view(|window, cx| {
        let conversation = cx.new(|cx| Conversation::new(shell_core.clone(), window, cx));
        let inspector =
            cx.new(|cx| Inspector::new(shell_core.clone(), conversation.clone(), window, cx));
        Shell {
            inspector,
            conversation,
        }
    });
    cx.simulate_resize(gpui::size(px(480.0), px(420.0)));
    let inspector = cx.read(|cx| shell.read(cx).inspector.clone());
    wait(cx, "paired Forge", |cx| {
        cx.read(|cx| {
            core.read(cx)
                .host(&host_id)
                .is_some_and(|host| host.read(cx).is_live())
        })
    });
    core.update(cx, |core, cx| {
        core.navigate(
            Screen::Thread {
                host: host_id,
                thread: forge.thread.clone(),
            },
            cx,
        );
    });
    wait(cx, "large canonical Inspector summary", |cx| {
        cx.read(|cx| {
            let inspector = inspector.read(cx);
            inspector.summary.agents.len() == 160
                && inspector.summary.terminals.len() == 160
                && inspector
                    .summary
                    .plan
                    .stored()
                    .is_some_and(|source| source.count == 500)
        })
    });
    draw(cx);
    let last = cx.read(|cx| inspector.read(cx).entries.len() - 1);
    inspector.update(cx, |inspector, cx| {
        inspector.test_rendered_entries.borrow_mut().clear();
        inspector.list.scroll_to(gpui::ListOffset {
            item_ix: last,
            offset_in_item: px(0.0),
        });
        cx.notify();
    });
    draw(cx);
    let selector: &'static str = format!("inspector-entry-{last}").leak();
    let final_row = cx
        .debug_bounds(selector)
        .expect("final terminal must be reachable in the native list");
    assert!(final_row.top() >= px(0.0) && final_row.bottom() <= px(420.0));
    cx.read(|cx| {
        let rendered = inspector.read(cx).test_rendered_entries.borrow();
        let unique = rendered
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        assert!(unique.contains(&last));
        assert!(
            unique.len() <= 32,
            "native Inspector must not mount all 320 records: {}",
            unique.len()
        );
    });
    // Reach the stored checklist through the same native list and activate its
    // actual button, then scroll the shared collection viewport to its end.
    let checklist = cx.read(|cx| {
        inspector
            .read(cx)
            .entries
            .iter()
            .position(|entry| matches!(entry, crate::Entry::PlanCollection))
            .unwrap()
    });
    inspector.update(cx, |inspector, cx| {
        inspector.list.scroll_to(gpui::ListOffset {
            item_ix: checklist,
            offset_in_item: px(0.0),
        });
        cx.notify();
    });
    draw(cx);
    let checklist_button = cx.debug_bounds("inspector-checklist").unwrap();
    cx.simulate_click(checklist_button.center(), Modifiers::none());
    wait(cx, "mounted stored checklist", |cx| {
        draw(cx);
        cx.read(|cx| inspector.read(cx).plan.is_some())
    });
    let panel = cx.debug_bounds("inspector-plan-panel").unwrap();
    assert!(
        panel.top() >= px(0.0) && panel.bottom() <= px(420.0),
        "collection native viewport must fit its small owner window: {panel:?}"
    );
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: point(panel.center().x, panel.bottom() - px(30.0)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-1_000_000.0))),
        ..Default::default()
    });
    wait(cx, "final stored checklist record painted", |cx| {
        draw(cx);
        cx.read(|cx| {
            inspector
                .read(cx)
                .test_plan_presented
                .borrow()
                .contains(&499)
        })
    });
    inspector.update(cx, |inspector, _| {
        inspector.test_plan_presented.borrow_mut().clear();
    });
    draw(cx);
    cx.read(|cx| {
        let presented = inspector.read(cx).test_plan_presented.borrow();
        let unique = presented
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        assert!(unique.contains(&499));
        assert!(
            unique.len() <= 32,
            "collection mounts its bounded native viewport, not 500 records"
        );
    });
    // The dialog overlays the full workspace; the command offsets are created
    // once on item admission and reused by every virtual row/frame.
    inspector.update(cx, |inspector, _| inspector.plan = None);
    cx.update(|window, cx| {
        inspector.update(cx, |inspector, cx| {
            inspector.open_terminal(&jobs[0], window, cx);
        });
    });
    let dialog = cx.read(|cx| inspector.read(cx).terminal_dialog().clone());
    wait(cx, "multiline terminal command loaded", |cx| {
        cx.read(|cx| {
            dialog
                .read(cx)
                .job
                .as_ref()
                .is_some_and(|job| job.commands.len() == 80)
        })
    });
    for _ in 0..8 {
        draw(cx);
    }
    let bounds = cx.debug_bounds(TERMINAL_DIALOG_SELECTOR).unwrap();
    assert!(
        bounds.left() >= px(0.0)
            && bounds.right() <= px(480.0)
            && bounds.top() >= px(0.0)
            && bounds.bottom() <= px(420.0),
        "terminal dialog must fit the small viewport: {bounds:?}"
    );
    assert_eq!(
        cx.read(|cx| dialog.read(cx).job.as_ref().unwrap().command_offset_builds),
        1
    );
}

/// Work nothing waits on gets its own section: a command the engine moved to
/// the background, a watcher and a workflow sit under "In the background",
/// while a command the turn waits on stays under Terminals. An agent always
/// sits under Agents, even one running in the background.
#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario with every kind of background work"
)]
fn background_work_has_its_own_section(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let forge = Forge::start();
    let put = |id: &str, kind| {
        forge
            .runtime
            .block_on(forge.store.upsert_item(NewItem {
                id: id.into(),
                thread_id: forge.thread.clone(),
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Running,
                kind,
            }))
            .unwrap();
    };
    let shell = |command: &str, background, watch| ItemKind::Tool {
        tool: ToolKind::Shell {
            command: command.into(),
            cwd: None,
            background,
            watch,
        },
        title: None,
        output: None,
        exit_code: None,
        approval: None,
    };
    put("server", shell("npm run dev", true, false));
    put("tests", shell("cargo test", false, false));
    put("watcher", shell("tail -f build.log", true, true));
    put(
        "workflow",
        ItemKind::Tool {
            tool: ToolKind::Other {
                name: "Workflow".into(),
                input: Some(r#"{"name":"review-changes"}"#.into()),
            },
            title: None,
            output: None,
            exit_code: None,
            approval: None,
        },
    );
    put(
        "crawler",
        ItemKind::Agent {
            engine: None,
            thread: None,
            name: None,
            description: "Prepare playable crawl".into(),
            prompt: None,
            agent_id: None,
            context_used: None,
            model: None,
            background: true,
            result: None,
            label: None,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (files, host) = forge.pair(dir.path());
    let core = cx.new(|cx| Core::new(files, cx).unwrap());
    let shell_core = core.clone();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let conversation = cx.new(|cx| Conversation::new(shell_core.clone(), window, cx));
        let inspector =
            cx.new(|cx| Inspector::new(shell_core.clone(), conversation.clone(), window, cx));
        Shell {
            inspector,
            conversation,
        }
    });
    let inspector = cx.read(|cx| view.read(cx).inspector.clone());
    let live_host = host.clone();
    wait(cx, "the host to connect", |cx| {
        cx.read(|cx| {
            core.read(cx)
                .host(&live_host)
                .is_some_and(|host| host.read(cx).is_live())
        })
    });
    core.update(cx, |core, cx| {
        core.navigate(
            Screen::Thread {
                host,
                thread: forge.thread.clone(),
            },
            cx,
        );
    });
    wait(cx, "the summary", |cx| {
        cx.read(|cx| {
            inspector.read(cx).summary.terminals.len() == 3
                && inspector.read(cx).summary.agents.len() == 1
        })
    });
    draw(cx);
    let sections = cx.read(|cx| {
        let inspector = inspector.read(cx);
        let summary = &inspector.summary;
        inspector
            .entries
            .iter()
            .filter_map(|entry| match entry {
                crate::Entry::Heading(heading) => Some((*heading).to_owned()),
                crate::Entry::Terminal(index) => {
                    let terminal = &summary.terminals[*index];
                    let kind = if terminal.watch { "watcher" } else { "command" };
                    Some(format!("  {kind}: {:?}", terminal.command))
                }
                crate::Entry::Workflow(index) => {
                    Some(format!("  workflow: {:?}", summary.workflows[*index].name))
                }
                crate::Entry::Agent(index) => {
                    Some(format!("  agent: {:?}", summary.agents[*index].description))
                }
                crate::Entry::Fact(_) | crate::Entry::Plan(_) | crate::Entry::PlanCollection => {
                    None
                }
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        sections,
        [
            "Agents",
            "  agent: Inline(\"Prepare playable crawl\")",
            "Terminals",
            "  command: Inline(\"cargo test\")",
            "In the background",
            "  workflow: Some(\"review-changes\")",
            "  watcher: Inline(\"tail -f build.log\")",
            "  command: Inline(\"npm run dev\")",
        ],
    );
}
