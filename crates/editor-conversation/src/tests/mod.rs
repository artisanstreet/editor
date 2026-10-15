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
    cx.update(editor_sources::init);
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
                        Row::ProseBlock { id, block } => {
                            Some((id, crate::fold::Slot::Prose, *block, id.to_string()))
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
                                        Row::ProseBlock { id, block } => {
                                            let block = *block;
                                            if let Some(source) = c
                                                .shared
                                                .stored_sources
                                                .borrow()
                                                .get(&(id.clone(), crate::fold::Slot::Prose))
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

mod agents;
mod media;
mod paging;
mod reading;
mod requests;
mod stored;
mod streaming;
mod work;
