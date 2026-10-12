//! The conversation against a real in-process Forge: what the reader sees
//! when a long thread opens and scrolls back, and as items stream in.

use crate::Conversation;
use crate::fold::Row;
use crate::viewport::Input;
use artisan_api::{
    Answer, CreateProject, CreateThread, EngineConfig, EngineId, Item, ItemId, ItemKind, Phase,
    ProjectId, Question, QuestionOption, RunId, SendMessage, Status, ThreadId,
};
use editor_core::hosts::{self, HostKind, HostsFile};
use editor_core::{Core, EditorFiles, HostId, Screen};
use forge_engines::{EngineEvent, Harness, Outcome, RunIo, Turn};
use forge_store::{NewItem, Store};
use futures::future::BoxFuture;
use gpui::{AppContext as _, Entity, Focusable as _, TestAppContext, VisualTestContext};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A Forge on its own runtime with one project.
struct Forge {
    store: Store,
    runtime: tokio::runtime::Runtime,
    project: ProjectId,
    address: String,
    _dir: tempfile::TempDir,
}

impl Forge {
    fn start() -> Self {
        Self::serve(None)
    }

    /// A Forge whose runs are driven by `engine` (as an installed Claude).
    fn with_engine(engine: Arc<dyn Harness>) -> Self {
        Self::serve(Some(engine))
    }

    fn serve(engine: Option<Arc<dyn Harness>>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let config = forge_http::ServeConfig {
            data_dir: dir.path().join("forge"),
            listen: "127.0.0.1:0".parse().unwrap(),
        };
        let server = match engine {
            Some(engine) => {
                // Installed, so runs get past "not installed".
                let version = config.data_dir.join("engines/claude/versions/1.0.0");
                std::fs::create_dir_all(&version).unwrap();
                std::fs::write(version.join("claude"), "").unwrap();
                std::fs::write(config.data_dir.join("engines/claude/current"), "1.0.0").unwrap();
                runtime.block_on(forge_http::start_with_harness(
                    config,
                    Arc::new(move |_| Arc::clone(&engine)),
                ))
            }
            None => runtime.block_on(forge_http::start(config)),
        }
        .unwrap();
        // The link-title scenarios serve their pages from loopback.
        server.reach_private_links();
        let address = server.local_addr().unwrap().to_string();
        let store = server.store().clone();
        runtime.spawn(async move {
            let _ = server.run().await;
        });
        let path = dir.path().join("project");
        std::fs::create_dir_all(&path).unwrap();
        let project = ProjectId::new();
        runtime
            .block_on(store.create_project(CreateProject {
                id: project.clone(),
                path: path.display().to_string(),
                name: None,
            }))
            .unwrap();
        Self {
            store,
            runtime,
            project,
            address,
            _dir: dir,
        }
    }

    fn thread(&self) -> ThreadId {
        let id = ThreadId::new();
        self.runtime
            .block_on(self.store.create_thread(CreateThread {
                id: id.clone(),
                project_id: self.project.clone(),
                config: EngineConfig::new(EngineId::Claude),
                title: None,
                parent_thread_id: None,
            }))
            .unwrap();
        id
    }

    fn put(&self, thread: &ThreadId, id: &ItemId, status: Status, kind: ItemKind) -> Item {
        self.put_in(&RunId::from("run"), thread, id, status, kind)
    }

    /// Puts an item in `run`: each turn of a real thread is its own run.
    fn put_in(
        &self,
        run: &RunId,
        thread: &ThreadId,
        id: &ItemId,
        status: Status,
        kind: ItemKind,
    ) -> Item {
        self.runtime
            .block_on(self.store.upsert_item(NewItem {
                id: id.clone(),
                thread_id: thread.clone(),
                run_id: Some(run.clone()),
                parent_id: None,
                native_id: None,
                status,
                kind,
            }))
            .unwrap()
    }

    /// Pairs an Editor data dir with this Forge, as the user would.
    fn pair(&self, editor: &Path) -> (EditorFiles, HostId) {
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

fn reply(text: &str) -> ItemKind {
    ItemKind::AssistantMessage {
        text: text.to_owned(),
        phase: Phase::Final,
    }
}

/// Runs the app until `done` holds; network replies arrive on other threads.
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

/// An Editor connected to `forge` with the conversation in a window.
fn editor<'a>(
    forge: &Forge,
    editor_dir: &Path,
    cx: &'a mut TestAppContext,
) -> (
    Entity<Core>,
    HostId,
    Entity<Conversation>,
    &'a mut VisualTestContext,
) {
    // Forge and the network run on real threads.
    cx.executor().allow_parking();
    let (files, host) = forge.pair(editor_dir);
    let core = cx.new(|cx| Core::new(files, cx).unwrap());
    let conversation_core = core.clone();
    let (conversation, cx) =
        cx.add_window_view(|window, cx| Conversation::new(conversation_core, window, cx));
    let core_for_wait = core.clone();
    let host_for_wait = host.clone();
    wait(cx, "the host to connect", |cx| {
        cx.read(|cx| {
            core_for_wait
                .read(cx)
                .host(&host_for_wait)
                .is_some_and(|host| host.read(cx).is_live())
        })
    });
    (core, host, conversation, cx)
}

/// Paints a frame, so the transcript renders what it holds.
fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        // Deliver the frame running animations asked for, as the platform
        // would.
        window.simulate_next_frame(cx);
        window.refresh();
        window.draw(cx).clear(cx);
    });
}

fn loaded(conversation: &Entity<Conversation>, cx: &mut VisualTestContext) -> Vec<Item> {
    cx.read(|cx| conversation.read(cx).items().cloned().collect())
}

fn open(core: &Entity<Core>, host: &HostId, thread: &ThreadId, cx: &mut VisualTestContext) {
    core.update(cx, |core, cx| {
        core.navigate(
            Screen::Thread {
                host: host.clone(),
                thread: thread.clone(),
            },
            cx,
        );
    });
}

#[gpui::test]
fn scrolling_up_a_long_thread_loads_all_of_it_once(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    // 125 turns: a question and its reply each.
    let stored: Vec<Item> = (0..250)
        .map(|index| {
            let kind = if index % 2 == 0 {
                ItemKind::UserMessage {
                    text: format!("question {index}"),
                    images: Vec::new(),
                    steer: false,
                }
            } else {
                reply(&format!("reply {index}"))
            };
            forge.put(&thread, &ItemId::new(), Status::Done, kind)
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);

    open(&core, &host, &thread, cx);
    wait(cx, "the newest page", |cx| {
        !loaded(&conversation, cx).is_empty()
    });
    draw(cx);
    let first = loaded(&conversation, cx);
    assert_eq!(first, stored[150..], "opening shows the newest page");

    // The reader reaches the top edge, again and again.
    for expected in [200, 250] {
        conversation.update(cx, |conversation, cx| {
            conversation.viewport(vec![Input::TopReached], cx);
        });
        wait(cx, "an older page", |cx| {
            loaded(&conversation, cx).len() == expected
        });
    }
    conversation.update(cx, |conversation, cx| {
        conversation.viewport(vec![Input::TopReached], cx);
    });
    cx.run_until_parked();
    assert_eq!(
        loaded(&conversation, cx),
        stored,
        "the whole thread, in order, with no gaps or duplicates"
    );
    let rows = cx.read(|cx| conversation.read(cx).rows.len());
    assert_eq!(rows, 250, "one reply row per item");
}

#[gpui::test]
fn items_stream_into_the_open_thread_only(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let other = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "hi".into(),
            images: Vec::new(),
            steer: false,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 1);

    // A reply streams in: created, then updated twice.
    let elsewhere = forge.put(&other, &ItemId::new(), Status::Done, reply("not here"));
    let id = ItemId::new();
    forge.put(&thread, &id, Status::Running, reply("Hel"));
    forge.put(&thread, &id, Status::Running, reply("Hello"));
    let last = forge.put(&thread, &id, Status::Done, reply("Hello there"));
    wait(cx, "the finished reply", |cx| {
        loaded(&conversation, cx).contains(&last)
    });

    draw(cx);
    let items = loaded(&conversation, cx);
    assert_eq!(items.len(), 2, "the reply shows once: {items:#?}");
    assert!(
        !items.iter().any(|item| item.id == elsewhere.id),
        "another thread's item stays out"
    );
    let replies = cx.read(|cx| {
        conversation
            .read(cx)
            .rows
            .iter()
            .filter(|(row, _)| *row == Row::Reply(id.clone()))
            .count()
    });
    assert_eq!(replies, 1, "one row for the reply");
}

/// Asks one question with two options, then stores the answer it gets.
struct Asker;

impl Harness for Asker {
    fn run(&self, _turn: Turn, mut io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(async move {
            let question = |answers| ItemKind::Question {
                blocking: true,
                questions: vec![Question {
                    id: "colour".into(),
                    header: Some("Colour".into()),
                    text: "Which colour?".into(),
                    multi_select: false,
                    options: ["Red", "Blue"]
                        .map(|label| QuestionOption {
                            label: label.into(),
                            description: None,
                        })
                        .into(),
                }],
                answers,
            };
            io.emit(EngineEvent::Item {
                key: "ask".into(),
                parent: None,
                status: Status::AwaitingUser,
                kind: question(None),
            })
            .await;
            let Some(forge_engines::Input::Answer {
                answer: Answer::Question { answers },
                ..
            }) = io.inbox.recv().await
            else {
                return Outcome::failed("no answer");
            };
            io.emit(EngineEvent::Item {
                key: "ask".into(),
                parent: None,
                status: Status::Done,
                kind: question(Some(answers)),
            })
            .await;
            Outcome::done()
        })
    }
}

#[gpui::test]
fn a_question_is_answered_in_your_own_words_from_the_composer(cx: &mut TestAppContext) {
    let forge = Forge::with_engine(Arc::new(Asker));
    let thread = forge.thread();
    let dir = tempfile::tempdir().unwrap();
    cx.update(editor_ui::text_editor::TextEditor::bind_actions);
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    let client = cx.read(|cx| core.read(cx).host(&host).unwrap().read(cx).client.clone());
    let message = SendMessage {
        id: ItemId::new(),
        text: "pick a colour".into(),
        images: Vec::new(),
        config: None,
    };
    editor_core::runtime::runtime()
        .block_on(client.send_message(&thread, &message))
        .unwrap();

    let asked = |cx: &mut VisualTestContext| {
        loaded(&conversation, cx)
            .into_iter()
            .find(|item| matches!(item.kind, ItemKind::Question { .. }))
    };
    wait(cx, "the question", |cx| {
        asked(cx).is_some_and(|item| item.status == Status::AwaitingUser)
    });
    let question = asked(cx).unwrap().id;
    draw(cx);
    assert!(
        cx.debug_bounds("question-status-0").is_some(),
        "the transcript points to the composer: \"Answer it above the message box.\""
    );
    assert!(
        cx.debug_bounds("composer-questionnaire").is_some(),
        "the composer asks the question above the message box"
    );
    let composer = cx.read(|cx| conversation.read(cx).composer.clone().unwrap());
    cx.update(|window, cx| {
        let focus = composer.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
    cx.simulate_input("Green, please");
    cx.simulate_keystrokes("enter");

    let stored = || {
        forge
            .runtime
            .block_on(forge.store.item(&question))
            .unwrap()
            .unwrap()
    };
    wait(cx, "Forge to record the answer", |_| {
        stored().status == Status::Done
    });
    let ItemKind::Question { answers, .. } = stored().kind else {
        unreachable!()
    };
    assert_eq!(answers, Some(vec![Some(vec!["Green, please".to_owned()])]));
}

mod link_titles;

fn shell(command: &str) -> ItemKind {
    ItemKind::Tool {
        tool: artisan_api::ToolKind::Shell {
            command: command.to_owned(),
            cwd: None,
            background: false,
        },
        title: None,
        output: None,
        exit_code: Some(0),
        approval: None,
    }
}

/// Draws frames until no row is animating (an accordion has finished
/// opening, say), so what it reveals can be clicked.
fn settle(cx: &mut VisualTestContext, conversation: &Entity<Conversation>) {
    wait(cx, "the motion to settle", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation
                .read(cx)
                .row_views
                .iter()
                .all(|row| !row.read(cx).animating)
        })
    });
}

/// Clicks the middle of the element painted under `selector`.
fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is on screen"));
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    draw(cx);
}

#[gpui::test]
fn finished_work_folds_under_its_header_and_chains_open_on_click(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "build it".into(),
            images: Vec::new(),
            steer: false,
        },
    );
    forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo build"));
    forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo test"));
    forge.put(&thread, &ItemId::new(), Status::Done, reply("It builds."));
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 4);
    draw(cx);

    // The run answered, so its work sits folded under "Worked for …".
    assert!(cx.debug_bounds("work-header").is_some(), "the work header");
    assert!(
        cx.debug_bounds("work-chain").is_none(),
        "the work is folded for the reply"
    );

    // Opening the section shows one collapsed chain for both commands.
    click(cx, "work-header");
    settle(cx, &conversation);
    assert!(cx.debug_bounds("work-chain").is_some(), "the command chain");
    assert!(
        cx.debug_bounds("work-activity").is_none(),
        "a chain starts closed"
    );

    // Opening the chain lists the commands on its rail as it grows open.
    click(cx, "work-chain");
    wait(cx, "the chain's commands", |cx| {
        draw(cx);
        cx.debug_bounds("work-activity").is_some()
    });
}

/// A turn's work shows between its message and its reply, with its
/// header: an agent of the turn before, resumed by this run and stored
/// before this turn's message, stays in the turn it sits in instead of
/// pulling this run's header up there (which left the real work folded
/// away with no header). A subagent's own lines never show in the thread,
/// even when its agent row isn't loaded.
#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: two turns as Forge stored them, and the rows they make"
)]
fn a_resumed_agent_never_splits_the_turn_that_resumed_it(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let (earlier, later) = (RunId::from("earlier"), RunId::from("later"));
    let user = |text: &str| ItemKind::UserMessage {
        text: text.to_owned(),
        images: Vec::new(),
        steer: false,
    };
    let agent = |description: &str| ItemKind::Agent {
        name: None,
        description: description.to_owned(),
        prompt: None,
        agent_id: None,
        background: true,
        result: None,
        label: None,
    };
    let id = |name: &str| ItemId::from(name.to_owned());
    forge.put_in(
        &earlier,
        &thread,
        &id("ask"),
        Status::Done,
        user("Bake the lights"),
    );
    // Launched in the earlier turn, resumed (and so filed) by the later run.
    forge.put_in(
        &later,
        &thread,
        &id("thea"),
        Status::Done,
        agent("Runtime bakes"),
    );
    // A line of another agent, whose agent row is not stored here at all.
    forge
        .runtime
        .block_on(forge.store.upsert_item(NewItem {
            id: id("orphan"),
            thread_id: thread.clone(),
            run_id: Some(earlier.clone()),
            parent_id: Some(id("elsewhere")),
            native_id: None,
            status: Status::Done,
            kind: reply("Cascade 0 is sharper than the old map."),
        }))
        .unwrap();
    forge.put_in(
        &earlier,
        &thread,
        &id("baked"),
        Status::Done,
        reply("Baking is in."),
    );
    forge.put_in(
        &later,
        &thread,
        &id("how"),
        Status::Done,
        user("Okay how is it going"),
    );
    forge.put_in(
        &later,
        &thread,
        &id("move"),
        Status::Done,
        shell("git mv smoke.rs pvs/"),
    );
    forge.put_in(
        &later,
        &thread,
        &id("moved"),
        Status::Done,
        reply("The smoke code moved."),
    );

    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 7);
    let rows: Vec<String> = cx.read(|cx| {
        conversation
            .read(cx)
            .rows
            .iter()
            .map(|(row, _)| match row {
                Row::User(id) => format!("user {id}"),
                Row::Reply(id) => format!("reply {id}"),
                Row::Work {
                    run,
                    segment,
                    entries,
                } => format!(
                    "work {} #{segment}: {}",
                    run.as_ref().map(ToString::to_string).unwrap_or_default(),
                    entries
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                other => format!("{other:?}"),
            })
            .collect()
    });
    assert_eq!(
        rows,
        [
            "user ask",
            "work earlier #0: thea",
            "reply baked",
            "user how",
            "work later #0: move",
            "reply moved",
        ]
    );
}

/// The lead's messages to an agent read as talking to it: in the lead's
/// work "Talked to Martine" under Agents (one agent, however often it was
/// messaged), and in Martine's own view as messages where they arrived,
/// with a live line while she works.
#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: a lead, its agent and a message between them"
)]
fn messages_to_an_agent_show_in_its_view_and_read_as_talking_to_it(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let id = |name: &str| ItemId::from(name.to_owned());
    let put = |id: ItemId, parent: Option<ItemId>, status: Status, kind: ItemKind| {
        forge
            .runtime
            .block_on(forge.store.upsert_item(NewItem {
                id,
                thread_id: thread.clone(),
                run_id: Some(RunId::from("run")),
                parent_id: parent,
                native_id: None,
                status,
                kind,
            }))
            .unwrap()
    };
    let martine = put(
        id("martine"),
        None,
        Status::Running,
        ItemKind::Agent {
            name: None,
            description: "Close renderer gaps".into(),
            prompt: Some("Close the renderer gaps: thin geometry and glass.".into()),
            agent_id: Some("ae5992bd2a38ec98a".into()),
            background: true,
            result: None,
            label: None,
        },
    );
    let commentary = |text: &str| ItemKind::AssistantMessage {
        text: text.to_owned(),
        phase: Phase::Commentary,
    };
    put(
        id("start"),
        Some(martine.id.clone()),
        Status::Done,
        commentary("Starting with glass."),
    );
    let message = put(
        id("message"),
        None,
        Status::Done,
        ItemKind::Tool {
            tool: artisan_api::ToolKind::Other {
                name: "SendMessage".into(),
                input: Some(
                    serde_json::json!({
                        "to": "ae5992bd2a38ec98a",
                        "summary": "Use content/variants",
                        "message": "Map variants already exist: use content/variants for weather."
                    })
                    .to_string(),
                ),
            },
            title: None,
            output: None,
            exit_code: None,
            approval: None,
        },
    );
    put(
        id("switch"),
        Some(martine.id.clone()),
        Status::Done,
        commentary("Switching to the variants."),
    );
    put(
        id("build"),
        Some(martine.id.clone()),
        Status::Running,
        shell("cargo build -p render"),
    );

    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 5);

    // The lead's work: one chain, counting Martine once.
    let message_item = loaded(&conversation, cx)
        .into_iter()
        .find(|item| item.id == message.id)
        .unwrap();
    assert_eq!(
        crate::activity::category(&message_item),
        Some(crate::activity::ActivityCategory::Subagent),
        "a message to an agent counts under Agents"
    );
    let agents = HashMap::from([("ae5992bd2a38ec98a".to_owned(), "Martine".to_owned())]);
    let sent = crate::activity::agent_message(&message_item).unwrap();
    assert_eq!(crate::activity::recipient(&sent.to, &agents), "Martine");
    let lead_work = loaded(&conversation, cx);
    let chain =
        crate::activity::chain_categories(lead_work.iter().filter(|item| item.parent_id.is_none()));
    assert_eq!(
        crate::activity::chain_clause(chain.into_iter()),
        "Talked to an agent",
        "Martine started and messaged is one agent"
    );

    // Martine's view: her task, her work, the message where it arrived,
    // her work after it, and her live line (a command of hers runs).
    conversation.update(cx, |conversation, cx| {
        conversation.open_agent(Some(martine.id.clone()), cx);
    });
    wait(cx, "her view", |cx| {
        cx.read(|cx| {
            conversation
                .read(cx)
                .rows
                .iter()
                .any(|(row, _)| matches!(row, Row::AgentMessage(_)))
        })
    });
    let rows: Vec<String> = cx.read(|cx| {
        conversation
            .read(cx)
            .rows
            .iter()
            .map(|(row, _)| match row {
                Row::AgentTask(id) => format!("task {id}"),
                Row::AgentMessage(id) => format!("message {id}"),
                Row::AgentStatus(_, working) => format!("live working={working}"),
                Row::Work { entries, .. } => format!(
                    "work {}",
                    entries
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                other => format!("{other:?}"),
            })
            .collect()
    });
    assert_eq!(
        rows,
        [
            "task martine",
            "work start",
            "message message",
            "work switch build",
            "live working=true",
        ]
    );
}

/// Opening a long thread hides it under the wordmark until it stops
/// moving, and shows it in place: nothing shifts afterwards. It reads only
/// the newest turns, not the whole thread: a turn's work is one row, so
/// reading ahead by rows used to load page after page.
#[gpui::test]
fn a_long_thread_opens_in_place_without_reading_all_of_it(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let mut total = 0;
    for turn in 0..8 {
        let run = RunId::from(format!("turn-{turn}"));
        let ask = ItemKind::UserMessage {
            text: format!("Turn {turn}: build the next part"),
            images: Vec::new(),
            steer: false,
        };
        forge.put_in(&run, &thread, &ItemId::new(), Status::Done, ask);
        for step in 0..150 {
            let kind = if step % 10 == 0 {
                ItemKind::AssistantMessage {
                    text: format!("Step {step} of turn {turn}: checking the build."),
                    phase: Phase::Commentary,
                }
            } else {
                shell(&format!("cargo test -p part{step}"))
            };
            forge.put_in(&run, &thread, &ItemId::new(), Status::Done, kind);
        }
        forge.put_in(
            &run,
            &thread,
            &ItemId::new(),
            Status::Done,
            reply(
                &(0..30)
                    .map(|line| {
                        format!("Paragraph {line} of turn {turn}'s answer, long enough to wrap.")
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            ),
        );
        total += 152;
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    let state = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            (
                c.landing(),
                c.view.as_ref().is_some_and(|view| view.window().in_flight),
                {
                    let top = c.list.logical_scroll_top();
                    (top.item_ix, f32::from(top.offset_in_item))
                },
                f32::from(c.list.max_offset_for_scrollbar().y),
            )
        })
    };
    // Hidden while pages are still coming in.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        draw(cx);
        cx.run_until_parked();
        let (landing, in_flight, ..) = state(cx);
        if in_flight {
            assert!(
                landing,
                "the transcript shows while a page is still loading"
            );
        }
        if !landing {
            break;
        }
        assert!(Instant::now() < deadline, "it never landed");
        std::thread::sleep(Duration::from_millis(5));
    }
    let (_, _, top, max) = state(cx);
    for _ in 0..30 {
        draw(cx);
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
        let (_, _, now_top, now_max) = state(cx);
        assert_eq!(
            (now_top, now_max),
            (top, max),
            "the transcript moved after it showed"
        );
    }
    let loaded = loaded(&conversation, cx).len();
    assert!(
        loaded < total / 2,
        "opening read {loaded} of {total} items: more than a few screens"
    );
}

/// Jumping to a message from the menu doesn't teleport: the view lands a
/// little short of it and glides the rest quickly.
#[gpui::test]
fn a_jump_to_a_message_glides_its_last_stretch(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let mut asks = Vec::new();
    for turn in 0..12 {
        let run = RunId::from(format!("turn-{turn}"));
        let ask = forge.put_in(
            &run,
            &thread,
            &ItemId::new(),
            Status::Done,
            ItemKind::UserMessage {
                text: format!("Question {turn}"),
                images: Vec::new(),
                steer: false,
            },
        );
        asks.push(ask);
        let answer = (0..12)
            .map(|line| format!("Paragraph {line} of answer {turn}, long enough to wrap."))
            .collect::<Vec<_>>()
            .join("\n\n");
        forge.put_in(&run, &thread, &ItemId::new(), Status::Done, reply(&answer));
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 24);
    settle(cx, &conversation);
    let target = asks[1].clone();
    let top = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let top = c.list.logical_scroll_top();
            let row = c
                .rows
                .get(top.item_ix)
                .and_then(|(row, _)| row.anchor().cloned());
            (row, f32::from(top.offset_in_item))
        })
    };
    conversation.update(cx, |conversation, cx| conversation.jump_to(&target, cx));
    draw(cx);
    let shift = conversation.update(cx, |conversation, _| conversation.arrival_offset());
    assert!(
        shift < -1.0,
        "jumping up, the transcript starts a little above its place and moves down into it, not cut ({shift})"
    );
    wait(cx, "the jump to arrive on the message", |cx| {
        draw(cx);
        let offset = conversation.update(cx, |conversation, _| conversation.arrival_offset());
        offset == 0.0 && top(cx) == (Some(target.id.clone()), 0.0)
    });
}

/// A 2×2 WebP, as the composer uploads.
fn image_bytes() -> Vec<u8> {
    const WEBP: &str =
        "524946461e000000574542505650384c110000002f014000000750a82217a5ff8188e87f0000";
    (0..WEBP.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&WEBP[at..at + 2], 16).unwrap())
        .collect()
}

/// Images sent with a message show in its bubble once fetched from Forge,
/// also for a steer, which sits inside the running turn's work.
#[gpui::test]
fn images_sent_with_a_message_show_in_it(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(&thread, &ItemId::new(), Status::Done, reply("Earlier."));
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 1);
    settle(cx, &conversation);
    // Steered into the running turn: uploaded first, then the message.
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Running,
        shell("cargo build"),
    );
    let bytes = image_bytes();
    let blob = forge.runtime.block_on(forge.store.put_blob(bytes)).unwrap();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "Look at this".into(),
            images: vec![blob.hash.clone()],
            steer: true,
        },
    );
    wait(cx, "the image", |cx| {
        draw(cx);
        cx.read(|cx| {
            matches!(
                conversation.read(cx).blobs.get(&blob.hash),
                Some(crate::row::Blob::Ready(_))
            )
        })
    });
}

/// The header names what the run did, so it doesn't flip between
/// "Thinking" and "Working" each time a command ends and the model thinks.
#[test]
fn a_work_header_keeps_its_verb_while_the_run_goes_back_to_thinking() {
    use crate::phase::{Phase, section_title};
    use artisan_api::RunStatus;
    let minute = 61_000;
    for phase in [
        Phase::Thinking,
        Phase::Working,
        Phase::Replying,
        Phase::Background,
    ] {
        assert_eq!(section_title(phase, true, minute), "Working for 1m 1s");
        assert_eq!(section_title(phase, false, minute), "Thinking for 1m 1s");
    }
    assert_eq!(
        section_title(Phase::Settled(RunStatus::Done), true, minute),
        "Worked for 1m 1s"
    );
    assert_eq!(
        section_title(Phase::Settled(RunStatus::Done), false, minute),
        "Thought for 1m 1s"
    );
}

#[test]
fn chain_headers_count_what_the_work_did() {
    use crate::activity::{ActivityCategory, chain_clause};
    assert_eq!(
        chain_clause(
            [
                ActivityCategory::Command,
                ActivityCategory::Command,
                ActivityCategory::FileRead
            ]
            .into_iter()
        ),
        "Ran 2 commands, read a file"
    );
}

/// Text streams as commentary inside the live work until the run ends, so
/// the section must stay open: folding it for "replying" left a bare header.
#[test]
fn text_streaming_in_live_work_stays_on_screen() {
    use crate::fold::fold;
    use crate::thread::ThreadView;
    use crate::viewport::Fetch;
    use artisan_api::{ItemsPage, Run, RunStatus, Usage};

    let thread = ThreadId::from("thread");
    let run = RunId::from("run");
    let item = |position: u64, status: Status, kind: ItemKind| Item {
        id: ItemId::new(),
        thread_id: thread.clone(),
        position,
        seq: position,
        run_id: Some(run.clone()),
        parent_id: None,
        native_id: None,
        status,
        created_at: 0,
        updated_at: 0,
        kind,
    };
    let commentary = item(
        3,
        Status::Running,
        ItemKind::AssistantMessage {
            text: "Looking at what master did".into(),
            phase: Phase::Commentary,
        },
    );
    let items = vec![
        item(
            1,
            Status::Done,
            ItemKind::UserMessage {
                text: "fix it".into(),
                images: Vec::new(),
                steer: false,
            },
        ),
        item(2, Status::Done, shell("rg follow")),
        commentary.clone(),
    ];
    let (mut view, _) = ThreadView::open(thread.clone());
    view.apply_page(Fetch::Tail, ItemsPage { items, more: false });
    let live = Run {
        id: run.clone(),
        thread_id: thread.clone(),
        engine: EngineId::Claude,
        model: None,
        status: RunStatus::Running,
        usage: Usage::default(),
        session_id: None,
        error: None,
        seq: 3,
        started_at: 0,
        ended_at: None,
    };
    let folded = fold(&view, Some(&live), None);
    assert!(folded.runs[&run].open, "the live section is open");
    assert!(
        folded.rows.iter().any(|(row, _)| matches!(
            row,
            Row::Work { entries, .. } if entries.contains(&commentary.id)
        )),
        "the streaming text is one of the open section's rows"
    );
}

/// Background work keeps updating items while the reader sits still: the
/// transcript must not move, scrolled up or at the end.
#[gpui::test]
fn updates_while_reading_never_move_the_transcript(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "merge the branches".into(),
            images: Vec::new(),
            steer: false,
        },
    );
    let tool = ItemId::new();
    forge.put(&thread, &tool, Status::Running, shell("cargo test"));
    let long = (0..60)
        .map(|line| format!("Paragraph {line} of the summary, long enough to wrap a little."))
        .collect::<Vec<_>>()
        .join("\n\n");
    forge.put(&thread, &ItemId::new(), Status::Done, reply(&long));
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 3);
    settle(cx, &conversation);
    let offset = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            f32::from(
                conversation
                    .read(cx)
                    .list
                    .scroll_px_offset_for_scrollbar()
                    .y,
            )
        })
    };
    for scrolled_up in [false, true] {
        if scrolled_up {
            // The reader scrolls up a little with a trackpad.
            let center = cx.update(|window, _| window.viewport_size()) / 2.0;
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: gpui::point(center.width, center.height),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(300.0))),
                ..Default::default()
            });
            settle(cx, &conversation);
        }
        let before = offset(cx);
        let mut seen = Vec::new();
        for step in 0..8 {
            forge.put(
                &thread,
                &tool,
                Status::Running,
                ItemKind::Tool {
                    tool: artisan_api::ToolKind::Shell {
                        command: "cargo test".into(),
                        cwd: None,
                        background: false,
                    },
                    title: Some(format!("running test {step}")),
                    output: None,
                    exit_code: None,
                    approval: None,
                },
            );
            wait(cx, "the update", |cx| {
                loaded(&conversation, cx).iter().any(|item| {
                    item.id == tool
                        && format!("{:?}", item.kind).contains(&format!("running test {step}"))
                })
            });
            for _ in 0..4 {
                draw(cx);
                seen.push(offset(cx));
            }
        }
        assert!(
            seen.iter().all(|at| (at - before).abs() < 0.5),
            "scrolled up: {scrolled_up}; offset {before} moved: {seen:?}"
        );
    }
}

/// A mouse wheel glides the transcript one way, to the end and back, while
/// background work keeps updating it: no frame jumps back.
#[gpui::test]
fn a_wheel_glide_never_jumps_back(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let mut tool = ItemId::new();
    for turn in 0..6 {
        forge.put(
            &thread,
            &ItemId::new(),
            Status::Done,
            ItemKind::UserMessage {
                text: format!("question {turn}"),
                images: Vec::new(),
                steer: false,
            },
        );
        tool = ItemId::new();
        forge.put(&thread, &tool, Status::Running, shell("cargo test"));
        let long = (0..12)
            .map(|line| format!("Paragraph {line} of answer {turn}, long enough to wrap a little."))
            .collect::<Vec<_>>()
            .join("\n\n");
        forge.put(&thread, &ItemId::new(), Status::Done, reply(&long));
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 18);
    settle(cx, &conversation);
    // Where the reader is: the first visible row's top on screen.
    let visual = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let top = c.list.logical_scroll_top();
            let bounds = c.list.bounds_for_item(top.item_ix);
            (
                top.item_ix,
                f32::from(top.offset_in_item),
                bounds.map(|b| f32::from(b.top())),
            )
        })
    };
    let center = cx.update(|window, _| window.viewport_size()) / 2.0;
    let mut update = 0;
    for (direction, lines, ticks) in [
        ("up", 3.0, 8),
        ("down", -3.0, 20),
        ("up", 1.0, 2),
        ("down", -1.0, 4),
    ] {
        for _ in 0..ticks {
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: gpui::point(center.width, center.height),
                delta: gpui::ScrollDelta::Lines(gpui::point(0.0, lines)),
                ..Default::default()
            });
        }
        let mut seen = vec![visual(cx)];
        for frame in 0..40 {
            if frame % 5 == 0 {
                update += 1;
                let mut kind = shell("cargo test");
                if let ItemKind::Tool { title, .. } = &mut kind {
                    *title = Some(format!("update {update}"));
                }
                forge.put(&thread, &tool, Status::Running, kind);
            }
            std::thread::sleep(Duration::from_millis(8));
            draw(cx);
            cx.run_until_parked();
            seen.push(visual(cx));
        }
        // Rows move up the screen scrolling down, and down scrolling up.
        let back = seen.windows(2).any(|pair| {
            let ((ia, oa, _), (ib, ob, _)) = (pair[0], pair[1]);
            let (a, b) = ((ia, oa), (ib, ob));
            if lines > 0.0 {
                b.0 > a.0 || (b.0 == a.0 && b.1 > a.1 + 0.5)
            } else {
                b.0 < a.0 || (b.0 == a.0 && b.1 < a.1 - 0.5)
            }
        });
        assert!(!back, "scrolling {direction} jumped back: {seen:?}");
    }
}

/// Long agentic turns span many pages. Opening at the end and scrolling up
/// through them, older pages load in behind the reader: what is on screen
/// never jumps, and no gap opens below.
#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: build the thread, open it, scroll it all the way up"
)]
fn scrolling_up_through_long_turns_never_jumps(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    for turn in 0..12 {
        let run = RunId::from(format!("run-{turn}").as_str());
        forge.put_in(
            &run,
            &thread,
            &ItemId::new(),
            Status::Done,
            ItemKind::UserMessage {
                text: format!("question {turn}"),
                images: Vec::new(),
                steer: false,
            },
        );
        for step in 0..130 {
            forge.put_in(
                &run,
                &thread,
                &ItemId::new(),
                Status::Done,
                shell(&format!("step {step}")),
            );
            if step % 20 == 0 {
                forge.put_in(
                    &run,
                    &thread,
                    &ItemId::new(),
                    Status::Done,
                    ItemKind::AssistantMessage {
                        text: format!("Checking part {step} of turn {turn}."),
                        phase: Phase::Commentary,
                    },
                );
            }
        }
        let long = (0..10)
            .map(|line| format!("Paragraph {line} of answer {turn}, long enough to wrap a little."))
            .collect::<Vec<_>>()
            .join("\n\n");
        forge.put_in(&run, &thread, &ItemId::new(), Status::Done, reply(&long));
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the last turn whole", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation
                .read(cx)
                .rows
                .iter()
                .any(|(row, _)| matches!(row, Row::User(_)))
        })
    });
    settle(cx, &conversation);
    // Where each visible question and reply sits on screen: rows that
    // never merge, so their movement is what the reader sees.
    let visible = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            c.rows
                .iter()
                .enumerate()
                .filter(|(_, (row, _))| matches!(row, Row::User(_) | Row::Reply(_)))
                .filter_map(|(index, (row, _))| {
                    let at = c.list.bounds_for_item(index)?;
                    Some((row.key(), f32::from(at.top())))
                })
                .collect::<HashMap<_, _>>()
        })
    };
    let center = cx.update(|window, _| window.viewport_size()) / 2.0;
    let exhausted = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            conversation
                .read(cx)
                .view
                .as_ref()
                .is_some_and(|view| view.window().exhausted)
        })
    };
    let describe = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let top = c.list.logical_scroll_top();
            format!(
                "top {top:?} rows {} items {}",
                c.rows.len(),
                c.items().count()
            )
        })
    };
    // A trackpad moves the list by exactly what it is given, so any other
    // movement is the transcript jumping under the reader.
    let step = 40.0;
    let mut frames = 0;
    while !exhausted(cx) || frames < 60 {
        assert!(frames < 3000, "scrolling up never reached the start");
        // Hidden while it lands: nothing on screen to jump.
        let shown = |cx: &mut VisualTestContext| cx.read(|cx| !conversation.read(cx).landing());
        let before = if shown(cx) {
            visible(cx)
        } else {
            HashMap::new()
        };
        let was = describe(cx);
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(center.width, center.height),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(step))),
            ..Default::default()
        });
        draw(cx);
        cx.run_until_parked();
        draw(cx);
        frames += 1;
        if !shown(cx) {
            continue;
        }
        for (key, now) in visible(cx) {
            if let Some(at) = before.get(&key) {
                let moved = now - at;
                assert!(
                    (-0.5..=step + 0.5).contains(&moved),
                    "{key:?} jumped from {at} to {now}\nbefore {was}\nafter  {}",
                    describe(cx)
                );
            }
        }
    }
    let gap = cx.read(|cx| {
        conversation
            .read(cx)
            .rows
            .iter()
            .any(|(row, _)| matches!(row, Row::Gap))
    });
    assert!(!gap, "nothing below was dropped");
}

/// A turn woken again after its "final" text (a background notification)
/// keeps one work section: that text was commentary, not the reply.
#[test]
fn work_after_a_final_reply_stays_in_one_section() {
    use crate::fold::fold;
    use crate::thread::ThreadView;
    use crate::viewport::Fetch;
    use artisan_api::ItemsPage;

    let thread = ThreadId::from("thread");
    let run = RunId::from("run");
    let item = |position: u64, kind: ItemKind| Item {
        id: ItemId::new(),
        thread_id: thread.clone(),
        position,
        seq: position,
        run_id: Some(run.clone()),
        parent_id: None,
        native_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind,
    };
    let early = item(3, reply("Still compiling; I'll pick up when it reports."));
    let last = item(5, reply("All tests pass."));
    let items = vec![
        item(
            1,
            ItemKind::UserMessage {
                text: "fix it".into(),
                images: Vec::new(),
                steer: false,
            },
        ),
        item(2, shell("cargo test")),
        early.clone(),
        item(4, shell("cargo test")),
        last.clone(),
    ];
    let (mut view, _) = ThreadView::open(thread.clone());
    view.apply_page(Fetch::Tail, ItemsPage { items, more: false });
    let rows: Vec<Row> = fold(&view, None, None)
        .rows
        .into_iter()
        .map(|(row, _)| row)
        .collect();
    let works: Vec<&Row> = rows
        .iter()
        .filter(|row| matches!(row, Row::Work { .. }))
        .collect();
    assert_eq!(works.len(), 1, "one work section: {rows:?}");
    assert!(
        matches!(works[0], Row::Work { entries, .. } if entries.contains(&early.id)),
        "the early text sits in the work"
    );
    assert_eq!(
        rows.last(),
        Some(&Row::Reply(last.id.clone())),
        "the last text is the reply"
    );
}

/// The newest turn is longer than what loaded first: its work shows at
/// once while its start loads, not just the status line.
#[test]
fn the_newest_turn_shows_before_its_start_loads() {
    use crate::fold::fold;
    use crate::thread::ThreadView;
    use crate::viewport::Fetch;
    use artisan_api::ItemsPage;

    let thread = ThreadId::from("thread");
    let items: Vec<Item> = (1000..1100)
        .map(|position| Item {
            id: ItemId::new(),
            thread_id: thread.clone(),
            position,
            seq: position,
            run_id: Some(RunId::from("run")),
            parent_id: None,
            native_id: None,
            status: Status::Done,
            created_at: 0,
            updated_at: 0,
            kind: shell("cargo test"),
        })
        .collect();
    let (mut view, _) = ThreadView::open(thread.clone());
    view.apply_page(Fetch::Tail, ItemsPage { items, more: true });
    assert!(view.needs_turn_start(), "its start is still to load");
    let rows = fold(&view, None, None).rows;
    assert!(
        rows.iter()
            .any(|(row, _)| matches!(row, Row::Work { entries, .. } if entries.len() == 100)),
        "the loaded work shows: {rows:?}"
    );
}

/// Forge names a subagent from Artisan's bank. Its row in the work opens
/// its own view (the task it got, its work, its report) with its name in
/// the trail; closing it goes back to the thread.
#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: store a named subagent, open it, go back"
)]
fn a_subagent_opens_into_its_own_view(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "build the HUD".into(),
            images: Vec::new(),
            steer: false,
        },
    );
    let agent = ItemId::new();
    forge.put(
        &thread,
        &agent,
        Status::Running,
        ItemKind::Agent {
            name: Some("general-purpose".into()),
            description: "Match UI: HUD and in-match screens".into(),
            prompt: None,
            agent_id: None,
            background: true,
            result: None,
            label: None,
        },
    );
    forge
        .runtime
        .block_on(forge.store.upsert_item(NewItem {
            id: ItemId::new(),
            thread_id: thread.clone(),
            run_id: Some(RunId::from("run")),
            parent_id: Some(agent.clone()),
            native_id: None,
            status: Status::Done,
            kind: shell("cargo test -p client"),
        }))
        .unwrap();
    let done = forge.put(
        &thread,
        &agent,
        Status::Done,
        ItemKind::Agent {
            name: Some("general-purpose".into()),
            description: "Match UI: HUD and in-match screens".into(),
            prompt: None,
            agent_id: None,
            background: true,
            result: Some("The HUD is in.".into()),
            label: None,
        },
    );
    let ItemKind::Agent {
        label: Some(name), ..
    } = &done.kind
    else {
        panic!("Forge named the agent and kept the name: {done:?}");
    };
    assert!(artisan_api::AGENT_NAMES.contains(&name.as_str()), "{name}");
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Started the HUD agent."),
    );

    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 4);
    settle(cx, &conversation);
    // The work section is folded under its reply: open it.
    click(cx, "work-header");
    settle(cx, &conversation);
    // The agent sits in the work's chain ("talked to an agent"): open it.
    click(cx, "work-chain");
    settle(cx, &conversation);
    click(cx, "work-agent");
    settle(cx, &conversation);
    let rows = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            conversation
                .read(cx)
                .rows
                .iter()
                .map(|(row, _)| row.clone())
                .collect::<Vec<_>>()
        })
    };
    let shown = rows(cx);
    assert!(
        matches!(shown.first(), Some(Row::AgentTask(id)) if *id == agent),
        "{shown:?}"
    );
    assert!(
        matches!(shown.last(), Some(Row::AgentResult(id)) if *id == agent),
        "{shown:?}"
    );
    assert!(
        shown
            .iter()
            .any(|row| matches!(row, Row::Work { entries, .. } if entries.len() == 1)),
        "its own work: {shown:?}"
    );
    let trail = cx.read(|cx| conversation.read(cx).agent_trail());
    assert_eq!(trail, vec![(agent.clone(), name.clone())]);

    conversation.update(cx, |conversation, cx| conversation.open_agent(None, cx));
    settle(cx, &conversation);
    assert!(
        rows(cx).iter().any(|row| matches!(row, Row::User(_))),
        "back in the thread"
    );
}

/// A steer stays in the turn's work where it was sent: one work section
/// around it, folding with it, never a turn of its own.
#[test]
fn a_steer_sits_in_the_work_where_it_was_sent() {
    use crate::fold::fold;
    use crate::thread::ThreadView;
    use crate::viewport::Fetch;
    use artisan_api::ItemsPage;

    let thread = ThreadId::from("thread");
    let run = RunId::from("run");
    let item = |position: u64, kind: ItemKind| Item {
        id: ItemId::new(),
        thread_id: thread.clone(),
        position,
        seq: position,
        run_id: Some(run.clone()),
        parent_id: None,
        native_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind,
    };
    let message = |text: &str, steer: bool| ItemKind::UserMessage {
        text: text.into(),
        images: Vec::new(),
        steer,
    };
    let (before, steer, after) = (
        item(2, shell("rg subagent")),
        item(3, message("also fix the cancelled agents", true)),
        item(4, shell("cargo test")),
    );
    let items = vec![
        item(1, message("name the subagents", false)),
        before.clone(),
        steer.clone(),
        after.clone(),
        item(5, reply("Both done.")),
    ];
    let (mut view, _) = ThreadView::open(thread.clone());
    view.apply_page(Fetch::Tail, ItemsPage { items, more: false });
    let rows: Vec<Row> = fold(&view, None, None)
        .rows
        .into_iter()
        .map(|(row, _)| row)
        .collect();
    assert_eq!(
        rows.len(),
        3,
        "the message, one work section, the reply: {rows:?}"
    );
    assert!(
        matches!(&rows[1], Row::Work { entries, .. }
            if *entries == vec![before.id.clone(), steer.id.clone(), after.id.clone()]),
        "the steer between the work before and after it: {rows:?}"
    );
}
