//! Against a real in-process Forge: pairing by pasting, saving the global
//! prompt, and the way from Extensions to the Marketplace's skills.

use crate::prompt::GLOBAL_PROMPT_KEY;
use crate::{BuildInfo, Settings};
use editor_core::{Core, EditorFiles, HostData, Screen, SettingsPage};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use std::time::Duration;

/// Runs GPUI until `check` holds; network work happens on real threads.
fn wait_for<T>(
    cx: &mut VisualTestContext,
    what: &str,
    check: impl Fn(&gpui::App) -> Option<T>,
) -> T {
    for _ in 0..400 {
        cx.run_until_parked();
        if let Some(value) = cx.read(|cx| check(cx)) {
            return value;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

#[gpui::test]
fn pasted_pairing_string_adds_a_live_host_and_the_prompt_saves_to_forge(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = tempfile::tempdir().unwrap();
    let forge = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let server = forge
        .block_on(forge_http::start(forge_http::ServeConfig {
            data_dir: dir.path().join("forge"),
            listen: "127.0.0.1:0".parse().unwrap(),
        }))
        .unwrap();
    let address = server.local_addr().unwrap();
    let store = server.store().clone();
    forge.spawn(async move {
        let _ = server.run().await;
    });
    let invite = forge.block_on(forge_http::invite(&store)).unwrap();
    let fingerprint = artisan_api::tls::fingerprint_of(&invite.forge_certificate).unwrap();

    let files = EditorFiles {
        dir: dir.path().join("editor"),
    };
    let core = cx.new(|cx| Core::new(files, cx).unwrap());
    let (settings, cx) = cx.add_window_view(|window, cx| {
        Settings::new(core.clone(), BuildInfo::default(), window, cx)
    });
    core.update(cx, |core, cx| {
        core.navigate(Screen::Settings(SettingsPage::Hosts), cx);
    });

    // What `ae forge pair` prints, pasted into the Hosts page.
    let pasted = format!("{address} {} {fingerprint}", invite.code);
    settings.update(cx, |settings, cx| {
        settings
            .hosts
            .pairing
            .update(cx, |editor, cx| editor.set_text(&pasted, cx));
        settings.pair_pasted(cx);
    });
    let host: Entity<HostData> = wait_for(cx, "the pasted host to become live", |cx| {
        let host = core.read(cx).hosts().next().map(|(_, host)| host.clone())?;
        host.read(cx).is_live().then_some(host)
    });
    settings.read_with(cx, |settings, _| assert_eq!(settings.hosts_error(), None));

    core.update(cx, |core, cx| {
        core.navigate(Screen::Settings(SettingsPage::GlobalPrompt), cx);
    });
    wait_for(cx, "the global prompt to be read", |cx| {
        settings
            .read(cx)
            .prompt_status(cx)
            .starts_with("Not saved")
            .then_some(())
    });
    let prompt = "Answer in British English.";
    settings.update(cx, |settings, cx| {
        settings
            .prompt
            .editor
            .update(cx, |editor, cx| editor.set_text(prompt, cx));
        settings.save_prompt(cx);
    });
    wait_for(cx, "Forge to send the saved prompt back", |cx| {
        (host
            .read(cx)
            .setting(GLOBAL_PROMPT_KEY)
            .and_then(|value| value.as_str())
            == Some(prompt))
        .then_some(())
    });
    let stored = forge.block_on(store.setting(GLOBAL_PROMPT_KEY)).unwrap();
    assert_eq!(stored, Some(serde_json::Value::String(prompt.to_owned())));
    let status = wait_for(cx, "the save to be confirmed", |cx| {
        let status = settings.read(cx).prompt_status(cx);
        status.starts_with("Saved").then_some(status)
    });
    assert_eq!(
        status, "Saved \u{b7} version 1",
        "the page names the saved version"
    );

    // Skills are the Marketplace's; the Extensions page leads there.
    core.update(cx, |core, cx| {
        core.navigate(Screen::Settings(SettingsPage::Extensions), cx);
    });
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    let manage = cx
        .debug_bounds(crate::extensions::MANAGE_SKILLS_SELECTOR)
        .expect("the Skills section offers the Marketplace");
    cx.simulate_click(manage.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        cx.read(|cx| core.read(cx).screen().clone()),
        Screen::Marketplace,
        "the button opens the Marketplace"
    );
}
