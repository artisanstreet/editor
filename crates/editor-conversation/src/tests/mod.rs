//! The conversation against a real in-process Forge: what the reader sees
//! when a long thread opens and scrolls back, and as items stream in.

use crate::Conversation;
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
            server.run().await.ok();
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
                spawned_by: None,
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
        self.end_run_in(run, thread, artisan_api::RunStatus::Done);
    }

    fn end_run_in(&self, run: &RunId, thread: &ThreadId, status: artisan_api::RunStatus) {
        self.runtime
            .block_on(self.store.upsert_run(artisan_api::Run {
                config: None,
                id: run.clone(),
                thread_id: thread.clone(),
                engine: EngineId::Claude,
                model: None,
                input_cutoff: 0,
                status,
                usage: artisan_api::Usage::default(),
                session_id: None,
                error: None,
                seq: 0,
                started_at: 0,
                ended_at: Some(1),
                continuation: None,
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
    cx.update(editor_sources::init);
    let core = cx.new(|cx| Core::for_tests(files, cx).unwrap());
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

fn loaded(conversation: &Entity<Conversation>, cx: &VisualTestContext) -> Vec<Item> {
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

fn shell(command: &str) -> ItemKind {
    ItemKind::Tool {
        tool: artisan_api::ToolKind::Shell {
            command: command.into(),
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

/// Where the row named `selector` (see `Row::selector`) painted in the
/// last frame, if it did.
fn shown(
    cx: &mut VisualTestContext,
    selector: impl Into<String>,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    painted(cx, format!("row:{}", selector.into()))
}

/// The rows of `selectors` that painted, top to bottom.
fn painted_order(cx: &mut VisualTestContext, selectors: &[String]) -> Vec<String> {
    let mut painted: Vec<_> = selectors
        .iter()
        .filter_map(|selector| Some((shown(cx, selector.clone())?.top(), selector.clone())))
        .collect();
    painted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    painted.into_iter().map(|(_, selector)| selector).collect()
}

/// Where the element marked `selector` painted in the last frame.
fn painted(cx: &mut VisualTestContext, selector: String) -> Option<gpui::Bounds<gpui::Pixels>> {
    thread_local! {
        static NAMES: std::cell::RefCell<HashMap<String, &'static str>> = std::cell::RefCell::default();
    }
    let name = NAMES.with(|names| {
        *names
            .borrow_mut()
            .entry(selector.clone())
            .or_insert_with(|| Box::leak(selector.into_boxed_str()))
    });
    cx.debug_bounds(name)
}

/// Whether Forge holds the thread read (the sidebar's unread dot is off).
fn read(forge: &Forge, thread: &ThreadId) -> bool {
    !forge
        .runtime
        .block_on(forge.store.thread(thread))
        .unwrap()
        .unwrap()
        .is_unread()
}

/// Waits until the opened thread shows: its first screen is ready.
fn landed(cx: &mut VisualTestContext, conversation: &Entity<Conversation>) {
    wait(cx, "the thread", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            !c.landing() && c.view.as_ref().is_some_and(|view| view.window().loaded())
        })
    });
}

/// Waits until nothing loads, reveals or slides any more and the
/// transcript has stopped moving.
fn settle(cx: &mut VisualTestContext, conversation: &Entity<Conversation>) {
    let mut last = None;
    let mut stable_since = Instant::now();
    wait(cx, "visible content and motion to settle", |cx| {
        draw(cx);
        let still = ["text-loading", "animating", "disclosing"]
            .iter()
            .all(|name| cx.debug_bounds(name).is_none());
        let (loading, geometry) = cx.read(|cx| {
            let c = conversation.read(cx);
            let top = c.list.logical_scroll_top();
            (
                c.view.as_ref().is_some_and(|view| view.window().in_flight)
                    || c.work_reading.is_some(),
                (
                    top.item_ix,
                    f32::from(top.offset_in_item),
                    f32::from(c.list.max_offset_for_scrollbar().y),
                ),
            )
        });
        let ready = still && !loading;
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

mod agents;
mod media;
mod paging;
mod previews;
mod reading;
mod requests;
mod stored;
mod streaming;
mod work;
