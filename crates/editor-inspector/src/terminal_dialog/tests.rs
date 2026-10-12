//! The terminal dialog against a real in-process Forge: a running shell's
//! whole output, as the reader opens it, scrolls it and closes it.

use super::{JobState, Row, TERMINAL_DIALOG_SELECTOR, TerminalDialog};
use crate::Inspector;
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, Event, ItemId, ItemKind, LiveTarget,
    ProjectId, RunId, Status, ThreadId, ToolKind,
};
use editor_conversation::Conversation;
use editor_core::hosts::{self, HostKind, HostsFile};
use editor_core::{Core, EditorFiles, Screen};
use forge_store::{NewItem, Store};
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ListOffset, Modifiers, ParentElement as _,
    Render, StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, point,
    px,
};
use std::time::{Duration, Instant};

/// A Forge on its own runtime with one project and one thread.
struct Forge {
    store: Store,
    runtime: tokio::runtime::Runtime,
    address: String,
    thread: ThreadId,
    _dir: tempfile::TempDir,
}

impl Forge {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let server = runtime
            .block_on(forge_http::start(forge_http::ServeConfig {
                data_dir: dir.path().join("forge"),
                listen: "127.0.0.1:0".parse().unwrap(),
            }))
            .unwrap();
        let address = server.local_addr().unwrap().to_string();
        let store = server.store().clone();
        runtime.spawn(async move {
            let _ = server.run().await;
        });
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
                })
                .await
                .unwrap();
        });
        Self {
            store,
            runtime,
            address,
            thread,
            _dir: dir,
        }
    }

    fn shell(&self, id: &ItemId, status: Status) {
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
                    },
                    title: None,
                    output: None,
                    exit_code: None,
                    approval: None,
                },
            }))
            .unwrap();
    }

    /// Output the running shell printed.
    fn print(&self, id: &ItemId, text: String) {
        self.store.publish_live(Event::Live {
            thread_id: self.thread.clone(),
            target: LiveTarget::Shell {
                item_id: id.clone(),
            },
            text,
            append: true,
        });
    }

    /// Pairs an Editor data dir with this Forge, as the user would.
    fn pair(&self, editor: &std::path::Path) -> (EditorFiles, editor_core::HostId) {
        let files = EditorFiles {
            dir: editor.to_path_buf(),
        };
        let identity = files.identity().unwrap();
        let invite = self
            .runtime
            .block_on(forge_http::invite(&self.store))
            .unwrap();
        let fingerprint = artisan_api::tls::fingerprint_of(&invite.forge_certificate).unwrap();
        let host = editor_core::runtime::runtime()
            .block_on(hosts::pair(
                &self.address,
                &invite.code,
                &fingerprint,
                &identity,
                "test",
                "Forge".into(),
                HostKind::Local,
            ))
            .unwrap();
        let id = host.id.clone();
        files
            .save_hosts(&HostsFile {
                hosts: vec![host],
                last: None,
            })
            .unwrap();
        (files, id)
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
        let _ = writeln!(text, "\x1b[32mline\x1b[0m {index}");
        text
    })
}

fn dialog_rows(dialog: &Entity<TerminalDialog>, cx: &mut VisualTestContext) -> usize {
    cx.read(|cx| dialog.read(cx).list.item_count())
}

/// The dialog opens on the newest output, follows what arrives while the
/// reader is at the bottom, stays put once they scroll up, swaps the cursor
/// for `^C` and a prompt when the job is stopped, and closes on escape or a
/// click outside.
#[gpui::test]
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
            core.read(cx)
                .host(&streamed_host)
                .and_then(|host| host.read(cx).live(&streamed_thread, &target).map(str::len))
                == Some(lines(0..400).len())
        })
    });

    cx.update(|window, cx| {
        inspector.update(cx, |inspector, cx| {
            inspector.open_terminal(&job, window, cx);
        });
    });
    draw(cx);
    assert!(cx.debug_bounds(TERMINAL_DIALOG_SELECTOR).is_some());
    let top = cx.read(|cx| dialog.read(cx).list.logical_scroll_top().item_ix);
    assert!(
        top > 300,
        "the dialog opens at the newest output, not line {top}"
    );
    cx.read(|cx| {
        let prompt = &dialog.read(cx).job.as_ref().expect("open").prompt.prompt;
        assert_eq!(prompt, "~/app$ ", "the prompt of the shell the job ran in");
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

    forge.shell(&job, Status::Cancelled);
    wait(cx, "the stop", |cx| {
        cx.read(|cx| {
            dialog
                .read(cx)
                .job
                .as_ref()
                .is_some_and(|job| job.state == JobState::Killed)
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
        assert_eq!(
            Row::at(job.state, dialog.shown, 2 + 420),
            Some(Row::Interrupt)
        );
        assert_eq!(Row::at(job.state, dialog.shown, 2 + 421), Some(Row::Prompt));
    });

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
