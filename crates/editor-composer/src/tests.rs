#[path = "../../editor-ui/test_support/forge.rs"]
mod forge_fixture;
// The composer against a real in-process Forge with a scripted engine:
// what the user types and pastes is what Forge stores.

use super::*;
use artisan_api::{
    CreateProject, EngineId, Item, ItemKind, ItemsQuery, Phase, Status, UpdateThread,
};
use forge_engines::{EngineEvent, Harness, Input, Outcome, RunIo, Turn};
use futures::future::BoxFuture;
use gpui::{ClipboardItem, Image, ImageFormat, TestAppContext, VisualTestContext, size};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Replies "ok". A message containing "hold" keeps the run live until a
/// steer arrives.
struct Engine;

/// The settings each run started with, by thread: what the engine was
/// asked to run.
static RUNS: std::sync::Mutex<Vec<(ThreadId, EngineConfig)>> = std::sync::Mutex::new(Vec::new());

fn runs_of(thread: &ThreadId) -> Vec<EngineConfig> {
    RUNS.lock()
        .unwrap()
        .iter()
        .filter(|(run, _)| run == thread)
        .map(|(_, config)| config.clone())
        .collect()
}

impl Harness for Engine {
    fn run(&self, turn: Turn, mut io: RunIo) -> BoxFuture<'static, Outcome> {
        RUNS.lock()
            .unwrap()
            .push((turn.thread_id.clone(), turn.config.clone()));
        Box::pin(async move {
            io.emit(EngineEvent::InputAccepted {
                id: turn.input.id.clone(),
            })
            .await;
            if turn.input.text.contains("hold") {
                tokio::select! {
                    input = io.inbox.recv() => {
                        if let Some(Input::Steer(steer)) = input {
                            io.emit(EngineEvent::InputAccepted { id: steer.id }).await;
                        }
                    }
                    () = io.cancel.cancelled() => return Outcome::cancelled(),
                }
            }
            io.emit(EngineEvent::Item {
                key: "reply".into(),
                parent: None,
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: "ok".into(),
                    phase: Phase::Final,
                },
            })
            .await;
            io.emit(EngineEvent::Session {
                id: turn.input.text.clone(),
            })
            .await;
            Outcome::done()
        })
    }

    /// A session whose message asked for one guesses the next message.
    fn side_query(
        &self,
        turn: Turn,
        _cancel: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'static, anyhow::Result<forge_engines::SideReply>> {
        let asked = turn
            .session_id
            .as_deref()
            .is_some_and(|session| session.contains("suggest"));
        Box::pin(async move {
            Ok(forge_engines::SideReply {
                text: if asked {
                    r#"{"suggestion": "Now the lexer"}"#.to_owned()
                } else {
                    r#"{"suggestion": null}"#.to_owned()
                },
                usage: forge_engines::SideUsage::default(),
            })
        })
    }
}

pub struct Fixture {
    core: Entity<Core>,
    pub(super) host: HostId,
    pub(super) project: ProjectId,
    store: forge_store::Store,
    forge: tokio::runtime::Runtime,
    dir: tempfile::TempDir,
}

impl Fixture {
    pub(super) fn new(cx: &mut TestAppContext) -> Self {
        // Forge and the network runtime wake GPUI tasks from their threads.
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("forge");
        // A fake installed Claude so runs get past "not installed".
        let version = data.join("engines/claude/versions/1.0.0");
        std::fs::create_dir_all(&version).unwrap();
        // Signed in, as far as `claude auth status` tells, and reading the
        // commands its `initialize` reply lists.
        let binary = version.join("claude");
        let commands = serde_json::json!({
            "type": "control_response",
            "response": { "request_id": "artisan-commands", "subtype": "success", "response": {
                "commands": [
                    { "name": "compact", "description": "Shrink the context", "argumentHint": "" },
                    { "name": "context", "description": "Show context use", "argumentHint": "" },
                    { "name": "goal", "description": "Work toward a goal", "argumentHint": "<objective>" },
                ],
            } },
        });
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nif [ \"$1\" = -p ]; then read -r line; echo '{commands}'; exit 0; fi\necho '{{\"loggedIn\":true,\"email\":\"test@example.com\"}}'\n"
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(data.join("engines/claude/current"), "1.0.0").unwrap();
        // The models it serves, as its CLI caches them.
        let catalog = data.join("engines/claude/home/.claude/cache/model-catalog");
        std::fs::create_dir_all(&catalog).unwrap();
        let effort = |id: &str| serde_json::json!({ "id": id });
        let listing = serde_json::json!({
            "fetchedAt": 1,
            "catalog": { "surface": "cc", "config": { "id": "cc", "models": [
                { "id": "sonnet", "name": "Sonnet", "runtime": { "max_input_tokens": 1_000_000 }, "thinking": { "type": "adaptive",
                    "effort_options": [effort("low"),
                        { "id": "medium", "badge": { "message": "Default" } }, effort("high")] },
                  "fast_mode": { "type": "toggle", "name": "Fast mode", "options": [
                        { "id": "fast", "name": "Enable fast mode", "selector_label": "Fast" },
                        { "id": "off", "name": "Off" }] } },
                { "id": "opus", "name": "Opus", "thinking": { "type": "adaptive",
                    "effort_options": [effort("low"), effort("high")] } },
            ] } },
        });
        std::fs::write(catalog.join("test-cc.json"), listing.to_string()).unwrap();

        let forge = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let engine: Arc<dyn Harness> = Arc::new(Engine);
        let config = forge_http::ServeConfig {
            data_dir: data,
            listen: "127.0.0.1:0".parse().unwrap(),
        };
        let server = forge
            .block_on(forge_http::start_with_harness(
                config,
                Arc::new(move |_| Arc::clone(&engine)),
            ))
            .unwrap();
        let address = server.local_addr().unwrap().to_string();
        let store = server.store().clone();
        forge.spawn(server.run());

        let (files, host) =
            forge_fixture::pair(&store, &forge, &address, &dir.path().join("editor"));
        let project = forge
            .block_on(store.create_project(CreateProject {
                id: ProjectId::new(),
                path: dir.path().display().to_string(),
                name: None,
            }))
            .unwrap()
            .id;
        cx.update(editor_sources::init);
        let core = cx.new(|cx| Core::new(files, cx).unwrap());
        Self {
            core,
            host: host.id,
            project,
            store,
            forge,
            dir,
        }
    }

    fn thread(&self) -> ThreadId {
        let create = CreateThread {
            id: ThreadId::new(),
            project_id: self.project.clone(),
            config: EngineConfig::new(EngineId::Claude),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        };
        self.forge
            .block_on(self.store.create_thread(create))
            .unwrap()
            .id
    }

    fn user_messages(&self, thread: &ThreadId) -> Vec<Item> {
        let query = ItemsQuery {
            before: None,
            after: None,
            limit: Some(100),
        };
        let page = self
            .forge
            .block_on(self.store.items_page(thread, query))
            .unwrap();
        page.items
            .into_iter()
            .filter(|item| matches!(item.kind, ItemKind::UserMessage { .. }))
            .collect()
    }

    /// Installs a skill on Forge, as `ae` or the settings page would.
    fn add_skill(&self, name: &str, description: &str) {
        let folder = self.dir.path().join("forge/extensions/skills").join(name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nDo it well.\n"),
        )
        .unwrap();
    }

    fn draft(&self, scope: &str) -> Option<serde_json::Value> {
        let body = self.forge.block_on(self.store.draft(scope)).unwrap().body;
        (body != serde_json::Value::Null).then_some(body)
    }

    pub(super) fn open<'a>(
        &self,
        cx: &'a mut TestAppContext,
        target: ComposerTarget,
    ) -> (Entity<Composer>, &'a mut VisualTestContext) {
        let core = self.core.clone();
        cx.update(editor_ui::text_editor::TextEditor::bind_actions);
        let (composer, cx) =
            cx.add_window_view(move |window, cx| Composer::new(core, target, window, cx));
        cx.simulate_resize(size(px(800.0), px(600.0)));
        let host = self.host.clone();
        let core = self.core.clone();
        let project = self.project.clone();
        wait(cx, "the Editor to follow Forge", move |cx| {
            cx.read(|app| {
                core.read(app).host(&host).is_some_and(|host| {
                    let host = host.read(app);
                    host.is_live() && host.projects.contains_key(&project)
                })
            })
        });
        cx.update(|window, app| {
            let focus = composer.read(app).focus_handle(app);
            window.focus(&focus, app);
        });
        cx.run_until_parked();
        (composer, cx)
    }
}

/// Lets GPUI and the network threads run until `done`; Forge answers on
/// its own threads, so the test executor alone can't get there.
pub fn wait(
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

fn knows_thread(
    cx: &VisualTestContext,
    core: &Entity<Core>,
    host: &HostId,
    thread: &ThreadId,
) -> bool {
    cx.read(|app| {
        core.read(app)
            .host(host)
            .is_some_and(|host| host.read(app).threads.contains_key(thread))
    })
}

fn text(cx: &VisualTestContext, composer: &Entity<Composer>) -> String {
    cx.read(|app| composer.read(app).text(app).to_owned())
}

#[gpui::test]
fn sending_stores_one_message_and_clears_the_draft(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let thread = forge.thread();
    let target = ComposerTarget::Thread {
        host: forge.host.clone(),
        thread: thread.clone(),
    };
    let (composer, cx) = forge.open(cx, target);
    let (core, host) = (forge.core.clone(), forge.host.clone());
    wait(cx, "the thread", |cx| {
        knows_thread(cx, &core, &host, &thread)
    });

    cx.simulate_input("hello forge");
    cx.executor().advance_clock(Duration::from_secs(1));
    let scope = format!("thread:{thread}");
    wait(cx, "the draft to be saved", |_| {
        forge
            .draft(&scope)
            .is_some_and(|draft| draft["text"] == "hello forge")
    });

    cx.simulate_keystrokes("enter");
    wait(cx, "the message", |cx| {
        !forge.user_messages(&thread).is_empty() && text(cx, &composer).is_empty()
    });
    wait(cx, "the draft to be cleared", |_| {
        forge.draft(&scope).is_none()
    });

    let messages = forge.user_messages(&thread);
    assert_eq!(
        messages.len(),
        1,
        "one send stores one message: {messages:?}"
    );
    assert!(
        matches!(&messages[0].kind, ItemKind::UserMessage { text, steer: false, .. } if text.inline() == Some("hello forge")),
        "{messages:?}"
    );
}

#[gpui::test]
fn sending_during_a_live_run_steers_it(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let thread = forge.thread();
    let target = ComposerTarget::Thread {
        host: forge.host.clone(),
        thread: thread.clone(),
    };
    let (composer, cx) = forge.open(cx, target);
    let (core, host) = (forge.core.clone(), forge.host.clone());
    wait(cx, "the thread", |cx| {
        knows_thread(cx, &core, &host, &thread)
    });
    let sent = Rc::new(std::cell::Cell::new(0));
    let counted = Rc::clone(&sent);
    let _subscription = cx.update(|_, app| {
        app.subscribe(&composer, move |_, _: &ComposerEvent, _| {
            counted.set(counted.get() + 1);
        })
    });

    cx.simulate_input("hold on");
    cx.simulate_keystrokes("enter");
    wait(cx, "the run to go live", |cx| {
        cx.read(|app| {
            core.read(app)
                .host(&host)
                .is_some_and(|host| host.read(app).is_working(&thread))
        })
    });

    wait(cx, "the first submission to release the input", |cx| {
        text(cx, &composer).is_empty() && sent.get() == 1
    });
    cx.simulate_input("also this");
    cx.simulate_keystrokes("enter");
    wait(cx, "the steer", |_| forge.user_messages(&thread).len() == 2);
    wait(cx, "the composer to settle", |cx| {
        text(cx, &composer).is_empty() && sent.get() == 2
    });

    let messages = forge.user_messages(&thread);
    assert!(
        matches!(&messages[1].kind, ItemKind::UserMessage { text, steer: true, .. } if text.inline() == Some("also this")),
        "the second message is a steer of the live run: {messages:?}"
    );
    let holding = cx.read(|app| {
        composer
            .read(app)
            .session
            .as_ref()
            .is_some_and(|session| session.read(app).has_outgoing())
    });
    assert!(!holding, "nothing stays pending in the composer");
}

#[gpui::test]
fn a_pasted_png_is_sent_as_webp(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, cx) = forge.open(cx, target);

    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 48, |x, y| {
        image::Rgba([
            u8::try_from(x * 4).unwrap(),
            u8::try_from(y * 5).unwrap(),
            128,
            255,
        ])
    }))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
        ImageFormat::Png,
        png,
    )));
    cx.simulate_keystrokes("ctrl-v");
    cx.simulate_input("what is this?");
    wait(cx, "the image upload", |cx| {
        cx.read(|app| {
            let composer = composer.read(app);
            !composer.attachments.is_empty()
                && composer
                    .attachments
                    .iter()
                    .all(|a| a.state.hash().is_some())
        })
    });
    cx.simulate_keystrokes("enter");

    let core = forge.core.clone();
    let mut thread = None;
    wait(cx, "the new thread to open", |cx| {
        thread = cx.read(|app| core.read(app).screen().thread().cloned());
        thread
            .as_ref()
            .is_some_and(|thread| !forge.user_messages(thread).is_empty())
    });
    let messages = forge.user_messages(&thread.unwrap());
    let ItemKind::UserMessage { text, images, .. } = &messages[0].kind else {
        unreachable!()
    };
    assert_eq!(text.inline(), Some("what is this?"));
    assert_eq!(images.len(), 1, "the message references the pasted image");
    let stored = std::fs::read(forge.store.blob_path(&images[0]).unwrap()).unwrap();
    assert!(
        &stored[..4] == b"RIFF" && &stored[8..12] == b"WEBP",
        "Forge got WebP, not PNG"
    );
}

#[gpui::test]
fn dropped_files_and_folders_are_sent_as_links(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, cx) = forge.open(cx, target);
    let dropped = tempfile::tempdir().unwrap();
    let notes = dropped.path().join("design notes.md");
    std::fs::write(&notes, "# notes").unwrap();
    let folder = dropped.path().join("src");
    std::fs::create_dir(&folder).unwrap();

    composer.update(cx, |composer, cx| {
        composer.add_paths(vec![notes.clone(), folder.clone(), notes.clone()], cx);
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read(|app| composer.read(app).files.len()),
        2,
        "the second drop of the same file shakes the first rather than adding it"
    );
    cx.simulate_input("read these");
    cx.simulate_keystrokes("enter");

    let core = forge.core.clone();
    let mut thread = None;
    wait(cx, "the new thread to open", |cx| {
        thread = cx.read(|app| core.read(app).screen().thread().cloned());
        thread
            .as_ref()
            .is_some_and(|thread| !forge.user_messages(thread).is_empty())
    });
    let messages = forge.user_messages(&thread.unwrap());
    let ItemKind::UserMessage { text, images, .. } = &messages[0].kind else {
        unreachable!()
    };
    assert_eq!(
        text.inline(),
        Some(
            format!(
                "read these\n\n[design notes.md](<{}>)\n[src/](<{}>)",
                notes.display(),
                folder.display()
            )
            .as_str()
        )
    );
    assert!(images.is_empty(), "nothing is uploaded for a linked file");
}

#[gpui::test]
fn a_new_thread_starts_with_the_model_last_used(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, vcx) = forge.open(cx, target.clone());
    let mut picked = EngineConfig::new(EngineId::Claude);
    picked.model = Some("opus".into());
    picked.effort = Some("high".into());
    picked.permissions = artisan_api::Permissions::Full;
    let chosen = picked.clone();
    composer.update(vcx, |composer, cx| composer.choose_config(chosen, cx));
    // Choosing only changes the draft. Acceptance remembers the actual run config.
    vcx.simulate_input("remember this model");
    vcx.simulate_keystrokes("enter");
    let expected = serde_json::to_value(&picked).unwrap();
    wait(vcx, "Forge to keep the choice as its default", |_| {
        forge
            .forge
            .block_on(forge.store.setting("default_engine_config"))
            .unwrap()
            .as_ref()
            == Some(&expected)
    });

    // The Editor restarts: a new Core reads the same files.
    let files = forge.core.read_with(vcx, |core, _| core.files.clone());
    vcx.update(|_, app| editor_sources::init(app));
    let core = vcx.update(|_, app| app.new(|cx| Core::new(files, cx).unwrap()));
    let (fresh, vcx) =
        cx.add_window_view(move |window, cx| Composer::new(core, target, window, cx));
    vcx.simulate_resize(size(px(800.0), px(600.0)));
    let config =
        |vcx: &mut VisualTestContext| vcx.read(|app| fresh.read(app).effective_config(app));
    wait(vcx, "the new Core to follow Forge", |vcx| {
        config(vcx).is_some_and(|config| config.model.is_some())
    });
    assert_eq!(config(vcx), Some(picked.clone()), "the remembered settings");

    // Sending creates the thread with them.
    vcx.update(|window, app| {
        let focus = fresh.read(app).focus_handle(app);
        window.focus(&focus, app);
    });
    vcx.simulate_input("start");
    vcx.simulate_keystrokes("enter");
    let core = fresh.read_with(vcx, |fresh, _| fresh.core().clone());
    let mut thread = None;
    wait(vcx, "the new thread", |vcx| {
        thread = vcx.read(|app| core.read(app).screen().thread().cloned());
        thread.is_some()
    });
    let stored = forge
        .forge
        .block_on(forge.store.thread(&thread.unwrap()))
        .unwrap()
        .expect("Forge made the thread");
    assert_eq!(stored.config, picked);
}

/// Each model keeps the effort, permissions and the rest last chosen for
/// it: switching away and back, and after the Editor restarts, it comes
/// back as it was rather than at its defaults.
#[gpui::test]
fn each_model_comes_back_as_it_was_last_chosen(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, vcx) = forge.open(cx, target.clone());
    wait(vcx, "the model catalog", |vcx| {
        composer.read_with(vcx, |composer, cx| {
            composer.host(cx).is_some_and(|host| {
                host.read(cx)
                    .engines
                    .get(&EngineId::Claude)
                    .is_some_and(|engine| !engine.models.is_empty())
            })
        })
    });
    let mut opus = EngineConfig::new(EngineId::Claude);
    opus.model = Some("opus".into());
    opus.effort = Some("high".into());
    opus.permissions = artisan_api::Permissions::Full;
    let mut sonnet = EngineConfig::new(EngineId::Claude);
    sonnet.model = Some("sonnet".into());
    sonnet.effort = Some("low".into());
    let config = |vcx: &mut VisualTestContext, composer: &Entity<Composer>| {
        composer.read_with(vcx, |composer, cx| composer.effective_config(cx).unwrap())
    };
    composer.update(vcx, |composer, cx| composer.choose_config(opus.clone(), cx));
    composer.update(vcx, |composer, cx| {
        composer.choose_model(EngineId::Claude, "sonnet", cx);
    });
    assert_eq!(
        config(vcx, &composer).effort.as_deref(),
        Some("medium"),
        "a model never chosen starts at its default effort"
    );
    composer.update(vcx, |composer, cx| {
        composer.choose_config(sonnet.clone(), cx);
    });
    wait(vcx, "Forge to remember both models", |vcx| {
        composer.read_with(vcx, |composer, cx| {
            let host = composer.host(cx).unwrap();
            let host = host.read(cx);
            crate::saved_model_config(host, EngineId::Claude, "opus").as_ref() == Some(&opus)
                && crate::saved_model_config(host, EngineId::Claude, "sonnet").as_ref()
                    == Some(&sonnet)
        })
    });
    composer.update(vcx, |composer, cx| {
        composer.choose_model(EngineId::Claude, "opus", cx);
    });
    assert_eq!(config(vcx, &composer), opus, "Opus comes back as chosen");

    // The Editor restarts: a fresh picker still knows Sonnet's settings.
    let files = forge.core.read_with(vcx, |core, _| core.files.clone());
    vcx.update(|_, app| editor_sources::init(app));
    let core = vcx.update(|_, app| app.new(|cx| Core::new(files, cx).unwrap()));
    let (fresh, vcx) =
        cx.add_window_view(move |window, cx| Composer::new(core, target, window, cx));
    wait(vcx, "the new Core to follow Forge", |vcx| {
        fresh.read_with(vcx, |fresh, cx| {
            fresh.draft_editable(cx)
                && fresh.host(cx).is_some_and(|host| {
                    crate::saved_model_config(host.read(cx), EngineId::Claude, "sonnet").is_some()
                })
        })
    });
    fresh.update(vcx, |fresh, cx| {
        fresh.choose_model(EngineId::Claude, "opus", cx);
        fresh.choose_model(EngineId::Claude, "sonnet", cx);
    });
    assert_eq!(config(vcx, &fresh), sonnet, "Sonnet comes back as chosen");
}

/// A new thread opened without a project starts in the one the last
/// thread was started in.
#[gpui::test]
fn a_new_thread_starts_in_the_project_last_used(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: None,
    };
    let (composer, vcx) = forge.open(cx, target.clone());
    assert_eq!(
        composer.read_with(vcx, Composer::project),
        None,
        "nothing was started yet"
    );
    let project = forge.project.clone();
    composer.update(vcx, |composer, cx| composer.choose_project(project, cx));
    vcx.simulate_input("start here");
    vcx.simulate_keystrokes("enter");
    let core = forge.core.clone();
    wait(vcx, "the new thread", |vcx| {
        vcx.read(|app| core.read(app).screen().thread().is_some())
    });

    let core = forge.core.clone();
    let (next, vcx) = cx.add_window_view(move |window, cx| Composer::new(core, target, window, cx));
    wait(vcx, "the next thread to start in that project", |vcx| {
        next.read_with(vcx, Composer::project) == Some(forge.project.clone())
    });
}

#[gpui::test]
fn hovering_a_model_previews_and_edits_its_settings(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, vcx) = forge.open(cx, target);
    let mut sonnet = EngineConfig::new(EngineId::Claude);
    sonnet.model = Some("sonnet".into());
    composer.update(vcx, |composer, cx| {
        composer.choose_config(sonnet, cx);
        composer.open_menu(crate::Menu::Model, cx);
    });
    let top = |vcx: &mut VisualTestContext, selector: &'static str| {
        vcx.run_until_parked();
        vcx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is on screen"))
            .top()
    };
    wait(vcx, "the model menu", |vcx| {
        vcx.debug_bounds("composer-model-row-opus").is_some()
    });
    assert!(
        top(vcx, "composer-model-row-sonnet") < top(vcx, "composer-model-row-opus"),
        "the chosen model lists first"
    );

    assert!(vcx.debug_bounds("composer-model-favorite-opus").is_none());
    let row = vcx.debug_bounds("composer-model-row-opus").unwrap();
    vcx.simulate_mouse_move(row.center(), None, gpui::Modifiers::none());
    vcx.run_until_parked();
    composer.read_with(vcx, |composer, cx| {
        assert_eq!(
            composer.preview_config(cx).unwrap().model.as_deref(),
            Some("opus")
        );
        assert_eq!(
            composer.effective_config(cx).unwrap().model.as_deref(),
            Some("sonnet")
        );
    });
    let control = vcx.debug_bounds("composer-axis-Thinking-trigger").unwrap();
    vcx.simulate_mouse_move(control.center(), None, gpui::Modifiers::none());
    vcx.simulate_click(control.center(), gpui::Modifiers::none());
    vcx.run_until_parked();
    vcx.simulate_keystrokes("end enter escape");
    composer.read_with(vcx, |composer, cx| {
        let config = composer.effective_config(cx).unwrap();
        assert_eq!(config.model.as_deref(), Some("opus"));
        assert_eq!(config.effort.as_deref(), Some("high"));
    });
}

#[gpui::test]
fn the_project_menu_chooses_a_project_or_asks_for_a_new_one(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: None,
    };
    let (composer, vcx) = forge.open(cx, target);
    let asked = Rc::new(std::cell::Cell::new(false));
    let noted = Rc::clone(&asked);
    let _subscription = vcx.update(|_, app| {
        app.subscribe(&composer, move |_, event: &ComposerEvent, _| {
            if matches!(event, ComposerEvent::NewProject) {
                noted.set(true);
            }
        })
    });
    let click = |vcx: &mut VisualTestContext, selector: &'static str| {
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let bounds = vcx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is on screen"));
        vcx.simulate_click(bounds.center(), gpui::Modifiers::none());
        vcx.run_until_parked();
    };

    // The tab opens the menu; a project row chooses that project.
    click(vcx, "composer-project");
    let row = format!("composer-project-row-{}", forge.project);
    click(vcx, Box::leak(row.into_boxed_str()));
    assert_eq!(
        composer.read_with(vcx, |composer, _| composer.project.clone()),
        Some(forge.project.clone())
    );

    // "No project" is its own row, never a project in the list, and
    // chooses Forge's scratch project.
    click(vcx, "composer-project");
    assert!(
        vcx.debug_bounds("composer-project-row-scratch").is_none(),
        "the scratch project is not listed as a project"
    );
    click(vcx, "composer-project-no-project");
    assert_eq!(
        composer.read_with(vcx, |composer, _| composer.project.clone()),
        Some(ProjectId::scratch())
    );
    assert!(composer.read_with(vcx, |composer, _| composer.menu.is_none()));

    // "New project" closes the menu and asks the shell for one.
    click(vcx, "composer-project");
    click(vcx, "composer-project-new-project");
    assert!(asked.get(), "the shell was asked for a new project");
    assert!(composer.read_with(vcx, |composer, _| composer.menu.is_none()));
}

#[gpui::test]
fn a_skill_chosen_from_the_mention_menu_is_sent_as_its_mention(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    forge.add_skill("review", "Reviews the current diff");
    forge.add_skill("apple-design", "Apple's approach to interface design");
    let thread = forge.thread();
    let target = ComposerTarget::Thread {
        host: forge.host.clone(),
        thread: thread.clone(),
    };
    let (composer, cx) = forge.open(cx, target);
    let (core, host) = (forge.core.clone(), forge.host.clone());
    wait(cx, "the thread", |cx| {
        knows_thread(cx, &core, &host, &thread)
    });

    cx.simulate_input("$re");
    let offered =
        |cx: &mut VisualTestContext| cx.read(|app| composer.read(app).mention_menu_names());
    wait(cx, "the menu to offer the skill", |cx| {
        offered(cx) == ["review"]
    });
    assert!(
        cx.debug_bounds(mentions::MENU_SELECTOR).is_some(),
        "the menu is painted over the composer"
    );

    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(text(cx, &composer), "$review ", "enter wrote the mention");
    assert!(
        offered(cx).is_empty(),
        "the written mention closes the menu"
    );
    assert!(
        forge.user_messages(&thread).is_empty(),
        "choosing a row never sends"
    );

    cx.simulate_input("this diff");
    cx.simulate_keystrokes("enter");
    wait(cx, "the message", |_| {
        !forge.user_messages(&thread).is_empty()
    });
    let messages = forge.user_messages(&thread);
    assert!(
        matches!(&messages[0].kind, ItemKind::UserMessage { text, .. } if text.inline() == Some("$review this diff")),
        "{messages:?}"
    );
}

/// `/` at the start of a message offers what the engine said it reads (here
/// Claude's `initialize` reply); the same `/` later in a message is just a
/// slash.
#[gpui::test]
fn the_command_menu_offers_the_engines_own_commands_at_the_start(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let thread = forge.thread();
    let (composer, cx) = forge.open(
        cx,
        ComposerTarget::Thread {
            host: forge.host.clone(),
            thread: thread.clone(),
        },
    );
    let (core, host) = (forge.core.clone(), forge.host.clone());
    wait(cx, "the thread", |cx| {
        knows_thread(cx, &core, &host, &thread)
    });
    let offered =
        |cx: &mut VisualTestContext| cx.read(|app| composer.read(app).mention_menu_names());
    cx.simulate_input("/con");
    wait(cx, "the engine's commands", |cx| offered(cx) == ["context"]);
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(text(cx, &composer), "/context ");
    cx.simulate_input("then /co");
    cx.run_until_parked();
    assert!(offered(cx).is_empty(), "a command only starts a message");
}

#[gpui::test]
fn the_mention_menu_offers_artisans_skills_but_not_ones_turned_off(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    forge.add_skill("review", "Reviews the current diff");
    forge.add_skill("release", "Ships it");
    forge
        .forge
        .block_on(forge.store.set_setting(
            "disabled_skills".into(),
            serde_json::json!(["local.release"]),
        ))
        .unwrap();
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, cx) = forge.open(cx, target);

    cx.simulate_input("$");
    wait(cx, "the menu to offer every skill the engines get", |cx| {
        cx.read(|app| composer.read(app).mention_menu_names()) == ["artisan-editor", "review"]
    });
}

/// `@` names other threads by title (and connectors), never skills; `$` is
/// only for skills. A thread dragged from the sidebar lands as the same
/// mention.
#[gpui::test]
fn at_mentions_other_threads_and_dollar_only_skills(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    forge.add_skill("review", "Reviews the current diff");
    let titled = |title: &str| {
        let thread = forge.thread();
        let update = UpdateThread {
            title: Some(title.to_owned()),
            ..Default::default()
        };
        forge
            .forge
            .block_on(forge.store.update_thread(&thread, update))
            .unwrap();
        thread
    };
    let other = titled("Fix sidebar lag");
    let thread = titled("Current work");
    let target = ComposerTarget::Thread {
        host: forge.host.clone(),
        thread: thread.clone(),
    };
    let (composer, cx) = forge.open(cx, target);
    let (core, host) = (forge.core.clone(), forge.host.clone());
    wait(cx, "both threads", |cx| {
        knows_thread(cx, &core, &host, &thread) && knows_thread(cx, &core, &host, &other)
    });
    let offered =
        |cx: &mut VisualTestContext| cx.read(|app| composer.read(app).mention_menu_names());

    cx.simulate_input("@");
    wait(cx, "the other thread, not this one or a skill", |cx| {
        offered(cx) == ["Fix sidebar lag"]
    });
    cx.simulate_input("fix");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(text(cx, &composer), "@Fix sidebar lag ");

    cx.simulate_input("$");
    wait(cx, "only skills after $", |cx| {
        offered(cx) == ["artisan-editor", "review"]
    });
    cx.simulate_keystrokes("escape backspace");
    cx.run_until_parked();

    let dragged = DraggedThread {
        host: forge.host.clone(),
        thread: other.clone(),
        title: "Fix sidebar lag".into(),
    };
    cx.update(|window, app| {
        composer.update(app, |composer, cx| {
            composer.mention_thread(&dragged, window, cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(text(cx, &composer), "@Fix sidebar lag @Fix sidebar lag ");
}

#[gpui::test]
fn backspace_removes_a_mention_whole(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    forge.add_skill("review", "Reviews the current diff");
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, cx) = forge.open(cx, target);

    cx.simulate_input("hi $review");
    wait(cx, "the menu to offer the skill", |cx| {
        cx.read(|app| composer.read(app).mention_menu_names()) == ["review"]
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(
        cx.read(|app| composer.read(app).mention_menu_names())
            .is_empty(),
        "escape closes the menu"
    );
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    assert_eq!(text(cx, &composer), "hi ");
}

#[gpui::test]
fn choosing_fast_runs_the_thread_at_that_speed(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, vcx) = forge.open(cx, target);
    let mut sonnet = EngineConfig::new(EngineId::Claude);
    sonnet.model = Some("sonnet".into());
    composer.update(vcx, |composer, cx| {
        composer.choose_config(sonnet, cx);
        composer.open_menu(crate::Menu::Model, cx);
    });
    let click = |vcx: &mut VisualTestContext, selector: &'static str| {
        let mut bounds = None;
        wait(vcx, selector, |vcx| {
            bounds = vcx.debug_bounds(selector);
            bounds.is_some()
        });
        vcx.simulate_click(bounds.unwrap().center(), gpui::Modifiers::none());
        vcx.run_until_parked();
    };
    // Sonnet's speeds: Standard, the default, then Fast.
    click(vcx, "composer-axis-Speed-trigger");
    vcx.simulate_keystrokes("end enter escape");
    assert_eq!(
        composer.read_with(vcx, |composer, cx| composer
            .effective_config(cx)
            .and_then(|config| config.speed)),
        Some("fast".into())
    );

    vcx.update(|window, app| {
        let focus = composer.read(app).focus_handle(app);
        window.focus(&focus, app);
    });
    vcx.simulate_input("go fast");
    vcx.simulate_keystrokes("enter");
    let core = forge.core.clone();
    let mut thread = None;
    wait(vcx, "the new thread", |vcx| {
        thread = vcx.read(|app| core.read(app).screen().thread().cloned());
        thread.is_some()
    });
    let thread = thread.unwrap();
    wait(vcx, "its run", |_| !runs_of(&thread).is_empty());
    assert_eq!(
        runs_of(&thread)[0].speed.as_deref(),
        Some("fast"),
        "the engine runs in fast mode"
    );
    let stored = forge
        .forge
        .block_on(forge.store.thread(&thread))
        .unwrap()
        .expect("Forge made the thread");
    assert_eq!(stored.config.speed.as_deref(), Some("fast"));
}

#[gpui::test]
fn a_resting_pointer_shows_an_options_description_beside_it(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, vcx) = forge.open(cx, target);
    let mut sonnet = EngineConfig::new(EngineId::Claude);
    sonnet.model = Some("sonnet".into());
    composer.update(vcx, |composer, cx| {
        composer.choose_config(sonnet, cx);
        composer.open_menu(crate::Menu::Model, cx);
    });
    let trigger = "composer-axis-Permission-trigger";
    let mut bounds = None;
    wait(vcx, trigger, |vcx| {
        bounds = vcx.debug_bounds(trigger);
        bounds.is_some()
    });
    vcx.simulate_click(bounds.unwrap().center(), gpui::Modifiers::none());
    // Let the menu and its dropdown finish sliding in.
    vcx.executor().advance_clock(Duration::from_millis(300));
    vcx.run_until_parked();
    let option = "composer-axis-Permission-1";
    let mut row = None;
    wait(vcx, option, |vcx| {
        row = vcx.debug_bounds(option);
        row.is_some()
    });
    let row = row.unwrap();
    vcx.simulate_mouse_move(row.center(), None, gpui::Modifiers::none());
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("composer-picker-tooltip").is_none(),
        "the description waits for the pointer to rest"
    );
    vcx.executor()
        .advance_clock(editor_ui::hover_card::TOOLTIP_DELAY + Duration::from_millis(50));
    vcx.run_until_parked();
    let tooltip = vcx
        .debug_bounds("composer-picker-tooltip")
        .expect("the description shows once the pointer rests");
    assert!(
        tooltip.left() >= row.right() || tooltip.right() <= row.left(),
        "the description sits beside the option, not over it"
    );
    vcx.simulate_mouse_move(
        gpui::point(gpui::px(1.0), gpui::px(1.0)),
        None,
        gpui::Modifiers::none(),
    );
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("composer-picker-tooltip").is_none(),
        "leaving the option hides its description"
    );
}

#[gpui::test]
fn pasting_the_same_image_twice_attaches_it_once(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, cx) = forge.open(cx, target);
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(32, 32, |x, y| {
        image::Rgba([
            u8::try_from(x * 8).unwrap(),
            u8::try_from(y * 8).unwrap(),
            9,
            255,
        ])
    }))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    let paste = |cx: &mut VisualTestContext| {
        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
            ImageFormat::Png,
            png.clone(),
        )));
        cx.simulate_keystrokes("ctrl-v");
    };
    paste(cx);
    wait(cx, "the first image to prepare", |cx| {
        cx.read(|app| {
            composer
                .read(app)
                .attachments
                .iter()
                .any(|a| a.preview.is_some())
        })
    });
    paste(cx);
    wait(cx, "the repeat to be refused", |cx| {
        cx.read(|app| {
            let composer = composer.read(app);
            composer.shake.is_some() && composer.attachments.len() == 1
        })
    });
}

/// A new thread starts in the selected project; its tab can change the project.
#[gpui::test]
fn a_new_threads_project_can_be_switched(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let other_dir = forge.dir.path().join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let other = forge
        .forge
        .block_on(forge.store.create_project(CreateProject {
            id: ProjectId::new(),
            path: other_dir.display().to_string(),
            name: None,
        }))
        .unwrap()
        .id;
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, vcx) = forge.open(cx, target);
    let (core, host, known) = (forge.core.clone(), forge.host.clone(), other.clone());
    wait(vcx, "the second project", move |cx| {
        cx.read(|app| {
            core.read(app)
                .host(&host)
                .is_some_and(|host| host.read(app).projects.contains_key(&known))
        })
    });
    let starts = composer.read_with(vcx, Composer::project);
    assert_eq!(
        starts,
        Some(forge.project.clone()),
        "it starts where it was opened"
    );
    let click = |vcx: &mut VisualTestContext, selector: String| {
        // `debug_bounds` takes a static selector.
        let selector: &'static str = Box::leak(selector.into_boxed_str());
        let mut bounds = None;
        wait(vcx, selector, |vcx| {
            vcx.update(|window, _| window.refresh());
            bounds = vcx.debug_bounds(selector);
            bounds.is_some()
        });
        vcx.simulate_click(bounds.unwrap().center(), gpui::Modifiers::none());
        vcx.run_until_parked();
    };
    click(vcx, "composer-project".to_owned());
    click(vcx, format!("composer-project-row-{other}"));
    let chosen = composer.read_with(vcx, Composer::project);
    assert_eq!(
        chosen,
        Some(other),
        "the tab switched the new thread's project"
    );
}

#[gpui::test]
fn immediate_send_then_navigation_finishes_original_draft_once(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let first = fixture.thread();
    let second = fixture.thread();
    let (composer, cx) = fixture.open(
        cx,
        ComposerTarget::Thread {
            host: fixture.host.clone(),
            thread: first.clone(),
        },
    );
    wait(cx, "both thread summaries", |cx| {
        knows_thread(cx, &fixture.core, &fixture.host, &first)
            && knows_thread(cx, &fixture.core, &fixture.host, &second)
    });
    cx.simulate_input("send without waiting for debounce");
    cx.update(|window, app| {
        composer.update(app, |composer, cx| {
            composer.send(cx);
            composer.set_target(
                ComposerTarget::Thread {
                    host: fixture.host.clone(),
                    thread: second.clone(),
                },
                window,
                cx,
            );
        });
    });
    cx.simulate_input("second draft stays here");
    wait(cx, "original acceptance", |_| {
        fixture.user_messages(&first).len() == 1
            && fixture.draft(&format!("thread:{first}")).is_none()
    });
    assert_eq!(text(cx, &composer), "second draft stays here");
    cx.update(|window, app| {
        composer.update(app, |composer, cx| {
            composer.set_target(
                ComposerTarget::Thread {
                    host: fixture.host.clone(),
                    thread: first.clone(),
                },
                window,
                cx,
            );
        });
    });
    wait(cx, "the accepted scope is empty", |cx| {
        text(cx, &composer).is_empty()
    });
    assert_eq!(fixture.user_messages(&first).len(), 1);
}

#[gpui::test]
fn selected_project_survives_draft_restore(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    std::fs::create_dir_all(fixture.dir.path().join("different")).unwrap();
    let second = fixture
        .forge
        .block_on(fixture.store.create_project(CreateProject {
            id: ProjectId::new(),
            path: fixture.dir.path().join("different").display().to_string(),
            name: Some("Second".into()),
        }))
        .unwrap()
        .id;
    let target = ComposerTarget::NewThread {
        host: fixture.host.clone(),
        project: Some(fixture.project.clone()),
    };
    let (composer, cx) = fixture.open(cx, target.clone());
    composer.update(cx, |composer, cx| {
        composer.project = Some(second.clone());
        composer.edited(cx);
    });
    cx.simulate_input("carried draft");
    cx.executor().advance_clock(Duration::from_secs(1));
    wait(cx, "project in draft", |_| {
        fixture
            .draft(&format!("new:{}", fixture.project))
            .is_some_and(|body| body["project"] == second.as_str())
    });
    cx.update(|window, app| {
        composer.update(app, |composer, cx| {
            composer.set_target(
                ComposerTarget::NewThread {
                    host: fixture.host.clone(),
                    project: None,
                },
                window,
                cx,
            );
            composer.set_target(target.clone(), window, cx);
        });
    });
    assert_eq!(composer.read_with(cx, Composer::project), Some(second));
    assert_eq!(text(cx, &composer), "carried draft");
}

#[gpui::test]
fn a_concurrent_draft_is_kept_until_the_reader_resolves_the_conflict(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let thread = fixture.thread();
    let target = ComposerTarget::Thread {
        host: fixture.host.clone(),
        thread: thread.clone(),
    };
    let (composer, cx) = fixture.open(cx, target);
    wait(cx, "initial draft snapshot", |cx| {
        cx.read(|cx| {
            composer
                .read(cx)
                .session
                .as_ref()
                .is_some_and(|session| session.read(cx).loaded)
        })
    });
    let scope = format!("thread:{thread}");
    let snapshot = fixture.forge.block_on(fixture.store.draft(&scope)).unwrap();
    let remote = serde_json::json!({"text":"remote draft","images":[]});
    fixture
        .forge
        .block_on(fixture.store.set_draft(
            scope.clone(),
            artisan_api::WriteDraft {
                expected_revision: snapshot.revision,
                body: remote.clone(),
            },
        ))
        .unwrap();
    composer.update(cx, |this, cx| {
        this.editor
            .update(cx, |editor, cx| editor.set_text("local draft", cx));
        this.has_text = true;
        this.edited(cx);
        this.session
            .as_ref()
            .unwrap()
            .update(cx, |session, cx| session.flush(cx));
    });
    wait(cx, "draft conflict", |cx| {
        cx.read(|cx| {
            composer
                .read(cx)
                .session
                .as_ref()
                .unwrap()
                .read(cx)
                .conflict
        })
    });
    assert_eq!(fixture.draft(&scope), Some(remote));
    composer.update(cx, |this, cx| {
        this.session
            .as_ref()
            .unwrap()
            .update(cx, |session, cx| session.keep_local(cx));
    });
    wait(cx, "local draft resolution", |_| {
        fixture
            .draft(&scope)
            .is_some_and(|body| body["text"] == "local draft")
    });
    assert_eq!(
        fixture
            .forge
            .block_on(fixture.store.draft(&scope))
            .unwrap()
            .revision,
        snapshot.revision + 2
    );
}

#[gpui::test]
fn submitted_draft_rejects_attachment_and_configuration_changes_until_settled(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new(cx);
    let thread = fixture.thread();
    let (composer, cx) = fixture.open(
        cx,
        ComposerTarget::Thread {
            host: fixture.host.clone(),
            thread,
        },
    );
    cx.simulate_input("submit one immutable draft");
    composer.update(cx, |composer, cx| {
        let original_config = composer.config.clone();
        composer.send(cx);
        assert!(composer.session.as_ref().unwrap().read(cx).sending());
        composer.add_image(crate::Source::Bytes(vec![1, 2, 3]), "late image".into(), cx);
        composer.choose_config(EngineConfig::new(EngineId::Codex), cx);
        composer.open_menu(crate::Menu::Model, cx);
        assert!(composer.attachments.is_empty());
        assert_eq!(composer.config, original_config);
        assert!(composer.menu.is_none());
    });
}

#[gpui::test]
fn removing_an_attachment_releases_its_uploaded_preview(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (composer, cx) = forge.open(cx, target);
    wait(cx, "the editable draft", |cx| {
        composer.read_with(cx, super::Composer::draft_editable)
    });
    let image = Arc::new(RenderImage::new(vec![image::Frame::new(
        image::RgbaImage::from_pixel(16, 12, image::Rgba([20, 30, 40, 255])),
    )]));
    composer.update(cx, |this, cx| {
        this.attachments.push(Attachment {
            key: 1,
            name: "image".into(),
            state: attachment::State::Encoding,
            preview: None,
            encoded: None,
            preview_error: None,
            revealed: None,
            shown: true,
            source: None,
            task: Task::ready(()),
        });
        this.admit_preview(1, std::sync::Arc::clone(&image), cx)
            .unwrap();
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        assert!(window.has_image_atlas_entry(&image));
    });
    composer.update(cx, |this, cx| this.remove_attachment(1, cx));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        assert!(!window.has_image_atlas_entry(&image));
    });
    assert!(composer.read_with(cx, |this, _| this.attachments.is_empty()));
}

#[gpui::test]
fn restored_preview_failure_is_retryable_without_losing_blob(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let thread = fixture.thread();
    let (composer, cx) = fixture.open(
        cx,
        ComposerTarget::Thread {
            host: fixture.host.clone(),
            thread: thread.clone(),
        },
    );
    let pixels = image::RgbaImage::from_pixel(24, 24, image::Rgba([23, 72, 131, 255]));
    let encoded = webp::Encoder::from_rgba(pixels.as_raw(), 24, 24)
        .encode_lossless()
        .to_vec();
    let blob = fixture
        .forge
        .block_on(fixture.store.put_blob(encoded.clone()))
        .unwrap();
    let path = fixture.store.blob_path(&blob.hash).unwrap();
    std::fs::remove_file(&path).unwrap();
    composer.update(cx, |composer, cx| {
        composer.restore_attachment(blob.hash.clone(), cx);
    });
    wait(cx, "real missing-blob preview failure", |cx| {
        cx.read(|app| {
            composer
                .read(app)
                .attachments
                .first()
                .is_some_and(|a| a.preview_error.is_some())
        })
    });
    assert_eq!(
        cx.read(|app| composer.read(app).attachments[0].state.hash().cloned()),
        Some(blob.hash.clone())
    );
    assert!(cx.read(|app| composer.read(app).attachments[0].preview.is_none()));
    // Restore the same remote blob; retry must bypass the cached failure.
    std::fs::write(path, encoded).unwrap();
    let tile = cx.debug_bounds("composer-attachment-0").unwrap();
    cx.simulate_click(
        gpui::point(tile.left() + px(15.0), tile.bottom() - px(15.0)),
        gpui::Modifiers::none(),
    );
    wait(cx, "actual retry action decodes preview", |cx| {
        cx.read(|app| {
            composer.read(app).attachments[0].preview.is_some()
                && composer.read(app).attachments[0].preview_error.is_none()
        })
    });
    assert_eq!(
        cx.read(|app| composer.read(app).attachments[0].state.hash().cloned()),
        Some(blob.hash.clone())
    );
    cx.update(|window, app| window.focus(&composer.read(app).focus_handle(app), app));
    cx.simulate_input("send restored image");
    assert_eq!(text(cx, &composer), "send restored image");
    assert_eq!(cx.read(|app| composer.read(app).blocked(app)), None);
    cx.simulate_keystrokes("enter");
    wait(cx, "restored blob remains sendable", |_| {
        fixture.user_messages(&thread).len() == 1
    });
    let messages = fixture.user_messages(&thread);
    let ItemKind::UserMessage { images, .. } = &messages[0].kind else {
        unreachable!()
    };
    assert_eq!(images, &vec![blob.hash]);
}

fn stop_then_navigate(cx: &mut TestAppContext, fail: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let fixture = Fixture::new(cx);
    let original = fixture.thread();
    let next = fixture.thread();
    let (composer, cx) = fixture.open(
        cx,
        ComposerTarget::Thread {
            host: fixture.host.clone(),
            thread: original.clone(),
        },
    );
    wait(cx, "both summaries", |cx| {
        knows_thread(cx, &fixture.core, &fixture.host, &original)
            && knows_thread(cx, &fixture.core, &fixture.host, &next)
    });
    cx.simulate_input("hold the original run");
    assert_eq!(text(cx, &composer), "hold the original run");
    assert_eq!(cx.read(|app| composer.read(app).blocked(app)), None);
    cx.simulate_keystrokes("enter");
    wait(cx, "native Stop button", |cx| {
        cx.debug_bounds("composer-stop").is_some()
    });
    let arrived = Arc::new(AtomicBool::new(false));
    let (release, receive) = futures::channel::oneshot::channel();
    composer.update(cx, |composer, _| {
        composer.cancel_completion_gate = Some((std::sync::Arc::clone(&arrived), receive));
    });
    let stop = cx.debug_bounds("composer-stop").unwrap();
    cx.simulate_click(stop.center(), gpui::Modifiers::none());
    // A second Stop during pending cancellation must not create another RPC.
    cx.simulate_click(stop.center(), gpui::Modifiers::none());
    wait(cx, "original cancellation response held", |_| {
        arrived.load(Ordering::Acquire)
    });
    assert_eq!(
        cx.read(|app| composer.read(app).cancel_requests.clone()),
        vec![original.clone()]
    );
    cx.update(|window, app| {
        composer.update(app, |composer, cx| {
            composer.set_target(
                ComposerTarget::Thread {
                    host: fixture.host.clone(),
                    thread: next.clone(),
                },
                window,
                cx,
            );
            composer.notice = Some("new target notice".into());
            window.focus(&composer.focus_handle(cx), cx);
        });
    });
    cx.simulate_input("keep the new draft");
    assert_eq!(text(cx, &composer), "keep the new draft");
    release.send(fail).unwrap();
    wait(cx, "obsolete cancellation completion", |cx| {
        cx.read(|app| composer.read(app).cancel_completions == 1)
    });
    assert_eq!(text(cx, &composer), "keep the new draft");
    assert_eq!(
        cx.read(|app| composer.read(app).notice.clone()),
        Some("new target notice".into())
    );
    assert!(!cx.read(|app| composer.read(app).cancelling));
    assert_eq!(
        cx.read(|app| composer.read(app).cancel_requests.clone()),
        vec![original]
    );
    assert!(fixture.user_messages(&next).is_empty());
}

#[gpui::test]
fn repeated_stop_then_navigation_ignores_original_cancel_failure(cx: &mut TestAppContext) {
    stop_then_navigate(cx, true);
}

#[gpui::test]
fn repeated_stop_then_navigation_ignores_original_cancel_success(cx: &mut TestAppContext) {
    stop_then_navigate(cx, false);
}

/// After Stop, the empty composer's button carries the turn on: the engine
/// runs again, and the user sees no message they didn't send.
#[gpui::test]
fn a_stopped_turn_continues_from_the_composer(cx: &mut TestAppContext) {
    let fixture = Fixture::new(cx);
    let thread = fixture.thread();
    let (composer, cx) = fixture.open(
        cx,
        ComposerTarget::Thread {
            host: fixture.host.clone(),
            thread: thread.clone(),
        },
    );
    wait(cx, "thread summary", |cx| {
        knows_thread(cx, &fixture.core, &fixture.host, &thread)
    });
    // A thread that never stopped has nothing to continue.
    assert!(cx.debug_bounds("composer-continue").is_none());
    cx.simulate_input("hold the run");
    cx.simulate_keystrokes("enter");
    wait(cx, "Stop button", |cx| {
        cx.debug_bounds("composer-stop").is_some()
    });
    let stop = cx.debug_bounds("composer-stop").unwrap();
    cx.simulate_click(stop.center(), gpui::Modifiers::none());
    wait(cx, "Continue button", |cx| {
        cx.debug_bounds("composer-continue").is_some()
    });
    // Typing turns it back into Send.
    cx.update(|window, app| window.focus(&composer.read(app).focus_handle(app), app));
    cx.simulate_input("x");
    wait(cx, "Send while there is text", |cx| {
        cx.debug_bounds("composer-continue").is_none()
    });
    cx.simulate_keystrokes("backspace");
    wait(cx, "Continue again once empty", |cx| {
        cx.debug_bounds("composer-continue").is_some()
    });
    let button = cx.debug_bounds("composer-continue").unwrap();
    cx.simulate_click(button.center(), gpui::Modifiers::none());
    // The fixture's engine holds again (the handoff repeats "hold"), so
    // the carried-on turn is live: Stop is back.
    wait(cx, "the turn carries on", |cx| {
        cx.debug_bounds("composer-stop").is_some()
    });
    let run = fixture
        .forge
        .block_on(fixture.store.latest_run(&thread))
        .unwrap()
        .unwrap();
    assert_eq!(run.continuation, Some(artisan_api::Continuation::Requested));
    assert_eq!(runs_of(&thread).len(), 2);
    assert_eq!(fixture.user_messages(&thread).len(), 1);
    assert!(cx.read(|app| composer.read(app).notice.is_none()));
}

#[gpui::test]
fn context_window_choice_reaches_the_run_and_standard_clears_the_override(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let (composer, vcx) = forge.open(
        cx,
        ComposerTarget::NewThread {
            host: forge.host.clone(),
            project: Some(forge.project.clone()),
        },
    );
    let mut sonnet = EngineConfig::new(EngineId::Claude);
    sonnet.model = Some("sonnet".into());
    composer.update(vcx, |composer, cx| {
        composer.choose_config(sonnet, cx);
        composer.open_menu(crate::Menu::Model, cx);
    });
    let click = |vcx: &mut VisualTestContext, selector: &'static str| {
        let mut bounds = None;
        wait(vcx, selector, |vcx| {
            bounds = vcx.debug_bounds(selector);
            bounds.is_some()
        });
        vcx.simulate_click(bounds.unwrap().center(), gpui::Modifiers::none());
        vcx.run_until_parked();
    };
    click(vcx, "composer-axis-Context-trigger");
    click(vcx, "composer-axis-Context-0");
    assert_eq!(
        composer.read_with(vcx, |composer, cx| composer
            .effective_config(cx)
            .unwrap()
            .context_window_override()),
        Some(500_000)
    );
    click(vcx, "composer-axis-Context-trigger");
    click(vcx, "composer-axis-Context-1");
    assert_eq!(
        composer.read_with(vcx, |composer, cx| composer
            .effective_config(cx)
            .unwrap()
            .context_window_override()),
        None
    );
    click(vcx, "composer-axis-Context-trigger");
    click(vcx, "composer-axis-Context-0");
    composer.update(vcx, super::Composer::close_menu);
    vcx.update(|window, app| window.focus(&composer.read(app).focus_handle(app), app));
    vcx.simulate_input("use the selected context window");
    vcx.simulate_keystrokes("enter");
    let mut thread = None;
    wait(vcx, "the configured run", |vcx| {
        thread = vcx.read(|cx| forge.core.read(cx).screen().thread().cloned());
        thread
            .as_ref()
            .is_some_and(|thread| !runs_of(thread).is_empty())
    });
    assert_eq!(
        runs_of(&thread.unwrap())[0].context_window_override(),
        Some(500_000)
    );
}

#[gpui::test]
fn model_picker_wheel_glides_once_and_hides_hover_outside_the_list(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let (composer, vcx) = forge.open(
        cx,
        ComposerTarget::NewThread {
            host: forge.host.clone(),
            project: Some(forge.project.clone()),
        },
    );
    wait(vcx, "the model catalog", |vcx| {
        composer.read_with(vcx, |composer, cx| {
            composer.host(cx).is_some_and(|host| {
                host.read(cx)
                    .engines
                    .get(&EngineId::Claude)
                    .is_some_and(|engine| !engine.models.is_empty())
            })
        })
    });
    composer.update(vcx, |composer, cx| {
        let host = composer.host(cx).unwrap();
        host.update(cx, |host, cx| {
            let models = &mut host.engines.get_mut(&EngineId::Claude).unwrap().models;
            let template = models[0].clone();
            let mut extended = models.to_vec();
            for index in 0..80 {
                let mut model = template.clone();
                model.id = format!("scroll-model-{index}");
                model.name = model.id.clone();
                extended.push(model);
            }
            *models = extended.into();
            cx.notify();
        });
        composer.open_menu(crate::Menu::Model, cx);
    });
    let mut viewport = None;
    wait(vcx, "the scrollable model list", |vcx| {
        viewport = vcx.debug_bounds("composer-model-viewport");
        viewport.is_some()
    });
    let viewport = viewport.unwrap();
    vcx.simulate_mouse_move(viewport.center(), None, gpui::Modifiers::none());
    let handle = composer.read_with(vcx, |composer, _| {
        composer.picker.model_scroll.0.borrow().base_handle.clone()
    });
    let step = vcx.update(|window, _| f32::from(window.line_height()) * 3.0);
    vcx.simulate_event(gpui::ScrollWheelEvent {
        position: viewport.center(),
        delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -3.0)),
        ..Default::default()
    });
    assert_eq!(
        handle.offset().y,
        px(0.0),
        "wheel scrolling starts with a glide, not an immediate native jump"
    );
    assert!(composer.read_with(vcx, |composer, _| composer.picker.model_glide.active()));
    wait(vcx, "the wheel glide to finish", |vcx| {
        std::thread::sleep(Duration::from_millis(10));
        vcx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
            window.draw(cx).clear(cx);
        });
        !composer.read_with(vcx, |composer, _| composer.picker.model_glide.active())
    });
    assert!(
        (f32::from(handle.offset().y) + step).abs() < 0.5,
        "the native list must consume the wheel distance exactly once"
    );
    vcx.simulate_mouse_move(
        viewport.origin - gpui::point(px(0.0), px(10.0)),
        None,
        gpui::Modifiers::none(),
    );
    assert!(!composer.read_with(vcx, |composer, _| { composer.picker.model_hover.visible() }));
    vcx.update(|_, cx| cx.set_reduce_motion(true));
    let before = handle.offset().y;
    vcx.simulate_event(gpui::ScrollWheelEvent {
        position: viewport.center(),
        delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -3.0)),
        ..Default::default()
    });
    assert!((f32::from(handle.offset().y - before) + step).abs() < 0.5);
    assert!(!composer.read_with(vcx, |composer, _| composer.picker.model_glide.active()));
}

/// Models served through several gateways list under a header each; only
/// the chosen model's gateway starts open, and a header opens or closes.
#[gpui::test]
fn gateway_groups_open_on_the_chosen_model_and_toggle(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let (composer, vcx) = forge.open(
        cx,
        ComposerTarget::NewThread {
            host: forge.host.clone(),
            project: Some(forge.project.clone()),
        },
    );
    wait(vcx, "the model catalog", |vcx| {
        composer.read_with(vcx, |composer, cx| {
            composer.host(cx).is_some_and(|host| {
                host.read(cx)
                    .engines
                    .get(&EngineId::Claude)
                    .is_some_and(|engine| !engine.models.is_empty())
            })
        })
    });
    composer.update(vcx, |composer, cx| {
        let host = composer.host(cx).unwrap();
        host.update(cx, |host, cx| {
            let models = &mut host.engines.get_mut(&EngineId::Claude).unwrap().models;
            let template = models[0].clone();
            *models = [("go-a", "Go"), ("go-b", "Go"), ("console-a", "Console")]
                .into_iter()
                .map(|(id, group)| artisan_api::Model {
                    id: id.to_owned(),
                    group: Some(group.to_owned()),
                    ..template.clone()
                })
                .collect::<Vec<_>>()
                .into();
            cx.notify();
        });
        composer.config = Some(EngineConfig {
            model: Some("console-a".to_owned()),
            ..EngineConfig::new(EngineId::Claude)
        });
        composer.open_menu(crate::Menu::Model, cx);
    });
    let rows = composer.read_with(vcx, |composer, _| composer.model_rows());
    assert_eq!(rows, ["-Go", "+Console", "console-a"]);
    composer.update(vcx, |composer, cx| {
        composer.toggle_group(EngineId::Claude, "Go".to_owned(), false, cx);
    });
    let rows = composer.read_with(vcx, |composer, _| composer.model_rows());
    assert_eq!(&rows[..3], ["+Go", "go-a", "go-b"]);

    // Forge remembers it: another picker on the host opens Go too.
    let core = forge.core.clone();
    let target = ComposerTarget::NewThread {
        host: forge.host.clone(),
        project: Some(forge.project.clone()),
    };
    let (other, vcx) =
        cx.add_window_view(move |window, cx| Composer::new(core, target, window, cx));
    other.update(vcx, |other, cx| {
        other.config = Some(EngineConfig {
            model: Some("console-a".to_owned()),
            ..EngineConfig::new(EngineId::Claude)
        });
        other.open_menu(crate::Menu::Model, cx);
    });
    wait(vcx, "Forge to remember the open group", |vcx| {
        other.update(vcx, |other, cx| {
            other.refresh_picker_catalog(cx);
            other.model_rows().first().map(String::as_str) == Some("+Go")
        })
    });
}

/// `/ultracode` is part of the message it starts, not a setting: it sends
/// as typed and runs a turn like any other message.
#[gpui::test]
fn ultracode_sends_as_a_message_of_its_own_turn(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let thread = forge.thread();
    let (composer, vcx) = forge.open(
        cx,
        ComposerTarget::Thread {
            host: forge.host.clone(),
            thread: thread.clone(),
        },
    );
    wait(vcx, "the command thread", |vcx| {
        knows_thread(vcx, &forge.core, &forge.host, &thread)
    });
    let mut config = EngineConfig::new(EngineId::Claude);
    config.model = Some("sonnet".into());
    composer.update(vcx, |composer, cx| composer.choose_config(config, cx));
    vcx.simulate_input("/ultracode build the parser");
    vcx.simulate_keystrokes("enter");
    wait(vcx, "the turn it starts", |_| !runs_of(&thread).is_empty());
    let sent: Vec<_> = forge
        .user_messages(&thread)
        .into_iter()
        .filter_map(|item| match item.kind {
            ItemKind::UserMessage { text, .. } => text.inline().map(str::to_owned),
            ItemKind::AssistantMessage { .. }
            | ItemKind::Tool { .. }
            | ItemKind::Agent { .. }
            | ItemKind::Question { .. }
            | ItemKind::Plan { .. }
            | ItemKind::Image { .. }
            | ItemKind::Compaction { .. }
            | ItemKind::Notice { .. }
            | ItemKind::Error { .. } => None,
        })
        .collect();
    assert_eq!(sent, ["/ultracode build the parser"]);
    wait(vcx, "the composer to clear", |vcx| {
        text(vcx, &composer).is_empty()
    });
}

#[gpui::test]
fn effort_header_is_not_a_keyboard_choice(cx: &mut TestAppContext) {
    // This test targets dropdown semantics; motion has separate native coverage.
    cx.update(|cx| cx.set_reduce_motion(true));
    let forge = Fixture::new(cx);
    let (composer, vcx) = forge.open(
        cx,
        ComposerTarget::NewThread {
            host: forge.host.clone(),
            project: Some(forge.project.clone()),
        },
    );
    wait(vcx, "the effort model", |vcx| {
        composer.read_with(vcx, |composer, cx| {
            composer.host(cx).is_some_and(|host| {
                host.read(cx)
                    .engines
                    .get(&EngineId::Claude)
                    .is_some_and(|engine| !engine.models.is_empty())
            })
        })
    });
    composer.update(vcx, |composer, cx| {
        let mut config = EngineConfig::new(EngineId::Claude);
        config.model = Some("sonnet".into());
        config.effort = Some("low".into());
        composer.choose_config(config, cx);
        composer.open_menu(crate::Menu::Model, cx);
    });
    let mut trigger = None;
    wait(vcx, "the effort control", |vcx| {
        trigger = vcx.debug_bounds("composer-axis-Thinking-trigger");
        trigger.is_some()
    });
    vcx.simulate_click(trigger.unwrap().center(), gpui::Modifiers::none());
    wait(vcx, "the effort heading", |vcx| {
        vcx.debug_bounds("composer-axis-Thinking-header").is_some()
    });
    vcx.simulate_keystrokes("end enter");
    assert_eq!(
        composer.read_with(vcx, |composer, cx| composer
            .effective_config(cx)
            .unwrap()
            .effort),
        Some("high".into())
    );
}

/// After a reply, the engine's guess at the next message stands in for the
/// placeholder; Tab writes it into the draft, and sending it withdraws it.
#[gpui::test]
fn tab_writes_the_suggested_next_message(cx: &mut TestAppContext) {
    let forge = Fixture::new(cx);
    let thread = forge.thread();
    let target = ComposerTarget::Thread {
        host: forge.host.clone(),
        thread: thread.clone(),
    };
    let (composer, cx) = forge.open(cx, target);
    let (core, host) = (forge.core.clone(), forge.host.clone());
    wait(cx, "the thread", |cx| {
        knows_thread(cx, &core, &host, &thread)
    });
    let shown = |cx: &mut VisualTestContext| {
        cx.read(|app| {
            composer
                .read(app)
                .editor
                .read(app)
                .shown_suggestion()
                .map(ToString::to_string)
        })
    };

    cx.simulate_input("parse it, then suggest what's next");
    cx.simulate_keystrokes("enter");
    wait(cx, "the suggestion after the reply", |cx| {
        shown(cx).as_deref() == Some("Now the lexer")
    });

    cx.simulate_keystrokes("tab");
    assert_eq!(text(cx, &composer), "Now the lexer");
    assert_eq!(shown(cx), None, "a draft with text shows no suggestion");

    cx.simulate_keystrokes("enter");
    wait(cx, "the suggestion to be sent", |_| {
        forge.user_messages(&thread).iter().any(|item| {
            matches!(&item.kind, ItemKind::UserMessage { text, .. } if text.inline() == Some("Now the lexer"))
        })
    });
    wait(cx, "the next run to withdraw it", |cx| {
        cx.read(|app| {
            core.read(app).host(&host).is_some_and(|host| {
                let host = host.read(app);
                !host.is_working(&thread) && host.suggestion(&thread).is_none()
            })
        })
    });
    assert_eq!(shown(cx), None);
}
