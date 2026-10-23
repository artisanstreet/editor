//! The Marketplace against a real in-process Forge: the skills on the host
//! and its global prompt, Edit opening the prompt's settings page, and a
//! skill turned off staying off in Forge.

use crate::Marketplace;
use editor_core::hosts::{self, HostKind, HostsFile};
use editor_core::runtime::runtime;
use editor_core::{Core, EditorFiles, Screen, SettingsPage};
use gpui::{AppContext as _, TestAppContext, VisualTestContext, size};
use std::time::{Duration, Instant};

fn wait(
    cx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        // The search waits for typing to pause; the test clock only moves here.
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A skills.sh that knows one skill, `pdf-tools`, and only for the query
/// `pd`; answers every request on its own thread.
fn stub_skills_sh() -> String {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 2048];
            let read = stream.read(&mut request).unwrap_or(0);
            let line = String::from_utf8_lossy(&request[..read]);
            let skills = if line.starts_with("GET /api/search?q=pd&") {
                r#"[{"id":"anthropics/skills/pdf-tools","source":"anthropics/skills","skillId":"pdf-tools","name":"pdf-tools","installs":41000}]"#
            } else {
                "[]"
            };
            let body = format!(r#"{{"skills":{skills}}}"#);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            ).ok();
        }
    });
    format!("http://{address}")
}

/// The user's own `pdf` skill in Forge's skills folder.
fn write_skill(folder: &std::path::Path) {
    std::fs::create_dir_all(folder).unwrap();
    std::fs::write(
        folder.join("SKILL.md"),
        "---\nname: pdf\ndescription: Reads and fills PDF forms.\n---\n\nDo it well.\n",
    )
    .unwrap();
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario from listing to opening the prompt"
)]
fn the_marketplace_lists_skills_turns_one_off_and_opens_the_prompt(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    cx.update(editor_ui::text_editor::TextEditor::bind_actions);
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("forge");
    write_skill(&data.join("extensions/skills/pdf"));
    let forge = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let server = forge
        .block_on(forge_http::start(forge_http::ServeConfig {
            data_dir: data,
            listen: "127.0.0.1:0".parse().unwrap(),
        }))
        .unwrap();
    let address = server.local_addr().unwrap().to_string();
    let store = server.store().clone();
    server.hub().set_skills_sh_url(stub_skills_sh());
    forge.spawn(server.run());
    forge
        .block_on(store.set_setting(
            "global_prompt".to_owned(),
            serde_json::json!("# Be brief\n\nAnswer in British English."),
        ))
        .unwrap();

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
            last: Some(host.id),
        })
        .unwrap();
    let core = cx.new(|cx| Core::new(files, cx).unwrap());
    core.update(cx, |core, cx| core.navigate(Screen::Marketplace, cx));
    let (marketplace, cx) = cx.add_window_view(|_, cx| Marketplace::new(core.clone(), cx));
    cx.simulate_resize(size(gpui::px(1000.0), gpui::px(1600.0)));

    wait(cx, "the host's skill", |cx| {
        cx.debug_bounds("marketplace-skill-local.pdf").is_some()
    });
    assert!(
        cx.debug_bounds("marketplace-skill-artisan.artisan-editor")
            .is_some(),
        "Artisan's own skill is listed"
    );

    // The search is focused when the page opens; typing narrows the host's
    // skills and finds skills.sh's below them.
    cx.simulate_input("pd");
    wait(cx, "the search to leave only the pdf skill", |cx| {
        cx.debug_bounds("marketplace-skill-artisan.artisan-editor")
            .is_none()
            && cx.debug_bounds("marketplace-skill-local.pdf").is_some()
    });
    wait(cx, "skills.sh's pdf-tools under the host's pdf", |cx| {
        cx.debug_bounds("marketplace-listing-skills.sh:anthropics/skills/pdf-tools-install")
            .is_some()
    });
    cx.simulate_input("zz");
    wait(cx, "the search to say nothing matches", |cx| {
        cx.debug_bounds("marketplace-empty").is_some()
            && cx.debug_bounds("marketplace-skill-local.pdf").is_none()
    });
    cx.simulate_keystrokes("escape");
    wait(cx, "Escape to bring every skill back", |cx| {
        cx.debug_bounds("marketplace-skill-artisan.artisan-editor")
            .is_some()
    });

    // Turning the skill off is stored in Forge and shown as off.
    let switch = cx
        .debug_bounds("marketplace-skill-local.pdf-switch")
        .expect("the skill's switch");
    cx.simulate_click(switch.center(), gpui::Modifiers::none());
    wait(cx, "Forge to answer with the skill off", |cx| {
        marketplace.read_with(cx, |marketplace, cx| {
            marketplace.core.read(cx).active_host().is_some_and(|host| {
                host.read(cx).skill_catalog.as_ref().is_some_and(|catalog| {
                    catalog
                        .iter()
                        .any(|skill| skill.id == "local.pdf" && !skill.enabled)
                })
            })
        })
    });
    assert_eq!(
        forge.block_on(store.setting("disabled_skills")).unwrap(),
        Some(serde_json::json!(["local.pdf"])),
        "Forge keeps the skill off for every engine"
    );

    wait(cx, "the global prompt's first line", |cx| {
        cx.read(|cx| {
            core.read(cx)
                .active_host()
                .is_some_and(|host| host.read(cx).setting("global_prompt").is_some())
        })
    });
    let edit = cx
        .debug_bounds("marketplace-global-prompt-open")
        .expect("the prompt's Edit button");
    cx.simulate_click(edit.center(), gpui::Modifiers::none());
    assert_eq!(
        cx.read(|cx| core.read(cx).screen().clone()),
        Screen::Settings(SettingsPage::GlobalPrompt),
        "Edit opens the global prompt's settings page"
    );
}
