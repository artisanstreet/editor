//! Against a real in-process Forge: pairing by pasting, saving the global
//! prompt, and the way from Extensions to the Marketplace's skills.

use crate::prompt::GLOBAL_PROMPT_KEY;
use crate::{BuildInfo, Settings};
use editor_core::{Core, HostData, Screen, SettingsPage};
use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use std::time::Duration;

/// Runs GPUI until `check` holds; network work happens on real threads.
pub fn wait_for<T>(
    cx: &VisualTestContext,
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
fn added_host_goes_live_and_the_prompt_saves_to_forge(cx: &mut TestAppContext) {
    let fixture = SettingsFixture::new(cx);
    let core = fixture.core.clone();
    let (settings, cx) = fixture.open(cx, SettingsPage::Hosts);
    let host: Entity<HostData> = cx.read(|cx| {
        core.read(cx)
            .host(&fixture.hosts[0])
            .cloned()
            .expect("the paired test host is present")
    });

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
    let stored = editor_core::runtime::runtime()
        .block_on(fixture.stores[0].setting(GLOBAL_PROMPT_KEY))
        .unwrap();
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

/// Holds a completed real request before its view callback. Each fixture owns
/// its gate; unrelated Settings instances and network work are unaffected.
#[derive(Clone)]
pub struct CompletionGate(std::sync::Arc<GateState>);
struct GateState {
    entered: std::sync::atomic::AtomicBool,
    released: tokio::sync::Semaphore,
    finished: std::sync::atomic::AtomicBool,
}
impl CompletionGate {
    pub(super) fn new() -> Self {
        Self(std::sync::Arc::new(GateState {
            entered: std::sync::atomic::AtomicBool::new(false),
            released: tokio::sync::Semaphore::new(0),
            finished: std::sync::atomic::AtomicBool::new(false),
        }))
    }
    pub(super) async fn wait(&self) {
        use std::sync::atomic::Ordering;
        self.0.entered.store(true, Ordering::Release);
        self.0.released.acquire().await.unwrap().forget();
        self.0.finished.store(true, Ordering::Release);
    }
    pub(super) fn entered(&self) -> bool {
        self.0.entered.load(std::sync::atomic::Ordering::Acquire)
    }
    pub(super) fn release(&self) {
        self.0.released.add_permits(1);
    }
    pub(super) fn finished(&self) -> bool {
        self.0.finished.load(std::sync::atomic::Ordering::Acquire)
    }
}

#[path = "../../editor-ui/test_support/forge.rs"]
mod forge_fixture;

pub struct SettingsFixture {
    pub(super) core: Entity<Core>,
    pub(super) hosts: [editor_core::HostId; 2],
    pub(super) stores: [forge_store::Store; 2],
    _runtime: tokio::runtime::Runtime,
    _directory: tempfile::TempDir,
}
impl SettingsFixture {
    pub(super) fn new(cx: &mut TestAppContext) -> Self {
        cx.executor().allow_parking();
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let mut configs = Vec::new();
        let mut stores = Vec::new();
        let mut files = None;
        for index in 0..2 {
            let server = runtime
                .block_on(forge_http::start(forge_http::ServeConfig {
                    data_dir: directory.path().join(format!("forge-{index}")),
                    listen: "127.0.0.1:0".parse().unwrap(),
                }))
                .unwrap();
            let address = server.local_addr().unwrap().to_string();
            let store = server.store().clone();
            runtime.spawn(async move {
                server.run().await.ok();
            });
            let (paired_files, config) =
                forge_fixture::pair(&store, &runtime, &address, &directory.path().join("editor"));
            files = Some(paired_files);
            stores.push(store);
            configs.push(config);
        }
        let files = files.unwrap();
        let hosts = [configs[0].id.clone(), configs[1].id.clone()];
        files
            .save_hosts(&editor_core::hosts::HostsFile {
                hosts: configs.clone(),
                last: Some(hosts[0].clone()),
            })
            .unwrap();
        // `for_tests` mocks the gate Ready and connects the saved paired
        // hosts through the private inventory path.
        let core = cx.new(|cx| Core::for_tests(files, cx).unwrap());
        Self {
            core,
            hosts,
            stores: [stores.remove(0), stores.remove(0)],
            _runtime: runtime,
            _directory: directory,
        }
    }
    pub(super) fn open<'a>(
        &self,
        cx: &'a mut TestAppContext,
        page: SettingsPage,
    ) -> (Entity<Settings>, &'a mut VisualTestContext) {
        let core = self.core.clone();
        core.update(cx, |core, cx| core.navigate(Screen::Settings(page), cx));
        cx.update(editor_ui::text_editor::TextEditor::bind_actions);
        let (settings, cx) = cx.add_window_view(|window, cx| {
            Settings::new(core.clone(), BuildInfo::default(), window, cx)
        });
        wait_for(cx, "both isolated hosts", |cx| {
            self.hosts
                .iter()
                .all(|id| {
                    core.read(cx)
                        .host(id)
                        .is_some_and(|host| host.read(cx).is_live())
                })
                .then_some(())
        });
        (settings, cx)
    }
    pub(super) fn select(&self, index: usize, page: SettingsPage, cx: &mut VisualTestContext) {
        self.core.update(cx, |core, cx| {
            core.navigate(
                Screen::NewThread {
                    host: self.hosts[index].clone(),
                    project: None,
                },
                cx,
            );
            core.navigate(Screen::Settings(page), cx);
        });
        cx.run_until_parked();
    }
}
pub fn wait_gate(cx: &VisualTestContext, gate: &CompletionGate) {
    wait_for(cx, "completed request at callback gate", |_| {
        gate.entered().then_some(())
    });
}
pub fn finish_gate(cx: &VisualTestContext, gate: &CompletionGate) {
    gate.release();
    wait_for(cx, "released callback", |_| gate.finished().then_some(()));
    cx.run_until_parked();
}

#[gpui::test]
fn subsections_show_related_controls_in_their_own_category(cx: &mut TestAppContext) {
    use editor_core::SettingsSection as S;
    let fixture = SettingsFixture::new(cx);
    let (_, cx) = fixture.open(cx, SettingsPage::Section(S::Appearance));
    cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(900.0)));
    assert!(cx.debug_bounds("settings-font-set-trigger").is_some());
    assert!(cx.debug_bounds("settings-switch-notify").is_none());
    assert!(cx.debug_bounds("settings-switch-enhanced-claude").is_none());
    for (section, visible, absent) in [
        (
            S::Notifications,
            "settings-switch-notify",
            "settings-font-set-trigger",
        ),
        (
            S::Streaming,
            "settings-streaming-mode-trigger",
            "settings-switch-retry-overloaded",
        ),
        (
            S::Automation,
            "settings-switch-retry-overloaded",
            "settings-switch-enhanced-claude",
        ),
        (
            S::Assistance,
            "settings-switch-suggestions",
            "settings-switch-retry-overloaded",
        ),
        (
            S::ClaudeAi,
            "settings-switch-enhanced-claude",
            "settings-section-account",
        ),
    ] {
        fixture.core.update(cx, |core, cx| {
            core.navigate(Screen::Settings(SettingsPage::Section(section)), cx);
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(visible).is_some(),
            "{section:?} shows {visible}"
        );
        assert!(
            cx.debug_bounds(absent).is_none(),
            "{section:?} excludes {absent}"
        );
    }
}
