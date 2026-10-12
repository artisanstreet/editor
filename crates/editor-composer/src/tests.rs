//! The composer against a real in-process Forge with a scripted engine:
//! what the user types and pastes is what Forge stores.

use super::*;
use artisan_api::{CreateProject, EngineId, Item, ItemKind, ItemsQuery, Phase, Status};
use editor_core::EditorFiles;
use editor_core::hosts::{self, HostKind, HostsFile};
use editor_core::runtime::runtime;
use forge_engines::{EngineEvent, Harness, Input, Outcome, RunIo, Turn};
use futures::future::BoxFuture;
use gpui::{ClipboardItem, TestAppContext, VisualTestContext, size};
use std::rc::Rc;
use std::time::Instant;

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
            if turn.input.text.contains("hold") {
                tokio::select! {
                    input = io.inbox.recv() => {
                        if let Some(Input::Steer(steer)) = input {
                            io.emit(EngineEvent::SteerAccepted { id: steer.id }).await;
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
            Outcome::done()
        })
    }
}

struct Fixture {
    core: Entity<Core>,
    host: HostId,
    project: ProjectId,
    store: forge_store::Store,
    forge: tokio::runtime::Runtime,
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(cx: &mut TestAppContext) -> Self {
        // Forge and the network runtime wake GPUI tasks from their threads.
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("forge");
        // A fake installed Claude so runs get past "not installed".
        let version = data.join("engines/claude/versions/1.0.0");
        std::fs::create_dir_all(&version).unwrap();
        // Signed in, as far as `claude auth status` tells.
        let binary = version.join("claude");
        std::fs::write(
            &binary,
            "#!/bin/sh\necho '{\"loggedIn\":true,\"email\":\"test@example.com\"}'\n",
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
                { "id": "sonnet", "name": "Sonnet", "thinking": { "type": "adaptive",
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

        let files = EditorFiles {
            dir: dir.path().join("editor"),
        };
        let identity = files.identity().unwrap();
        let invite = forge.block_on(forge_http::invite(&store)).unwrap();
        let fingerprint = forge_http::tls::fingerprint_of(&invite.forge_certificate).unwrap();
        let host = runtime()
            .block_on(hosts::pair(
                &address,
                &invite.code,
                &fingerprint,
                &identity,
                "test",
                "Forge".into(),
                HostKind::Local,
            ))
            .unwrap();
        files
            .save_hosts(&HostsFile {
                hosts: vec![host.clone()],
                last: Some(host.id.clone()),
            })
            .unwrap();
        let project = forge
            .block_on(store.create_project(CreateProject {
                id: ProjectId::new(),
                path: dir.path().display().to_string(),
                name: None,
            }))
            .unwrap()
            .id;
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
        self.forge.block_on(self.store.draft(scope)).unwrap()
    }

    fn open<'a>(
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

fn knows_thread(
    cx: &mut VisualTestContext,
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

fn text(cx: &mut VisualTestContext, composer: &Entity<Composer>) -> String {
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
        matches!(&messages[0].kind, ItemKind::UserMessage { text, steer: false, .. } if text == "hello forge"),
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
    let counted = sent.clone();
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

    cx.simulate_input("also this");
    cx.simulate_keystrokes("enter");
    wait(cx, "the steer", |_| forge.user_messages(&thread).len() == 2);
    wait(cx, "the composer to settle", |cx| {
        text(cx, &composer).is_empty() && sent.get() == 2
    });

    let messages = forge.user_messages(&thread);
    assert!(
        matches!(&messages[1].kind, ItemKind::UserMessage { text, steer: true, .. } if text == "also this"),
        "the second message is a steer of the live run: {messages:?}"
    );
    let holding = cx.read(|app| composer.read(app).outgoing.is_some());
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
                    .all(|a| a.machine.state().hash().is_some())
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
    assert_eq!(text, "what is this?");
    assert_eq!(images.len(), 1, "the message references the pasted image");
    let stored = std::fs::read(forge.store.blob_path(&images[0]).unwrap()).unwrap();
    assert!(
        &stored[..4] == b"RIFF" && &stored[8..12] == b"WEBP",
        "Forge got WebP, not PNG"
    );
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
    // Forge keeps it as the default Settings → Engines shows.
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

#[gpui::test]
fn a_starred_model_lists_first_and_stays_starred(cx: &mut TestAppContext) {
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
        "the chosen model lists first while nothing is starred"
    );

    let star = vcx.debug_bounds("composer-model-favorite-opus").unwrap();
    vcx.simulate_click(star.center(), gpui::Modifiers::none());
    assert!(
        top(vcx, "composer-model-row-opus") < top(vcx, "composer-model-row-sonnet"),
        "a starred model lists before the chosen one"
    );
    assert!(
        composer.read_with(vcx, |composer, _| composer.menu.is_some()),
        "starring keeps the menu open"
    );

    // The Editor restarts: the star is in its settings.json.
    let files = forge.core.read_with(vcx, |core, _| core.files.clone());
    let core = vcx.update(|_, app| app.new(|cx| Core::new(files, cx).unwrap()));
    let favorites = core.read_with(vcx, |core, _| core.settings.get("model_favorites").cloned());
    assert_eq!(favorites, Some(serde_json::json!({ "claude": ["opus"] })));
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
    let noted = asked.clone();
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

    // "New project" closes the menu and asks the shell for one.
    click(vcx, "composer-project");
    click(vcx, "composer-project-new");
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
        matches!(&messages[0].kind, ItemKind::UserMessage { text, .. } if text == "$review this diff"),
        "{messages:?}"
    );
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
    click(vcx, "composer-axis-Speed");
    click(vcx, "composer-axis-Speed-1");
    assert!(
        vcx.debug_bounds("composer-model-speed").is_some(),
        "the model button names the chosen tier"
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
