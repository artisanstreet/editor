//! The conversation against a real in-process Forge: what the reader sees
//! when a long thread opens and scrolls back, and as items stream in.

use crate::Conversation;
use crate::fold::Row;
use crate::thread::ItemUi;
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

    /// Explicit canonical completion for fixtures which bypass the run controller.
    fn finish_run(&self, thread: &ThreadId) {
        self.finish_run_in(&RunId::from("run"), thread);
    }

    fn finish_run_in(&self, run: &RunId, thread: &ThreadId) {
        self.runtime
            .block_on(self.store.upsert_run(artisan_api::Run {
                id: run.clone(),
                thread_id: thread.clone(),
                engine: EngineId::Claude,
                model: None,
                input_cutoff: 0,
                status: artisan_api::RunStatus::Done,
                usage: Default::default(),
                session_id: None,
                error: None,
                seq: 0,
                started_at: 0,
                ended_at: Some(1),
            }))
            .unwrap();
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
        text: text.into(),
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
                    text: format!("question {index}").into(),
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
            .filter(|(row, _)| matches!(row, Row::Reply(item) | Row::ProseBlock { id:item,block:0 } if *item == id))
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
                        .into_iter()
                        .enumerate()
                        .map(|(index, label)| QuestionOption {
                            id: format!("option-{index}"),
                            label: label.into(),
                            description: None,
                        })
                        .collect::<Vec<_>>()
                        .into(),
                }]
                .into(),
                answer_text: None,
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
                kind: question(Some(answers.into())),
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
        draft: None,
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
    wait(cx, "the shared questionnaire summary", |cx| {
        draw(cx);
        cx.debug_bounds("composer-questionnaire").is_some()
    });
    assert!(
        cx.debug_bounds("question-status").is_some(),
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
    assert_eq!(
        answers,
        Some(
            vec![Some(artisan_api::QuestionAnswer::Text {
                text: "Green, please".to_owned()
            })]
            .into()
        )
    );
}

fn shell(command: &str) -> ItemKind {
    ItemKind::Tool {
        tool: artisan_api::ToolKind::Shell {
            command: command.into(),
            cwd: None,
            background: false,
        },
        title: None,
        output: None,
        exit_code: Some(0),
        approval: None,
    }
}

/// Waits for the observed scope, visible prose and geometry before interacting.
fn settle(cx: &mut VisualTestContext, conversation: &Entity<Conversation>) {
    let mut last = None;
    let mut stable_since = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(20);
    wait(cx, "visible content and motion to settle", |cx| {
        draw(cx);
        let (ready, geometry, diagnostic) = cx.read(|cx| {
            let c = conversation.read(cx);
            let top = c.list.logical_scroll_top();
            let ready = c.disclosures.is_empty()
                && c.agent_task.is_none()
                && !c.view.as_ref().is_some_and(|view| view.window().in_flight)
                && c.row_views.iter().enumerate().all(|(index, row)| {
                    let viewport = c.list.viewport_bounds();
                    row.as_ref().is_none_or(|row| {
                        c.list.bounds_for_item(index).is_none_or(|bounds| {
                            bounds.bottom() <= viewport.top()
                                || bounds.top() >= viewport.bottom()
                                || !row.read(cx).animating
                        })
                    })
                })
                && c.rows.iter().enumerate().all(|(index, (row, _))| {
                    let viewport = c.list.viewport_bounds();
                    if c.row_views.get(index).is_none_or(Option::is_none)
                        || c.list.bounds_for_item(index).is_none_or(|bounds| {
                            bounds.bottom() <= viewport.top() || bounds.top() >= viewport.bottom()
                        })
                    {
                        return true;
                    }
                    let formatted = match row {
                        Row::Reply(id) => Some((id, "prose", 0, id.to_string())),
                        Row::ProseBlock { id, block } => {
                            Some((id, "prose", *block, id.to_string()))
                        }
                        Row::TextBlock {
                            id, section, block, ..
                        } => Some((
                            id,
                            section.slot(),
                            *block,
                            format!("{id}:{}", section.slot()),
                        )),
                        _ => None,
                    };
                    let Some((id, slot, block, inline_key)) = formatted else {
                        return true;
                    };
                    if let Some(source) = c.shared.stored_sources.borrow().get(&(id.clone(), slot))
                    {
                        let key = format!("{id}:{slot}:leaf:{block}").into();
                        c.shared
                            .markdown
                            .is_stored_leaf_prepared(&key, source, block)
                    } else {
                        c.shared
                            .markdown
                            .prepared_source(&inline_key.into())
                            .is_some()
                    }
                });
            (
                ready,
                (
                    top.item_ix,
                    f32::from(top.offset_in_item),
                    f32::from(c.list.max_offset_for_scrollbar().y),
                    c.rows.len(),
                ),
                format!(
                    "agent_loading={} page_loading={} mounted_animations={:?} visible_rows={:?}",
                    c.agent_task.is_some(),
                    c.view.as_ref().is_some_and(|view| view.window().in_flight),
                    c.row_views
                        .iter()
                        .enumerate()
                        .filter_map(|(index, row)| {
                            row.as_ref()
                                .filter(|row| row.read(cx).animating)
                                .map(|_| (index, c.rows[index].0.key()))
                        })
                        .collect::<Vec<_>>(),
                    c.rows
                        .iter()
                        .enumerate()
                        .filter_map(|(index, (row, _))| {
                            let bounds = c.list.bounds_for_item(index)?;
                            let viewport = c.list.viewport_bounds();
                            (bounds.bottom() > viewport.top() && bounds.top() < viewport.bottom())
                                .then(|| {
                                    let preparation = match row {
                                        Row::Reply(id) | Row::ProseBlock { id, .. } => {
                                            let block = match row {
                                                Row::ProseBlock { block, .. } => *block,
                                                _ => 0,
                                            };
                                            if let Some(source) = c
                                                .shared
                                                .stored_sources
                                                .borrow()
                                                .get(&(id.clone(), "prose"))
                                            {
                                                let key = format!("{id}:prose:leaf:{block}").into();
                                                format!(
                                                    "stored_leaf={} total={}",
                                                    c.shared.markdown.is_stored_leaf_prepared(
                                                        &key, source, block
                                                    ),
                                                    source.block_count()
                                                )
                                            } else {
                                                format!(
                                                    "inline={}",
                                                    c.shared
                                                        .markdown
                                                        .prepared_source(&id.to_string().into())
                                                        .is_some()
                                                )
                                            }
                                        }
                                        Row::TextBlock {
                                            id, section, block, ..
                                        } => {
                                            if let Some(source) = c
                                                .shared
                                                .stored_sources
                                                .borrow()
                                                .get(&(id.clone(), section.slot()))
                                            {
                                                let key =
                                                    format!("{id}:{}:leaf:{block}", section.slot())
                                                        .into();
                                                format!(
                                                    "stored_leaf={} total={}",
                                                    c.shared.markdown.is_stored_leaf_prepared(
                                                        &key, source, *block
                                                    ),
                                                    source.block_count()
                                                )
                                            } else {
                                                format!(
                                                    "inline={}",
                                                    c.shared
                                                        .markdown
                                                        .prepared_source(
                                                            &format!("{id}:{}", section.slot())
                                                                .into()
                                                        )
                                                        .is_some()
                                                )
                                            }
                                        }
                                        _ => "literal or structural".to_owned(),
                                    };
                                    (index, row.clone(), bounds, preparation)
                                })
                        })
                        .collect::<Vec<_>>(),
                ),
            )
        });
        assert!(
            ready || Instant::now() < deadline,
            "content did not settle: {diagnostic}"
        );
        if !ready || last != Some(geometry) {
            stable_since = Instant::now();
            last = Some(geometry);
        }
        ready && stable_since.elapsed() >= Duration::from_millis(300)
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
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        shell("python3 - <<'PY'\nfrom pathlib import Path\nprint(Path.cwd())\nPY"),
    );
    forge.put(&thread, &ItemId::new(), Status::Done, reply("It builds."));
    forge.finish_run(&thread);
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
    settle(cx, &conversation);
    let activity = cx.debug_bounds("work-activity").unwrap();
    assert!(
        activity.size.height <= gpui::px(28.0),
        "a multiline command still occupies one compact summary row: {activity:?}"
    );
    click(cx, "work-chain");
    wait(cx, "the commands collapsed back to one header", |cx| {
        draw(cx);
        cx.debug_bounds("work-chain").is_some()
            && cx.debug_bounds("work-activity").is_none()
            && cx.read(|cx| conversation.read(cx).disclosures.is_empty())
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
        text: text.into(),
        images: Vec::new(),
        steer: false,
    };
    let agent = |description: &str| ItemKind::Agent {
        name: None,
        description: description.into(),
        prompt: None,
        agent_id: None,
        context_used: None,
        model: None,
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
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 6);
    let rows: Vec<String> = cx.read(|cx| {
        let c = conversation.read(cx);
        // This asserts canonical ordering, independently of native leaf subdivision.
        crate::fold::fold(
            c.view.as_ref().unwrap(),
            c.run(cx).as_ref(),
            c.agent.as_ref(),
        )
        .rows
        .iter()
        .map(|(row, _)| match row {
            Row::User(id) => format!("user {id}"),
            Row::Reply(id) | Row::ProseBlock { id, block: 0 } => format!("reply {id}"),
            Row::Work {
                run,
                segment,
                entries,
                ..
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
            context_used: None,
            model: None,
            background: true,
            result: None,
            label: None,
        },
    );
    let commentary = |text: &str| ItemKind::AssistantMessage {
        text: text.into(),
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
            tool: artisan_api::ToolKind::AgentMessage {
                recipient: "ae5992bd2a38ec98a".into(),
                recipient_label: "Martine".into(),
                summary: Some("Use content/variants".into()),
                text: "Map variants already exist: use content/variants for weather.".into(),
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
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 2);

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
    let sent = crate::activity::agent_message(&message_item).unwrap();
    assert_eq!(sent.label, "Martine");
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
            conversation.read(cx).rows.iter().any(|(row, _)| {
                matches!(
                    row,
                    Row::PlainBlock {
                        section: crate::fold::PlainSection::AgentMessage,
                        ..
                    }
                )
            })
        })
    });
    let rows: Vec<String> = cx.read(|cx| {
        let c = conversation.read(cx);
        // This asserts canonical ordering, independently of native leaf subdivision.
        crate::fold::fold(
            c.view.as_ref().unwrap(),
            c.run(cx).as_ref(),
            c.agent.as_ref(),
        )
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
            text: format!("Turn {turn}: build the next part").into(),
            images: Vec::new(),
            steer: false,
        };
        forge.put_in(&run, &thread, &ItemId::new(), Status::Done, ask);
        for step in 0..150 {
            let kind = if step % 10 == 0 {
                ItemKind::AssistantMessage {
                    text: format!("Step {step} of turn {turn}: checking the build.").into(),
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
    settle(cx, &conversation);
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
                text: format!("Question {turn}").into(),
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
    let message = forge.put(
        &thread,
        &ItemId::from("message-with-image"),
        Status::Done,
        ItemKind::UserMessage {
            text: "Look at this".into(),
            images: vec![blob.hash.clone()],
            steer: true,
        },
    );
    wait(cx, "the steer", |cx| {
        loaded(&conversation, cx)
            .iter()
            .any(|item| item.id == message.id)
    });
    // The steer is inside a collapsed work disclosure. Hidden attachments must
    // not start image downloads; opening the disclosure paints the message.
    conversation.update(cx, |conversation, cx| {
        conversation.toggle_work(&RunId::from("run"), true, cx)
    });
    wait(cx, "the visible image", |cx| {
        draw(cx);
        if cx.debug_bounds("sent-image-message-with-image-0").is_none() {
            return false;
        }
        cx.read(|cx| {
            matches!(
                core.read(cx)
                    .host(&host)
                    .unwrap()
                    .read(cx)
                    .image(&blob.hash),
                Some(crate::row::Blob::Ready(_))
            )
        })
    });
}

/// Only what arrives while the reader watches animates in. Opening a
/// thread whose run is live, items that come over the live stream while it
/// lands are history: they appear at once. One arriving after it landed
/// streams in.
#[gpui::test]
fn only_what_arrives_while_watched_animates_in(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    for turn in 0..3 {
        forge.put(
            &thread,
            &ItemId::new(),
            Status::Done,
            reply(&format!("Answer {turn}.")),
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    // Live items race the first page.
    for step in 0..5 {
        forge.put(
            &thread,
            &ItemId::new(),
            Status::Done,
            reply(&format!("Live {step}.")),
        );
    }
    let animating = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            conversation
                .read(cx)
                .row_views
                .iter()
                .flatten()
                .any(|row| row.read(cx).animating)
        })
    };
    wait(cx, "the thread to land", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            !c.landing() && c.items().count() == 8
        })
    });
    draw(cx);
    let fresh = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let view = c.view.as_ref().unwrap();
            view.items
                .values()
                .filter(|item| view.ui.get(&item.id).is_some_and(ItemUi::fresh))
                .count()
        })
    };
    assert_eq!(
        fresh(cx),
        0,
        "what arrived while it opened would animate in"
    );
    assert!(!animating(cx), "what arrived while it opened animated in");
    cx.update(|window, _| window.activate_window());
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Arrived while watched, word by word."),
    );
    wait(cx, "the new reply", |cx| {
        draw(cx);
        cx.read(|cx| conversation.read(cx).items().count() == 9)
    });
    settle(cx, &conversation);
    assert_eq!(
        fresh(cx),
        1,
        "only the reply that arrived while watched is new"
    );
}

/// Reduced motion and content without prose must release the next native row.
#[gpui::test]
fn fresh_non_prose_content_never_blocks_later_replies(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Initial reply."),
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the initial reply", |cx| {
        draw(cx);
        loaded(&conversation, cx).len() == 1
    });
    settle(cx, &conversation);
    cx.update(|window, _| window.activate_window());
    for reduced in [true, false] {
        cx.update(|_, cx| cx.set_reduce_motion(reduced));
        let separator = ItemId::new();
        let next = ItemId::new();
        forge.put(&thread, &separator, Status::Done, reply("---"));
        forge.put(
            &thread,
            &next,
            Status::Done,
            reply("This reply must remain visible."),
        );
        wait(cx, "the reply after non-prose content", |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                c.rows.iter().enumerate().any(|(index, (row, _))| {
                    row.anchor() == Some(&next)
                        && c.row_views.get(index).is_some_and(Option::is_some)
                        && c.list
                            .bounds_for_item(index)
                            .is_some_and(|bounds| bounds.size.height > gpui::px(0.0))
                })
            })
        });
        cx.read(|cx| {
            assert!(
                conversation
                    .read(cx)
                    .view
                    .as_ref()
                    .unwrap()
                    .ui
                    .get(&separator)
                    .is_some_and(|ui| ui.arrived.is_some()),
                "the separator must arrive while watched"
            );
        });
        settle(cx, &conversation);
    }
}

/// The context meter reads the open agent's own context and model: an
/// agent's view showed the lead's.
#[gpui::test]
fn an_agents_view_reads_its_own_context(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let jenny = forge.put(
        &thread,
        &ItemId::new(),
        Status::Running,
        ItemKind::Agent {
            name: None,
            description: "Perf overlay".into(),
            prompt: None,
            agent_id: None,
            context_used: Some(91_464),
            model: Some("claude-haiku-5-5".into()),
            background: true,
            result: None,
            label: None,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 1);
    let reading = |cx: &mut VisualTestContext| cx.read(|cx| conversation.read(cx).agent_context());
    assert_eq!(reading(cx), None, "the thread itself reads the lead's run");
    conversation.update(cx, |conversation, cx| {
        conversation.open_agent(Some(jenny.id.clone()), cx);
    });
    assert_eq!(
        reading(cx),
        Some(Some((91_464, Some("claude-haiku-5-5".to_owned())))),
        "the agent's view reads its own context"
    );
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
    view.apply_page(
        Fetch::Tail,
        ItemsPage {
            items,
            runs: Vec::new(),
            more: false,
        },
    );
    let live = Run {
        id: run.clone(),
        thread_id: thread.clone(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
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
                text: format!("question {turn}").into(),
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
                text: format!("question {turn}").into(),
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
                        text: format!("Checking part {step} of turn {turn}.").into(),
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
        forge.finish_run_in(&run, &thread);
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the bounded latest page", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation.read(cx).view.as_ref().is_some_and(|view| {
                !view.items.is_empty() && !view.window().in_flight && !view.window().exhausted
            })
        })
    });
    assert!(
        loaded(&conversation, cx).len() <= 200,
        "opening must not backfill a whole turn"
    );
    settle(cx, &conversation);
    // Where each visible question and reply sits on screen: rows that
    // never merge, so their movement is what the reader sees.
    let visible = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            c.rows
                .iter()
                .enumerate()
                .filter(|(_, (row, _))| {
                    matches!(
                        row,
                        Row::User(_)
                            | Row::PlainBlock {
                                section: crate::fold::PlainSection::User,
                                block: 0,
                                ..
                            }
                            | Row::Reply(_)
                            | Row::ProseBlock { block: 0, .. }
                    )
                })
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
    assert!(gap, "the bounded history window exposes its newer boundary");
    assert!(
        loaded(&conversation, cx).len() <= 800,
        "history loading retains a bounded item window"
    );
    conversation.update(cx, |c, cx| c.jump_to_latest(cx));
    wait(cx, "return to the latest bounded page", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            !c.view.as_ref().is_some_and(|view| view.window().in_flight)
                && c.rows.iter().all(|(row, _)| !matches!(row, Row::Gap))
                && c.items().count() <= 200
        })
    });
}

/// Later tools cannot reclassify an already canonical final message.
#[test]
fn canonical_final_messages_keep_their_phase_when_later_work_arrives() {
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
    let early = item(3, reply("The first result is available."));
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
    view.apply_page(
        Fetch::Tail,
        ItemsPage {
            items,
            runs: Vec::new(),
            more: false,
        },
    );
    let rows: Vec<Row> = fold(&view, None, None)
        .rows
        .into_iter()
        .map(|(row, _)| row)
        .collect();
    let works: Vec<&Row> = rows
        .iter()
        .filter(|row| matches!(row, Row::Work { .. }))
        .collect();
    assert_eq!(
        works.len(),
        2,
        "two work stretches around the first reply: {rows:?}"
    );
    assert_eq!(rows[2], Row::Reply(early.id.clone()));
    assert!(
        works
            .iter()
            .all(|row| matches!(row, Row::Work { entries, .. } if !entries.contains(&early.id)))
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
    view.apply_page(
        Fetch::Tail,
        ItemsPage {
            items,
            runs: Vec::new(),
            more: true,
        },
    );
    assert_eq!(
        view.shown().count(),
        100,
        "partial turns are usable immediately"
    );
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
            context_used: None,
            model: None,
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
            context_used: None,
            model: None,
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
    forge.finish_run(&thread);

    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 3);
    settle(cx, &conversation);
    // The work section is folded under its reply: open it.
    click(cx, "work-header");
    settle(cx, &conversation);
    // The agent sits in the work's chain ("talked to an agent"): open it.
    click(cx, "work-chain");
    settle(cx, &conversation);
    click(cx, "work-agent");
    settle(cx, &conversation);
    assert!(
        cx.read(|cx| conversation.read(cx).agent_task.is_none()),
        "completed agent ancestry must release its loading task"
    );
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
        matches!(shown.first(), Some(Row::PlainBlock {id, section: crate::fold::PlainSection::AgentTask, block: 0}) if *id == agent),
        "{shown:?}"
    );
    assert!(
        matches!(shown.last(), Some(Row::TextBlock {id, section: crate::fold::TextSection::AgentResult, ..}) if *id == agent),
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
        rows(cx).iter().any(|row| matches!(
            row,
            Row::PlainBlock {
                section: crate::fold::PlainSection::User,
                block: 0,
                ..
            }
        )),
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
    view.apply_page(
        Fetch::Tail,
        ItemsPage {
            items,
            runs: Vec::new(),
            more: false,
        },
    );
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

#[test]
fn canonical_tool_commands_preserve_quoted_whitespace() {
    let command = "bash -lc \"printf 'a   b'\"";
    let item = Item {
        id: ItemId::new(),
        thread_id: ThreadId::new(),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        native_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: shell(command),
    };
    assert_eq!(
        crate::activity::detail_text(&item, crate::activity::ActivityCategory::Command),
        command
    );
}

/// A visible native disclosure clips original glyph rows and reverses in place.
#[gpui::test]
fn visible_work_disclosure_paints_intermediate_native_heights(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let run = RunId::from("short-commentary");
    let id = ItemId::new();
    forge.put_in(
        &run,
        &thread,
        &id,
        Status::Done,
        ItemKind::AssistantMessage {
            text: "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.".into(),
            phase: Phase::Commentary,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the short commentary", |cx| {
        loaded(&conversation, cx).len() == 1
    });
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    settle(cx, &conversation);
    let intrinsic = cx.read(|cx| conversation.read(cx).shared.row_heights.borrow().clone());
    conversation.update(cx, |c, cx| c.toggle_work(&run, false, cx));
    draw(cx);
    let partial_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let partially_clipped = cx.read(|cx| {
            let c = conversation.read(cx);
            intrinsic.iter().any(|(key, full)| {
                let Some(index) = c.row_context.key_rows.get(key).copied() else {
                    return false;
                };
                if !matches!(c.rows[index].0, Row::ProseBlock { .. }) {
                    return false;
                }
                let Some(bounds) = c.list.bounds_for_item(index) else {
                    return false;
                };
                let end = if index + 1 == c.rows.len() {
                    gpui::px(c.row_context.end_space)
                } else {
                    gpui::px(0.0)
                };
                let visible = bounds.size.height - end;
                visible > gpui::px(0.5)
                    && visible < *full - gpui::px(0.5)
                    && c.shared.row_heights.borrow().get(key) == Some(full)
            })
        });
        if partially_clipped {
            break;
        }
        assert!(
            Instant::now() < partial_deadline,
            "the native disclosure never painted a partially clipped original row"
        );
        std::thread::sleep(Duration::from_millis(10));
        draw(cx);
    }
    assert!(
        cx.debug_bounds("work-header").is_some(),
        "the disclosure header must remain mounted during the flight"
    );
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    settle(cx, &conversation);
    cx.read(|cx| {
        let c = conversation.read(cx);
        assert!(
            c.rows
                .iter()
                .any(|(row, _)| matches!(row, Row::ProseBlock { id: item, .. } if *item == id))
        );
    });
}

/// A work disclosure never mounts a whole commentary, list or table subtree.
#[gpui::test]
fn expanded_work_mounts_only_visible_semantic_leaves(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let run = RunId::from("long-commentary");
    let id = ItemId::new();
    let text = format!(
        "{}\n\n{}\n\n| column | value |\n| --- | --- |\n{}",
        (0..250)
            .map(|n| format!("Paragraph {n} explains a separate part of the change."))
            .collect::<Vec<_>>()
            .join("\n\n"),
        (0..250)
            .map(|n| format!("- Checklist entry {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
        (0..250)
            .map(|n| format!("| row {n} | value {n} | "))
            .collect::<Vec<_>>()
            .join("\n")
    );
    forge.put_in(
        &run,
        &thread,
        &id,
        Status::Done,
        ItemKind::AssistantMessage {
            text: text.into(),
            phase: Phase::Commentary,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "commentary item", |cx| {
        loaded(&conversation, cx).len() == 1
    });
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    wait(cx, "flat commentary leaves", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            let sources = c.shared.stored_sources.borrow();
            sources.get(&(id.clone(), "prose")).is_some_and(|source| {
                source.block_count() == 751
                    && source.loaded_range().len() <= 64
                    && !source.loaded_range().is_empty()
                    && c.rows.iter().filter(|(row, _)| matches!(row, Row::ProseBlock { id: item, .. } if *item == id)).count() == source.loaded_range().len()
            })
        })
    });
    settle(cx, &conversation);
    cx.read(|cx| {
        let c = conversation.read(cx);
        assert_eq!(
            c.shared.stored_sources.borrow()[&(id.clone(), "prose")].block_count(),
            751
        );
        assert!(
            c.rows.len() <= 65,
            "one header and at most64 registered semantic leaves"
        );
        let mounted = c.row_views.iter().flatten().count();
        assert!(
            mounted < 128,
            "only viewport and bounded cache mount views: {mounted}/{}",
            c.rows.len()
        );
        // A header contains metadata, not every child's model or rendered element.
        assert!(
            c.rows
                .iter()
                .filter(|(row, _)| matches!(row, Row::Work { .. }))
                .count()
                == 1
        );
    });
    // Capture already shaped native body heights. The flight must clip those
    // original rows; it must not reshape a growing text prefix or compress all
    // children at once.
    let source_window = cx.read(|cx| {
        conversation.read(cx).shared.stored_sources.borrow()[&(id.clone(), "prose")].loaded_range()
    });
    conversation.update(cx, |c, cx| c.toggle_work(&run, false, cx));
    draw(cx);
    cx.read(|cx| {
        let c = conversation.read(cx);
        assert!(
            !c.disclosures.is_empty(),
            "closing retains its native owners during the flight"
        );
        assert!(
            c.rows.len() > 1 && c.rows.len() <= 129,
            "the closing window stays bounded"
        );
    });
    // Reversing a live native flight keeps the source window and header rather
    // than dropping/reloading the descendant models and starting from zero.
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    draw(cx);
    cx.read(|cx| {
        let c = conversation.read(cx);
        assert!(!c.disclosures.is_empty(), "the interrupted flight reverses");
        assert_eq!(
            c.shared.stored_sources.borrow()[&(id.clone(), "prose")].loaded_range(),
            source_window
        );
        assert!(c.row_views.iter().flatten().count() <= 128);
    });
    wait(cx, "the reopened native flight", |cx| {
        draw(cx);
        cx.read(|cx| conversation.read(cx).disclosures.is_empty())
    });
    settle(cx, &conversation);
    conversation.update(cx, |c, cx| c.toggle_work(&run, false, cx));
    wait(cx, "closing to release descendant native owners", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            c.disclosures.is_empty() && c.rows.len() == 1
        })
    });
    cx.read(|cx| {
        let c = conversation.read(cx);
        assert!(matches!(c.rows[0].0, Row::Work { .. }));
        assert!(c.row_views.iter().flatten().count() <= 1);
        assert!(
            c.shared
                .row_heights
                .borrow()
                .keys()
                .all(|key| *key == c.rows[0].0.key()),
            "descendant geometry ownership is released with its native row"
        );
    });
}

/// Finishing preparation above the reader retains the native visible row and its pixel offset.
#[gpui::test]
fn background_work_preparation_preserves_the_reading_anchor(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let run = RunId::from("prepare-above-reader");
    let earlier = ItemId::new();
    forge.put_in(
        &run,
        &thread,
        &earlier,
        Status::Done,
        ItemKind::AssistantMessage {
            text: "Earlier commentary.".into(),
            phase: Phase::Commentary,
        },
    );
    let later = ItemId::new();
    let long = (0..100)
        .map(|n| format!("Visible paragraph {n}, enough content to scroll away from the tail."))
        .collect::<Vec<_>>()
        .join("\n\n");
    forge.put_in(
        &run,
        &thread,
        &later,
        Status::Done,
        ItemKind::AssistantMessage {
            text: long.into(),
            phase: Phase::Commentary,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "both commentary items", |cx| {
        loaded(&conversation, cx).len() == 2
    });
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    settle(cx, &conversation);
    let center = cx.update(|window, _| window.viewport_size()) / 2.0;
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: gpui::point(center.width, center.height),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(300.0))),
        ..Default::default()
    });
    settle(cx, &conversation);
    let anchor = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let top = c.list.logical_scroll_top();
            (c.rows[top.item_ix].0.key(), f32::from(top.offset_in_item))
        })
    };
    let before = anchor(cx);
    assert!(matches!(&before.0, crate::fold::RowKey::ProseBlock(id, _) if *id == later));
    let expanded: Arc<str> = Arc::from(
        (0..300)
            .map(|n| format!("Earlier background paragraph {n}."))
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    forge.put_in(
        &run,
        &thread,
        &earlier,
        Status::Done,
        ItemKind::AssistantMessage {
            text: expanded.to_string().into(),
            phase: Phase::Commentary,
        },
    );
    wait(cx, "expanded offscreen item revision", |cx| {
        loaded(&conversation, cx).iter().any(|item| item.id == earlier && matches!(&item.kind, ItemKind::AssistantMessage { text, .. } if text.inline() == Some(expanded.as_ref())))
    });
    conversation.update(cx, |c, cx| {
        let owner = cx.entity().downgrade();
        let _ =
            c.shared
                .markdown
                .prepare_source(earlier.to_string().into(), expanded, cx, move |cx| {
                    let _ = owner.update(cx, Conversation::rebuild);
                });
    });
    wait(cx, "offscreen preparation publication", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation
                .read(cx)
                .rows
                .iter()
                .filter(|(row, _)| matches!(row, Row::ProseBlock { id, .. } if *id == earlier))
                .count()
                == 300
        })
    });
    settle(cx, &conversation);
    let after = anchor(cx);
    assert_eq!(
        after.0, before.0,
        "same visible native row across preparation"
    );
    assert!(
        (after.1 - before.1).abs() < 0.5,
        "pixel anchor moved: {before:?} -> {after:?}"
    );
}

#[test]
fn plain_native_partitions_preserve_utf8_and_whitespace() {
    for text in [
        "".to_owned(),
        "界🙂".repeat(500),
        "first\n\n  second\t🙂 third ".repeat(200),
    ] {
        let ranges = crate::fold::plain_ranges(&text);
        assert!(ranges.iter().all(|range| range.len() <= 1024));
        assert!(ranges.iter().all(|range| text.is_char_boundary(range.start) && text.is_char_boundary(range.end)));
        let rebuilt = ranges
            .iter()
            .map(|range| &text[range.clone()])
            .collect::<String>();
        assert_eq!(
            rebuilt, text,
            "literal source never drops whitespace at native boundaries"
        );
    }
}

#[gpui::test]
fn long_plain_messages_are_native_leaves_with_final_leaf_read_evidence(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let user = ItemId::new();
    let error = ItemId::new();
    let user_text = "A long literal user message with UTF8 界🙂.\n".repeat(8000);
    forge.put(
        &thread,
        &user,
        Status::Done,
        ItemKind::UserMessage {
            text: user_text.into(),
            images: Vec::new(),
            steer: false,
        },
    );
    forge.put(
        &thread,
        &error,
        Status::Failed,
        ItemKind::Error {
            message: "A long error explaining the failed operation.\n"
                .repeat(8000)
                .into(),
            kind: artisan_api::ErrorKind::Execution,
            retryable: false,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "literal items arrive", |cx| {
        loaded(&conversation, cx).iter().any(|item| item.id == user)
            && loaded(&conversation, cx)
                .iter()
                .any(|item| item.id == error)
    });
    for (id, section) in [
        (&user, crate::fold::PlainSection::User),
        (&error, crate::fold::PlainSection::Error),
    ] {
        // Use the production navigation intent. A raw native scroll alone
        // leaves Following active and the next frame correctly returns to tail.
        conversation.update(cx, |c, cx| {
            if section == crate::fold::PlainSection::Error {
                // The error is the newest Item. Navigate to the latest thread
                // tail, rather than its first leaf where older-page prefetch
                // correctly starts before this test can inspect the source.
                c.jump_to_latest(cx);
            } else {
                let item = c.view.as_ref().unwrap().item(id).unwrap().clone();
                c.jump_to(&item, cx);
            }
        });
        conversation.update(cx, |c, _| {
            let index = c
                .rows
                .iter()
                .rposition(|(row, _)| row.anchor() == Some(id))
                .unwrap();
            c.list.scroll_to(gpui::ListOffset {
                item_ix: index,
                offset_in_item: gpui::px(0.0),
            });
        });
        wait(cx, "latest literal source page", |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                let sources = c.shared.stored_sources.borrow();
                let Some(source) = sources.get(&(id.clone(), section.slot())) else { return false; };
                let Some(item) = c.view.as_ref().and_then(|view| view.item(id)) else { return false; };
                source.format() == artisan_api::SourceIndexFormat::Literal

                    && section.text(item).is_some_and(|text| matches!(text,
                        artisan_api::TextContent::Stored(reference) if reference == source.source_ref()))
                    && source.block_count() == 8001
                    && source.loaded_range().end == source.block_count()
            })
        });
        conversation.update(cx, |c, _| {
            let index = c
                .rows
                .iter()
                .rposition(|(row, _)| row.anchor() == Some(id))
                .unwrap();
            c.list.scroll_to(gpui::ListOffset {
                item_ix: index,
                offset_in_item: gpui::px(0.0),
            });
        });
        wait(cx, "actual final literal body ready", |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                let row = c
                    .rows
                    .iter()
                    .rev()
                    .find(|(row, _)| row.anchor() == Some(id))
                    .unwrap();
                c.read_evidence(&row.0, cx)
                    == c.view.as_ref().unwrap().item(id).map(|item| item.seq)
            })
        });
        cx.read(|cx| {
            let c = conversation.read(cx);
            let sources = c.shared.stored_sources.borrow();
            let source = sources.get(&(id.clone(), section.slot())).unwrap();
            assert!(
                source.block_count() > 600,
                "original semantic source is never truncated"
            );
            let leaves: Vec<_> = c
                .rows
                .iter()
                .map(|(row, _)| row)
                .filter(|row| matches!(row, Row::PlainBlock { id: row_id, .. } if row_id == id))
                .collect();
            assert_eq!(leaves.len(), source.loaded_range().len());
            assert!(
                leaves.len() <= 64,
                "only the source page enters the native list"
            );
            assert_eq!(
                c.read_evidence(
                    &Row::PlainBlock {
                        id: id.clone(),
                        section,
                        block: 0
                    },
                    cx
                ),
                None,
                "the original first ordinal cannot acknowledge the entire source"
            );
            assert_eq!(
                c.read_evidence(leaves[0], cx),
                None,
                "an early current-page ordinal cannot acknowledge the entire source"
            );
            assert_eq!(
                c.read_evidence(leaves[leaves.len() - 1], cx),
                c.view.as_ref().unwrap().item(id).map(|item| item.seq)
            );
            assert!(
                c.row_views.iter().flatten().count() <= 128,
                "viewport and bounded native cache own mounted views"
            );
            assert!(
                c.rows
                    .iter()
                    .filter(|(row, _)| matches!(row, Row::PlainBlock { .. }))
                    .count()
                    <= 128,
                "two long literal Items register two bounded source pages"
            );
        });
    }
    // Reading at the start of the latest source page loads older leeway. A
    // viewport taller than the 32-row overlap must not fetch back and forth
    // merely because the splice exposes the opposite edge.
    conversation.update(cx, |c, cx| {
        let item = c.view.as_ref().unwrap().item(&user).unwrap().clone();
        c.jump_to(&item, cx);
    });
    wait(cx, "older literal source leeway", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation
                .read(cx)
                .shared
                .stored_sources
                .borrow()
                .get(&(user.clone(), crate::fold::PlainSection::User.slot()))
                .is_some_and(|source| source.loaded_range().start < 8001 - 64)
        })
    });
    let settled = cx.read(|cx| {
        let c = conversation.read(cx);
        (
            c.shared
                .stored_sources
                .borrow()
                .get(&(user.clone(), crate::fold::PlainSection::User.slot()))
                .unwrap()
                .loaded_range(),
            c.rows[c.list.logical_scroll_top().item_ix].0.key(),
        )
    });
    for _ in 0..6 {
        draw(cx);
        cx.run_until_parked();
        cx.read(|cx| {
            let c = conversation.read(cx);
            assert_eq!(
                c.rows[c.list.logical_scroll_top().item_ix].0.key(),
                settled.1
            );
            assert_eq!(
                c.shared
                    .stored_sources
                    .borrow()
                    .get(&(user.clone(), crate::fold::PlainSection::User.slot()))
                    .unwrap()
                    .loaded_range(),
                settled.0,
                "repainting the same reader position must not reverse source paging"
            );
        });
    }
    conversation.update(cx, |c, cx| {
        let index = c
            .rows
            .iter()
            .rposition(|(row, _)| row.anchor() == Some(&user))
            .unwrap();
        c.list.scroll_to(gpui::ListOffset {
            item_ix: index,
            offset_in_item: gpui::px(0.0),
        });
        cx.notify();
    });
    wait(cx, "reader movement permits newer source paging", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation
                .read(cx)
                .shared
                .stored_sources
                .borrow()
                .get(&(user.clone(), crate::fold::PlainSection::User.slot()))
                .is_some_and(|source| {
                    source.loaded_range().end == 8001 && source.loaded_range().len() <= 64
                })
        })
    });
}

#[test]
fn scoped_agent_source_order_surrounds_canonical_child_positions() {
    let task = crate::fold::selection_order(true, 10, "agent-task");
    let child = crate::fold::selection_order(true, 25, "prose");
    let result = crate::fold::selection_order(true, 10, "result-view");
    assert!(
        task < child && child < result,
        "reused Agent position must not put its result before child work"
    );
    assert_eq!(
        crate::fold::selection_order(false, 10, "result-view"),
        vec![10],
        "root preserves canonical Item position"
    );
}

#[gpui::test]
fn repaint_preserves_manual_unread_until_new_content_is_seen(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let first = forge
        .runtime
        .block_on(forge.store.insert_user_message(NewItem {
            id: ItemId::new(),
            thread_id: thread.clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::UserMessage {
                text: "First visible input".into(),
                images: Vec::new(),
                steer: false,
            },
        }))
        .unwrap()
        .0;
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    cx.update(|window, _| window.activate_window());
    open(&core, &host, &thread, cx);
    wait(
        cx,
        "first input is actually painted and acknowledged",
        |cx| {
            draw(cx);
            let stored = forge
                .runtime
                .block_on(forge.store.thread(&thread))
                .unwrap()
                .unwrap();
            stored.read_seq >= first.seq && !stored.marked_unread
        },
    );
    forge
        .runtime
        .block_on(forge.store.mark_unread(&thread))
        .unwrap()
        .unwrap();
    wait(cx, "manual unread event reaches the Client", |cx| {
        cx.read(|cx| {
            core.read(cx)
                .host(&host)
                .unwrap()
                .read(cx)
                .threads
                .get(&thread)
                .unwrap()
                .marked_unread
        })
    });
    for _ in 0..20 {
        draw(cx);
        cx.run_until_parked();
    }
    assert!(
        forge
            .runtime
            .block_on(forge.store.thread(&thread))
            .unwrap()
            .unwrap()
            .marked_unread,
        "repainting the same input must preserve the explicit unread mark"
    );
    let next = forge
        .runtime
        .block_on(forge.store.insert_user_message(NewItem {
            id: ItemId::new(),
            thread_id: thread.clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::UserMessage {
                text: "New visible input".into(),
                images: Vec::new(),
                steer: false,
            },
        }))
        .unwrap()
        .0;
    wait(
        cx,
        "new visible evidence advances the acknowledgement",
        |cx| {
            draw(cx);
            let stored = forge
                .runtime
                .block_on(forge.store.thread(&thread))
                .unwrap()
                .unwrap();
            stored.read_seq >= next.seq && !stored.marked_unread
        },
    );
    assert!(
        loaded(&conversation, cx)
            .iter()
            .any(|item| item.id == next.id)
    );
}

#[gpui::test]
fn reading_current_final_reply_acknowledges_its_terminal_run_boundary(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let item = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Current final reply"),
    );
    let run = forge
        .runtime
        .block_on(forge.store.upsert_run(artisan_api::Run {
            id: item.run_id.clone().unwrap(),
            thread_id: thread.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: 0,
            status: artisan_api::RunStatus::Done,
            usage: Default::default(),
            session_id: None,
            error: None,
            seq: 0,
            started_at: 0,
            ended_at: Some(1),
        }))
        .unwrap();
    assert!(
        run.seq > item.seq,
        "settlement is a later activity boundary than its reply"
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, _conversation, cx) = editor(&forge, dir.path(), cx);
    cx.update(|window, _| window.activate_window());
    open(&core, &host, &thread, cx);
    wait(
        cx,
        "current final content acknowledges the completed turn",
        |cx| {
            draw(cx);
            let stored = forge
                .runtime
                .block_on(forge.store.thread(&thread))
                .unwrap()
                .unwrap();
            stored.read_seq >= run.seq && !stored.is_unread()
        },
    );
}

#[gpui::test]
fn live_text_refreshes_only_its_rows_until_the_prepared_shape_changes(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    for text in ["First turn", "Second turn"] {
        forge.put(
            &thread,
            &ItemId::new(),
            Status::Done,
            ItemKind::UserMessage {
                text: text.into(),
                images: Vec::new(),
                steer: false,
            },
        );
    }
    let message = forge.put(
        &thread,
        &ItemId::new(),
        Status::Running,
        reply("Initial paragraph"),
    );
    let key: gpui::SharedString = message.id.to_string().into();
    let target = artisan_api::LiveTarget::Text {
        item_id: message.id.clone(),
    };
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "initial prose leaf", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation
                .read(cx)
                .rows
                .iter()
                .any(|(row, _)| matches!(row, Row::ProseBlock { id, .. } if id == &message.id))
        })
    });
    settle(cx, &conversation);
    let markers = cx.read(|cx| conversation.read(cx).navigator_markers.clone().unwrap());
    forge.store.publish_live(artisan_api::Event::Live {
        thread_id: thread.clone(),
        target: target.clone(),
        text: "Replacement paragraph".into(),
        append: false,
        coverage: artisan_api::LiveCoverage {
            start: 0,
            total: "Replacement paragraph".len() as u64,
        },
    });
    wait(
        cx,
        "replacement is prepared from the current immutable live snapshot",
        |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                c.view
                    .as_ref()
                    .and_then(|view| view.live.get(&target))
                    .is_some_and(|source| {
                        source.to_string() == "Replacement paragraph"
                            && c.shared.markdown.is_rope_source_prepared(&key, source)
                    })
            })
        },
    );
    cx.read(|cx| {
        assert!(
            Arc::ptr_eq(
                &markers,
                conversation.read(cx).navigator_markers.as_ref().unwrap()
            ),
            "unchanged body shape must not refold unrelated turns or discard navigator preparation"
        )
    });
    forge.store.publish_live(artisan_api::Event::Live {
        thread_id: thread.clone(),
        target: target.clone(),
        text: "\n\nA second paragraph".into(),
        append: true,
        coverage: artisan_api::LiveCoverage {
            start: 0,
            total: "Replacement paragraph\n\nA second paragraph".len() as u64,
        },
    });
    wait(
        cx,
        "new semantic leaf changes native list structure",
        |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                let leaves = c
                    .rows
                    .iter()
                    .filter(
                        |(row, _)| matches!(row, Row::ProseBlock { id, .. } if id == &message.id),
                    )
                    .count();
                leaves == 2
                    && c.view
                        .as_ref()
                        .and_then(|view| view.live.get(&target))
                        .is_some_and(|source| {
                            c.shared.markdown.is_rope_source_prepared(&key, source)
                        })
            })
        },
    );
    cx.read(|cx| {
        let c = conversation.read(cx);
        let last = c
            .rows
            .iter()
            .rev()
            .find_map(|(row, _)| {
                matches!(row, Row::ProseBlock { id, .. } if id == &message.id).then_some(row)
            })
            .unwrap();
        assert_eq!(c.read_evidence(last, cx), Some(message.seq));
    });
    let retained = "Replacement paragraph\n\nA second paragraph".len() as u64;
    forge.store.publish_live(artisan_api::Event::Live {
        thread_id: thread.clone(),
        target: target.clone(),
        text: String::new().into(),
        append: true,
        coverage: artisan_api::LiveCoverage {
            start: 0,
            total: retained + 1_000_000,
        },
    });
    wait(
        cx,
        "incomplete live coverage reaches the current prepared leaf",
        |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                let snapshot = core
                    .read(cx)
                    .host(&host)
                    .unwrap()
                    .read(cx)
                    .live_snapshot(&thread, &target);
                snapshot.is_some_and(|snapshot| snapshot.coverage.total == retained + 1_000_000)
                    && c.view
                        .as_ref()
                        .and_then(|view| view.live.get(&target))
                        .is_some_and(|source| {
                            c.shared.markdown.is_rope_source_prepared(&key, source)
                        })
            })
        },
    );
    cx.read(|cx| {
        let c = conversation.read(cx);
        let last = c
            .rows
            .iter()
            .rev()
            .find_map(|(row, _)| {
                matches!(row, Row::ProseBlock { id, .. } if id == &message.id).then_some(row)
            })
            .unwrap();
        assert_eq!(
            c.read_evidence(last, cx),
            None,
            "a prepared prefix cannot acknowledge omitted live content"
        );
    });
}

#[test]
fn stored_payloads_keep_canonical_rows_and_agent_reports() {
    let source = artisan_api::TextContent::Stored(artisan_api::ContentRef {
        id: artisan_api::ContentId("a".repeat(64)),
        bytes: 100_000,
        lines: 1000,
    });
    let mut item = Item {
        id: ItemId::new(),
        thread_id: ThreadId::new(),
        position: 17,
        seq: 41,
        run_id: None,
        parent_id: None,
        native_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::UserMessage {
            text: source.clone(),
            images: Vec::new(),
            steer: false,
        },
    };
    assert_eq!(crate::fold::PlainSection::User.text(&item), Some(&source));
    item.kind = ItemKind::Agent {
        name: None,
        description: "Task".into(),
        prompt: Some(source.clone()),
        agent_id: None,
        context_used: None,
        model: None,
        background: false,
        result: Some(source.clone()),
        label: None,
    };
    assert_eq!(
        crate::fold::PlainSection::AgentTask.text(&item),
        Some(&source)
    );
    assert_eq!(crate::fold::agent_report(&item), Some(&source));
    assert_eq!(
        crate::fold::TextSection::AgentResult.text(&item),
        Some(&source)
    );
    assert_eq!(
        item.position, 17,
        "source storage never changes Item ordering"
    );
}

#[gpui::test]
fn stored_raw_preview_cannot_acknowledge_complete_item(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let id = ItemId::new();
    let text = // A giant unbroken physical line exceeds literal raw-leaf admission.
    // Its descriptor/error preview must never masquerade as complete body paint.
    "Long literal source 界🙂 with preserved whitespace. ".repeat(2000);
    forge.put(
        &thread,
        &id,
        Status::Done,
        ItemKind::UserMessage {
            text: text.into(),
            images: Vec::new(),
            steer: false,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "stored message remains a real native row", |cx| {
        draw(cx);
        loaded(&conversation, cx).iter().any(|item| item.id == id)
    });
    settle(cx, &conversation);
    cx.read(|cx| {
        let conversation = conversation.read(cx);
        let item = conversation.view.as_ref().unwrap().item(&id).unwrap();
        assert!(matches!(
            &item.kind,
            ItemKind::UserMessage {
                text: artisan_api::TextContent::Stored(_),
                ..
            }
        ));
        let row = conversation
            .rows
            .iter()
            .map(|(row, _)| row)
            .find(|row| row.anchor() == Some(&id))
            .expect("stored source has a transcript row");
        assert_eq!(
            conversation.read_evidence(row, cx),
            None,
            "loading and raw preview are not evidence of final formatted content paint"
        );
    });
}

#[gpui::test]
fn stored_markdown_has_native_semantic_rows_and_immutable_leaf_read_proof(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let text = (0..750)
        .map(|index| {
            format!("Paragraph **{index}** retains its authored *formatting* and source order.\n\n")
        })
        .collect::<String>();
    let message = forge.put(&thread, &ItemId::new(), Status::Done, reply(&text));
    let ItemKind::AssistantMessage {
        text: artisan_api::TextContent::Stored(reference),
        ..
    } = &message.kind
    else {
        panic!("fixture must exercise immutable Stored text, not inline preparation");
    };
    assert!(reference.bytes > artisan_api::INLINE_TEXT_BYTES as u64);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(
        cx,
        "bounded stored semantic rows with complete logical topology",
        |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                let sources = c.shared.stored_sources.borrow();
                sources.get(&(message.id.clone(), "prose")).is_some_and(|source| {
                    source.source_ref() == reference
                        && source.block_count() == 750
                        && !source.loaded_range().is_empty()
                        && source.loaded_range().len() <= 64
                        && c.rows.iter().filter(|(row, _)| matches!(row, Row::ProseBlock { id, .. } if id == &message.id)).count() == source.loaded_range().len()
                })
            })
        },
    );
    cx.read(|cx| {
        let conversation = conversation.read(cx);
        let sources = conversation.shared.stored_sources.borrow();
        let source = sources.get(&(message.id.clone(), "prose")).unwrap();
        assert_eq!(source.source_ref(), reference);
        assert_eq!(source.block_count(), 750);
        assert!(
            conversation.row_views.iter().flatten().count() < 128,
            "source topology must not create 750 mounted views"
        );
        assert_eq!(
            conversation.read_evidence(
                &Row::ProseBlock {
                    id: message.id.clone(),
                    block: 0
                },
                cx
            ),
            None,
            "an indexed early leaf cannot acknowledge the final body"
        );
        let final_key: gpui::SharedString = format!("{}:prose:leaf:749", message.id).into();
        if !conversation
            .shared
            .markdown
            .is_stored_leaf_prepared(&final_key, source, 749)
        {
            assert_eq!(
                conversation.read_evidence(
                    &Row::ProseBlock {
                        id: message.id.clone(),
                        block: 749
                    },
                    cx
                ),
                None,
                "topology alone is not read evidence"
            );
        }
    });
}

#[gpui::test]
fn stored_single_literal_line_uses_bounded_original_native_glyph_rows(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let id = ItemId::new();
    let text = "original native glyphs 界🙂 ".repeat(1500);
    assert!(text.len() > 16 * 1024 && text.len() < 64 * 1024);
    let item = forge.put(
        &thread,
        &id,
        Status::Done,
        ItemKind::UserMessage {
            text: text.into(),
            images: Vec::new(),
            steer: false,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "bounded original native glyph paragraph tail", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            let key = Row::PlainBlock {
                id: id.clone(),
                section: crate::fold::PlainSection::User,
                block: 0,
            }
            .key();
            let states = c.shared.native_windows.borrow();
            let Some(state) = states.get(&key) else {
                return false;
            };
            let last = c
                .rows
                .iter()
                .rev()
                .find(|(row, _)| row.anchor() == Some(&id));
            state.count > 128
                && state.window.end == state.count
                && last.is_some_and(|(row, _)| c.read_evidence(row, cx) == Some(item.seq))
        })
    });
    cx.read(|cx| {
        let c = conversation.read(cx);
        let key = Row::PlainBlock {
            id: id.clone(),
            section: crate::fold::PlainSection::User,
            block: 0,
        }
        .key();
        let sources = c.shared.stored_sources.borrow();
        let source = sources.get(&(id.clone(), "user")).unwrap();
        assert_eq!(source.format(), artisan_api::SourceIndexFormat::Literal);
        assert_eq!(
            source.block_count(),
            1,
            "the literal remains one semantic hard line"
        );
        let states = c.shared.native_windows.borrow();
        let state = states.get(&key).unwrap();
        assert!(
            state.count > 128,
            "real native wrapping supplies the visual topology"
        );
        let rows: Vec<_> = c
            .rows
            .iter()
            .map(|(row, _)| row)
            .filter(|row| row.anchor() == Some(&id))
            .collect();
        assert_eq!(rows.len(), state.window.len());
        assert!(rows.len() <= crate::row::NATIVE_BODY_WINDOW);
        assert!(c.row_views.iter().flatten().count() <= 128);
        assert_eq!(
            c.read_evidence(rows[0], cx),
            None,
            "early painted glyph rows are not final source proof"
        );
        assert_eq!(rows.last().unwrap().visual(), Some(state.count - 1));
        assert_eq!(c.read_evidence(rows.last().unwrap(), cx), Some(item.seq));
    });
}

mod request_windows;

/// Native ListState may measure a whole UserMessage while only a few of its
/// attachments intersect the transcript clip. Only those tiles request blobs.
#[gpui::test]
fn partially_clipped_attachment_row_loads_only_painted_tiles_not_measured_siblings(
    cx: &mut TestAppContext,
) {
    // Pinned GPUI test selectors require static strings. This fixture has one
    // fixed canonical Item identity and exactly 32 attachments.
    const SELECTORS: [&str; 32] = [
        "sent-image-clipped-attachments-0",
        "sent-image-clipped-attachments-1",
        "sent-image-clipped-attachments-2",
        "sent-image-clipped-attachments-3",
        "sent-image-clipped-attachments-4",
        "sent-image-clipped-attachments-5",
        "sent-image-clipped-attachments-6",
        "sent-image-clipped-attachments-7",
        "sent-image-clipped-attachments-8",
        "sent-image-clipped-attachments-9",
        "sent-image-clipped-attachments-10",
        "sent-image-clipped-attachments-11",
        "sent-image-clipped-attachments-12",
        "sent-image-clipped-attachments-13",
        "sent-image-clipped-attachments-14",
        "sent-image-clipped-attachments-15",
        "sent-image-clipped-attachments-16",
        "sent-image-clipped-attachments-17",
        "sent-image-clipped-attachments-18",
        "sent-image-clipped-attachments-19",
        "sent-image-clipped-attachments-20",
        "sent-image-clipped-attachments-21",
        "sent-image-clipped-attachments-22",
        "sent-image-clipped-attachments-23",
        "sent-image-clipped-attachments-24",
        "sent-image-clipped-attachments-25",
        "sent-image-clipped-attachments-26",
        "sent-image-clipped-attachments-27",
        "sent-image-clipped-attachments-28",
        "sent-image-clipped-attachments-29",
        "sent-image-clipped-attachments-30",
        "sent-image-clipped-attachments-31",
    ];
    let forge = Forge::start();
    let thread = forge.thread();
    let mut hashes = Vec::new();
    for index in 0..32u32 {
        let mut bytes = image_bytes();
        if bytes.len() % 2 != 0 {
            bytes.push(0);
        }
        bytes.extend_from_slice(b"JUNK");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&index.to_le_bytes());
        let length = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&length.to_le_bytes());
        hashes.push(
            forge
                .runtime
                .block_on(forge.store.put_blob(bytes))
                .unwrap()
                .hash,
        );
    }
    let message = forge.put(
        &thread,
        &ItemId::from("clipped-attachments"),
        Status::Done,
        ItemKind::UserMessage {
            text: "".into(),
            images: hashes.clone(),
            steer: false,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host_id, conversation, cx) = editor(&forge, dir.path(), cx);
    // Keep the initial published row under a zero-area native clip. Otherwise
    // legitimate initial tail paint may start requests before measurement.
    cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(0.0)));
    let host = cx.read(|cx| core.read(cx).host(&host_id).unwrap().clone());
    open(&core, &host_id, &thread, cx);
    wait(cx, "attachment message data", |cx| {
        loaded(&conversation, cx)
            .iter()
            .any(|item| item.id == message.id)
    });
    let previously_painted: std::collections::HashSet<_> = cx.read(|cx| {
        hashes
            .iter()
            .enumerate()
            .filter_map(|(index, hash)| host.read(cx).image(hash).is_some().then_some(index))
            .collect()
    });
    assert!(
        previously_painted.len() < hashes.len(),
        "offscreen siblings must exist before measurement"
    );
    // Measure the production row inside GPUI's prepaint phase. The measured
    // child is never prepainted or painted, as with native list measurement.
    use gpui::Styled as _;
    let measured = std::rc::Rc::new(std::cell::Cell::new(false));
    let measurement = measured.clone();
    let measure_conversation = conversation.clone();
    let measure_id = message.id.clone();
    cx.draw(
        gpui::point(gpui::px(0.0), gpui::px(0.0)),
        gpui::size(gpui::px(400.0), gpui::px(1.0)),
        move |_, _| {
            gpui::canvas(
                move |_, window, cx| {
                    // Release the controller borrow before the child renders.
                    let mut element = measure_conversation.update(cx, |c, cx| {
                        let index = c
                            .rows
                            .iter()
                            .position(|(row, _)| row.anchor() == Some(&measure_id))
                            .unwrap();
                        c.row_element(index, window, cx)
                    });
                    element.layout_as_root(
                        gpui::size(
                            gpui::AvailableSpace::Definite(gpui::px(400.0)),
                            gpui::AvailableSpace::MinContent,
                        ),
                        window,
                        cx,
                    );
                    measurement.set(true);
                },
                |_, (), _, _| {},
            )
            .w(gpui::px(400.0))
            .h(gpui::px(1.0))
        },
    );
    assert!(measured.get(), "the native measurement must actually run");
    assert!(
        cx.read(|cx| hashes.iter().enumerate().all(|(index, hash)| host
            .read(cx)
            .image(hash)
            .is_some()
            == previously_painted.contains(&index))),
        "measurement must not fetch an entire attachment row"
    );
    // An explicit public jump exits Following before native geometry places
    // the first attachment row above the clip, guaranteeing partial paint.
    cx.update(|_, cx| {
        conversation.update(cx, |c, cx| {
            c.jump_to(&message, cx);
            // This fixture checks stationary native clipping; navigation motion
            // has separate tests and must not move its measured tile boundary.
            c.arrival = None;
            let index = c
                .rows
                .iter()
                .position(|(row, _)| row.anchor() == Some(&message.id))
                .unwrap();
            c.list.scroll_to(gpui::ListOffset {
                item_ix: index,
                offset_in_item: gpui::px(100.0),
            });
            cx.notify();
        });
    });
    // Install the reading anchor while the native clip is still empty. A
    // resize is itself a draw and must not first paint the Following tail.
    cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(400.0)));
    wait(cx, "visible attachment previews", |cx| {
        draw(cx);
        let clip = cx.read(|cx| conversation.read(cx).list.viewport_bounds());
        let visible: Vec<_> = (0..hashes.len())
            .filter(|index| {
                cx.debug_bounds(SELECTORS[*index]).is_some_and(|bounds| {
                    bounds.right() > clip.left()
                        && bounds.left() < clip.right()
                        && bounds.bottom() > clip.top()
                        && bounds.top() < clip.bottom()
                })
            })
            .collect();
        !visible.is_empty()
            && cx.read(|cx| {
                visible.iter().all(|index| {
                    matches!(
                        host.read(cx).image(&hashes[*index]),
                        Some(crate::row::Blob::Ready(_))
                    )
                })
            })
    });
    let clip = cx.read(|cx| conversation.read(cx).list.viewport_bounds());
    let mut visible = std::collections::HashSet::new();
    let mut partial = false;
    for index in 0..hashes.len() {
        if let Some(bounds) = cx.debug_bounds(SELECTORS[index]) {
            if bounds.right() > clip.left()
                && bounds.left() < clip.right()
                && bounds.bottom() > clip.top()
                && bounds.top() < clip.bottom()
            {
                visible.insert(index);
                partial |= bounds.top() < clip.top() || bounds.bottom() > clip.bottom();
            }
        }
    }
    assert!(
        !visible.is_empty() && visible.len() < hashes.len(),
        "fixture must contain painted and clipped-out sibling tiles"
    );
    assert!(
        partial,
        "fixture must include a tile cut by the native clip"
    );
    cx.read(|cx| {
        for (index, hash) in hashes.iter().enumerate() {
            assert_eq!(
                host.read(cx).image(hash).is_some(),
                visible.contains(&index) || previously_painted.contains(&index),
                "only a currently or previously individually painted tile {index} may own a request/result"
            );
        }
    });
    // Newly visible tail tiles still load automatically through the existing
    // public navigation action; no permanent preview-admission pause is added.
    conversation.update(cx, |c, cx| c.jump_to_latest(cx));
    wait(cx, "newly painted tail attachment", |cx| {
        draw(cx);
        cx.read(|cx| {
            matches!(
                host.read(cx).image(hashes.last().unwrap()),
                Some(crate::row::Blob::Ready(_))
            )
        })
    });
}

/// Goal lifecycle belongs to one thread's composer, never its transcript.
#[gpui::test]
fn goals_are_thread_scoped_and_not_transcript_announcements(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let first = forge.thread();
    let second = forge.thread();
    let notice = ItemId::new();
    forge.put(
        &first,
        &notice,
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::Goal {
                state: "paused".into(),
            },
            text: "Finish the work in this thread only.".into(),
        },
    );
    forge.put(
        &first,
        &ItemId::new(),
        Status::Done,
        reply("A normal reply."),
    );
    forge.put(
        &second,
        &ItemId::new(),
        Status::Done,
        reply("A different thread."),
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &first, cx);
    wait(cx, "the first thread goal strip", |cx| {
        draw(cx);
        cx.debug_bounds("composer-goal-label").is_some()
    });
    cx.read(|cx| {
        assert!(
            !conversation
                .read(cx)
                .rows
                .iter()
                .any(|(row, _)| row.anchor() == Some(&notice)),
            "a nonempty goal lifecycle notice must not render in the transcript"
        );
    });
    core.update(cx, |core, cx| {
        core.navigate(
            Screen::NewThread {
                host: host.clone(),
                project: Some(forge.project.clone()),
            },
            cx,
        )
    });
    draw(cx);
    assert!(
        cx.debug_bounds("composer-goal-label").is_none(),
        "New Thread must clear the goal immediately, without a retracting old goal"
    );
    open(&core, &host, &second, cx);
    settle(cx, &conversation);
    assert!(
        cx.debug_bounds("composer-goal-label").is_none(),
        "the other thread has no goal"
    );
    open(&core, &host, &first, cx);
    wait(cx, "the first thread's own goal restored", |cx| {
        draw(cx);
        cx.debug_bounds("composer-goal-label").is_some()
    });
}

#[gpui::test]
fn adjacent_work_and_compaction_share_one_separator_and_keep_disclosure(cx: &mut TestAppContext) {
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
    let checkpoint = ItemId::new();
    let compact = || ItemKind::Compaction {
        tokens_before: Some(120_000),
        tokens_after: Some(40_000),
    };
    forge.put(&thread, &checkpoint, Status::Done, compact());
    forge.put(&thread, &ItemId::new(), Status::Done, reply("It builds."));
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the compacted turn", |cx| {
        loaded(&conversation, cx).len() == 4
    });
    draw(cx);
    let merged_revision = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            conversation
                .read(cx)
                .rows
                .iter()
                .find_map(|(row, version)| match row.semantic() {
                    Row::Work {
                        compaction: Some(id),
                        ..
                    } if id == &checkpoint => Some(*version),
                    _ => None,
                })
                .expect("checkpoint is metadata on the work separator")
        })
    };
    let initial_revision = merged_revision(cx);
    cx.read(|cx| {
        assert!(
            !conversation
                .read(cx)
                .rows
                .iter()
                .any(|(row, _)| matches!(row.semantic(), Row::Compaction(id) if id == &checkpoint)),
            "the merged checkpoint has no second native separator"
        )
    });
    let header = cx
        .debug_bounds("work-header")
        .expect("worked duration and disclosure remain");
    let indicator = cx
        .debug_bounds("work-compaction")
        .expect("compacted indicator remains");
    assert!(
        indicator.top() >= header.top() && indicator.bottom() <= header.bottom(),
        "the compacted indicator shares the work header"
    );
    assert!(
        cx.debug_bounds("work-chain").is_none(),
        "completed work starts collapsed"
    );
    click(cx, "work-header");
    settle(cx, &conversation);
    assert!(
        cx.debug_bounds("work-chain").is_some(),
        "the merged separator still expands its native work descendants"
    );
    assert!(cx.debug_bounds("work-compaction").is_some());
    forge.put(&thread, &checkpoint, Status::Cancelled, compact());
    wait(cx, "the merged checkpoint status revision", |cx| {
        merged_revision(cx) != initial_revision
    });
    draw(cx);
    assert!(
        cx.debug_bounds("work-compaction").is_some(),
        "status updates retain the combined separator"
    );
    let separate = ItemId::new();
    forge.put(&thread, &separate, Status::Done, compact());
    wait(cx, "a checkpoint separated by the reply", |cx| {
        loaded(&conversation, cx).len() == 5
    });
    cx.read(|cx| {
        assert!(
            conversation
                .read(cx)
                .rows
                .iter()
                .any(|(row, _)| matches!(row.semantic(), Row::Compaction(id) if id == &separate)),
            "a reply between separators prevents merging"
        )
    });
}
