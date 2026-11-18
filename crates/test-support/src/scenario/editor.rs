use super::access::{BridgeSink, Published};
use super::render::presenter;
use super::view::{Found, View};
use crate::items::{assistant_reply, user_message};
use crate::replay::ReplayingEngine;
#[cfg(windows)]
use aqua::AccessSink;
use aqua::prelude::{AquaWindowId, ObservedWindowState, WindowEvent, WindowOptions};
use aqua::{Aqua, AquaConfig, AquaUserEvent};
use aqua_a11y_uia::A11yBridge;
use aqua_a11y_uia::accesskit::{Action, ActionData, ActionRequest, Role, TreeId};
use aqua_core::{
    InputEvent, Key, KeyEvent, Modifiers, Point, PointerButton, PointerEvent, PointerKind,
};
use aqua_protocol::LogicalSize;
use artisan_api::{
    CreateProject, CreateThread, DiscoveryState, EngineConfig, EngineId, InstallState, ItemId,
    SignInState, Status, ThreadId,
};
use editor_aqua::core::{Core, EditorFiles, HostData, HostId, Screen, SettingsPage};
use editor_aqua::qualification::fixture::{Fixture, FixtureItem, ForgeServer};
use editor_aqua::qualification::sign_in::{ForgeInvite, MACHINE_ID, join_forges};
use editor_aqua::shell::application::ShellApplication;
use editor_aqua::shell::host::ShellResources;
use editor_aqua::shell::root::RootViews;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const SETTLE: Duration = Duration::from_secs(20);
const QUIET: Duration = Duration::from_millis(300);
const WIDTH: f64 = 1000.0;
const HEIGHT: f64 = 700.0;
const PROJECT: &str = "project";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opening {
    Thread(&'static str),
    NewThread,
    Settings(SettingsPage),
}

#[derive(Debug)]
struct Seed {
    name: String,
    threads: Vec<CreateThread>,
    items: Vec<FixtureItem>,
}

#[derive(Debug)]
pub struct Scenario {
    engine: EngineId,
    case: &'static str,
    forges: Vec<Seed>,
}

pub struct Editor {
    aqua: Aqua,
    app: ShellApplication<RootViews>,
    id: AquaWindowId,
    bridge: Arc<A11yBridge>,
    published: Rc<RefCell<Published>>,
    engine: ReplayingEngine,
    forges: Vec<ForgeServer>,
    directory: TempDir,
}

impl Seed {
    fn named(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            threads: Vec::new(),
            items: Vec::new(),
        }
    }
}

impl Scenario {
    #[must_use]
    pub fn replaying(engine: EngineId, case: &'static str) -> Self {
        Self {
            engine,
            case,
            forges: vec![Seed::named("Forge")],
        }
    }

    #[must_use]
    pub fn forge(mut self, name: &str) -> Self {
        self.forges.push(Seed::named(name));
        self
    }

    fn seed(&mut self) -> &mut Seed {
        self.forges
            .last_mut()
            .expect("invariant: a scenario starts with one Forge")
    }

    #[must_use]
    pub fn thread(mut self, id: &str, title: &str) -> Self {
        let config = EngineConfig::new(self.engine);
        self.seed().threads.push(CreateThread {
            id: ThreadId::from(id),
            project_id: PROJECT.into(),
            config,
            title: Some(title.to_owned()),
            parent_thread_id: None,
            spawned_by: None,
        });
        self
    }

    #[must_use]
    pub fn exchange(mut self, thread: &str, asked: &str, answered: &str) -> Self {
        let seed = self.seed();
        for kind in [user_message(asked), assistant_reply(answered)] {
            seed.items.push(FixtureItem {
                id: ItemId::from(format!("item-{}", seed.items.len() + 1)),
                thread_id: ThreadId::from(thread),
                run_id: None,
                parent_id: None,
                native_id: None,
                status: Status::Done,
                kind,
            });
        }
        self
    }

    #[must_use]
    pub fn open(self, opening: Opening) -> Editor {
        Editor::start(self, opening)
    }
}

fn side_queries_off() -> BTreeMap<String, serde_json::Value> {
    [
        forge_runs::SUGGESTIONS_KEY,
        forge_runs::YOU_SHOULD_KNOW_KEY,
        forge_runs::SUMMARY_TITLES_KEY,
    ]
    .into_iter()
    .map(|key| (key.to_owned(), serde_json::json!(false)))
    .collect()
}

fn duration_length(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut end = 0;
    let mut at = 0;
    loop {
        let digits = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        let unit = at;
        while bytes
            .get(at)
            .is_some_and(|byte| matches!(byte, b'h' | b'm' | b's'))
        {
            at += 1;
        }
        if at == digits || unit == digits || at == unit {
            return end;
        }
        end = at;
        if bytes.get(at) != Some(&b' ') {
            return end;
        }
        at += 1;
    }
}

fn steady(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((at, lead)) = [" for ", " after "]
        .into_iter()
        .filter_map(|lead| rest.find(lead).map(|at| (at, lead)))
        .min()
    {
        let (head, tail) = rest.split_at(at + lead.len());
        out.push_str(head);
        let length = duration_length(tail);
        if length > 0 {
            out.push_str("[duration]");
        }
        rest = tail
            .get(length..)
            .expect("invariant: a duration ends on an ASCII boundary");
    }
    out.push_str(rest);
    out
}

fn engines_checked(host: &HostData) -> bool {
    let installed: Vec<_> = host
        .engines
        .values()
        .filter(|status| matches!(status.install, InstallState::Installed { .. }))
        .collect();
    !installed.is_empty()
        && installed.iter().all(|status| {
            !matches!(
                status.discovery,
                DiscoveryState::Unknown | DiscoveryState::Refreshing { .. }
            ) && status.sign_in != SignInState::Unknown
        })
}

fn pointer(at: Point, buttons: u8) -> PointerEvent {
    PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: at,
        buttons,
    }
}

struct Served {
    forges: Vec<ForgeServer>,
    invites: Vec<ForgeInvite>,
    engine: ReplayingEngine,
}

fn serve(scenario: Scenario, root: &Path, work: &Path) -> Served {
    let runtime = editor_aqua::core::runtime::runtime();
    let mut engines = Vec::new();
    let mut forges = Vec::new();
    let mut invites = Vec::new();
    for (index, seed) in scenario.forges.into_iter().enumerate() {
        let forge_data = root.join(format!("forge-{index}"));
        engines.push(ReplayingEngine::install(
            &forge_data,
            "replay_engine",
            scenario.engine,
            scenario.case,
        ));
        let loopback = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0));
        let forge = runtime
            .block_on(ForgeServer::start(forge_data, loopback))
            .unwrap_or_else(|error| panic!("start the Forge {}: {error}", seed.name));
        let fixture = Fixture {
            version: 1,
            projects: vec![CreateProject {
                id: PROJECT.into(),
                path: work.display().to_string(),
                name: Some("Project".to_owned()),
            }],
            threads: seed.threads,
            runs: Vec::new(),
            items: seed.items,
            settings: side_queries_off(),
        };
        runtime
            .block_on(fixture.seed(&forge.store))
            .unwrap_or_else(|error| panic!("seed the Forge {}: {error}", seed.name));
        let pairing = runtime
            .block_on(forge.pairing_input())
            .unwrap_or_else(|error| panic!("invite the Editor to {}: {error}", seed.name));
        let host = match index {
            0 => HostId(MACHINE_ID.to_owned()),
            _ => HostId(format!("mch_scenario{index}")),
        };
        invites.push(ForgeInvite {
            host,
            name: seed.name,
            pairing,
        });
        forges.push(forge);
    }
    Served {
        forges,
        invites,
        engine: engines
            .into_iter()
            .next()
            .expect("invariant: a scenario starts with one Forge"),
    }
}

fn screen(opening: Opening, host: &HostId) -> Screen {
    match opening {
        Opening::Thread(thread) => Screen::Thread {
            host: host.clone(),
            thread: ThreadId::from(thread),
        },
        Opening::NewThread => Screen::NewThread {
            host: host.clone(),
            project: Some(PROJECT.into()),
        },
        Opening::Settings(page) => Screen::Settings(page),
    }
}

struct Window {
    aqua: Aqua,
    app: ShellApplication<RootViews>,
    id: AquaWindowId,
    size: LogicalSize,
}

fn window(core: Core) -> Window {
    let mut config = AquaConfig {
        headless: true,
        ..AquaConfig::default()
    };
    "Artisan Editor".clone_into(&mut config.window.title);
    let mut aqua = Aqua::new(config).expect("start a headless Aqua runtime");
    let mut resources =
        ShellResources::for_runtime(&aqua, 1.0).expect("lay out the Editor at scale 1");
    resources
        .ui
        .host_mut()
        .motion_mut()
        .set_reduced_motion(aqua_ui::motion::ReducedMotion::Reduce, 0.0);
    let app = ShellApplication::for_qualification(core, resources, RootViews::default());
    let id = AquaWindowId::new(1).expect("invariant: window id 1 is non-zero");
    let mut size = WindowOptions::default().inner_size;
    size.width = WIDTH;
    size.height = HEIGHT;
    let mut observed = ObservedWindowState::new(id, "Artisan Editor".to_owned(), size);
    observed.focused = true;
    aqua.add_headless_window(observed);
    assert!(
        aqua.attach_render(id, presenter(), Duration::from_millis(16)),
        "the scenario window takes a render link"
    );
    Window {
        aqua,
        app,
        id,
        size,
    }
}

impl Editor {
    fn start(scenario: Scenario, opening: Opening) -> Self {
        let directory = tempfile::tempdir().expect("create the scenario folder");
        let work = directory.path().join("work");
        let editor_dir = directory.path().join("editor");
        for folder in [&work, &editor_dir] {
            std::fs::create_dir_all(folder)
                .unwrap_or_else(|error| panic!("create {}: {error}", folder.display()));
        }
        let served = serve(scenario, directory.path(), &work);
        let files = EditorFiles { dir: editor_dir };
        join_forges(&files, &served.invites).expect("sign the Editor in to its Forges");
        let host = &served
            .invites
            .first()
            .expect("invariant: a scenario starts with one Forge")
            .host;
        let mut core = Core::for_tests(files).expect("open the Editor's core");
        core.navigate(screen(opening, host));
        let Window {
            mut aqua,
            mut app,
            id,
            size,
        } = window(core);
        let inbox = aqua.inbox();
        let bridge = A11yBridge::new(Arc::new(move || {
            inbox.post(AquaUserEvent::A11yActions(id));
        }));
        let published = Rc::new(RefCell::new(Published::default()));
        aqua.attach_access(
            id,
            Box::new(BridgeSink::new(Arc::clone(&bridge), Rc::clone(&published))),
        );
        aqua.dispatch_start(&mut app, id);
        aqua.dispatch_window_event(&mut app, id, &WindowEvent::Resized(id, size));
        aqua.dispatch_input(&mut app, id, &InputEvent::Focused(true));
        bridge.connect();
        let hosts = served.invites.len();
        let mut editor = Self {
            aqua,
            app,
            id,
            bridge,
            published,
            engine: served.engine,
            forges: served.forges,
            directory,
        };
        editor.wait_for_engine_checks(hosts);
        editor
    }

    #[track_caller]
    fn wait_for_engine_checks(&mut self, hosts: usize) {
        let deadline = Instant::now() + SETTLE;
        let mut pause = Duration::from_micros(200);
        loop {
            self.pump();
            let core = &self.app.shell.core;
            if core.hosts().count() == hosts && core.hosts().all(|(_, host)| engines_checked(host))
            {
                return;
            }
            if Instant::now() >= deadline {
                self.fail("every Forge to finish checking its engines");
            }
            std::thread::park_timeout(pause);
            pause = (pause * 2).min(Duration::from_millis(10));
        }
    }

    #[must_use]
    pub const fn engine(&self) -> &ReplayingEngine {
        &self.engine
    }

    pub fn stop_forge(&mut self) {
        let forge = self.forges.remove(0);
        editor_aqua::core::runtime::runtime()
            .block_on(forge.shutdown())
            .expect("stop the scenario's Forge");
    }

    fn pump(&mut self) {
        self.aqua.pump_inbox(&mut self.app);
        let now = self.aqua.clock();
        self.aqua.draw_ready(&mut self.app, now);
    }

    fn render(&self) -> Option<String> {
        let published = self.published.borrow();
        let tree = published.tree()?;
        Some(self.steady(&View::new(tree.state()).render()))
    }

    fn steady(&self, text: &str) -> String {
        let folder = self.directory.path().display().to_string();
        steady(&text.replace(&folder, "[scenario]"))
    }

    fn inspect<T>(&self, look: impl FnOnce(&View<'_>) -> T) -> Option<T> {
        let published = self.published.borrow();
        published.tree().map(|tree| look(&View::new(tree.state())))
    }

    #[track_caller]
    pub fn settle(&mut self, what: &str, done: impl Fn(&View<'_>) -> bool) -> String {
        self.settle_on(what, done, |view| Some(view.render()))
    }

    #[track_caller]
    pub fn settle_part(
        &mut self,
        what: &str,
        role: Role,
        label: Option<&str>,
        done: impl Fn(&View<'_>) -> bool,
    ) -> String {
        self.settle_on(what, done, |view| view.part(role, label))
    }

    #[track_caller]
    pub fn settle_on(
        &mut self,
        what: &str,
        done: impl Fn(&View<'_>) -> bool,
        show: impl Fn(&View<'_>) -> Option<String>,
    ) -> String {
        let deadline = Instant::now() + SETTLE;
        let mut previous: Option<String> = None;
        let mut quiet_since = Instant::now();
        let mut pause = Duration::from_micros(200);
        loop {
            self.pump();
            let current = self.render();
            let ready = self.inspect(&done).unwrap_or(false);
            if !ready || current != previous {
                quiet_since = Instant::now();
            }
            if quiet_since.elapsed() >= QUIET
                && let Some(shown) = self.inspect(&show).flatten()
            {
                return self.steady(&shown);
            }
            previous = current;
            if Instant::now() >= deadline {
                self.fail(what);
            }
            std::thread::park_timeout(pause);
            pause = (pause * 2).min(Duration::from_millis(10));
        }
    }

    #[track_caller]
    fn fail(&self, what: &str) -> ! {
        let shown = self
            .render()
            .unwrap_or_else(|| "nothing was published".to_owned());
        panic!(
            "timed out after {SETTLE:?} waiting for {what}; the reader sees:\n{shown}\nthe replaying engine saw:\n{}",
            self.engine.log()
        );
    }

    #[track_caller]
    pub fn shows(&mut self, text: &str) -> String {
        self.settle(&format!("{text:?} to show"), |view| view.shows(text))
    }

    #[track_caller]
    pub fn node(&mut self, role: Role, label: &str) -> Found {
        self.settle(&format!("a {role:?} named {label:?}"), |view| {
            view.find(role, label).is_some()
        });
        self.inspect(|view| view.find(role, label))
            .flatten()
            .unwrap_or_else(|| panic!("the settled view lost the {role:?} named {label:?}"))
    }

    #[must_use]
    pub fn view<T>(&self, look: impl FnOnce(&View<'_>) -> T) -> T {
        self.inspect(look)
            .expect("the scenario has published its accessibility tree")
    }

    fn dispatch(&mut self, events: &[InputEvent]) {
        for event in events {
            self.aqua.dispatch_input(&mut self.app, self.id, event);
        }
        self.pump();
    }

    #[track_caller]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "window coordinates fit in f32, which is what pointer input carries"
    )]
    pub fn click(&mut self, role: Role, label: &str) {
        let target = self.node(role, label);
        let bounds = target
            .bounds
            .unwrap_or_else(|| panic!("the {role:?} named {label:?} has no bounds to click"));
        let at = Point::new(
            f64::midpoint(bounds.x0, bounds.x1) as f32,
            f64::midpoint(bounds.y0, bounds.y1) as f32,
        );
        self.dispatch(&[
            InputEvent::PointerMove(pointer(at, 0)),
            InputEvent::PointerDown(pointer(at, 1), PointerButton::Primary),
            InputEvent::PointerUp(pointer(at, 0), PointerButton::Primary),
        ]);
    }

    #[track_caller]
    pub fn activate(&mut self, role: Role, label: &str) {
        let target = self.node(role, label);
        self.bridge.native_action(&ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: target.node,
            data: None,
        });
        self.pump();
    }

    #[track_caller]
    pub fn set_value(&mut self, role: Role, label: &str, value: &str) {
        let target = self.node(role, label);
        self.bridge.native_action(&ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: target.node,
            data: Some(ActionData::Value(value.into())),
        });
        self.pump();
    }

    pub fn press(&mut self, key: Key, modifiers: Modifiers) {
        let event = KeyEvent {
            key,
            modifiers,
            repeat: false,
        };
        self.dispatch(&[InputEvent::KeyDown(event), InputEvent::KeyUp(event)]);
    }

    pub fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            let event = KeyEvent {
                key: Key::Char(character),
                modifiers: Modifiers::NONE,
                repeat: false,
            };
            self.dispatch(&[
                InputEvent::KeyDown(event),
                InputEvent::TextInput {
                    text: character.to_string(),
                },
                InputEvent::KeyUp(event),
            ]);
        }
    }

    #[cfg(windows)]
    pub(super) fn attach_access(&mut self, sink: Box<dyn AccessSink>) {
        self.aqua.attach_access(self.id, sink);
        self.pump();
    }

    #[cfg(windows)]
    pub(super) fn access_waker(&self) -> aqua_a11y_uia::WakeFn {
        let inbox = self.aqua.inbox();
        let id = self.id;
        Arc::new(move || {
            inbox.post(AquaUserEvent::A11yActions(id));
        })
    }

    #[cfg(windows)]
    pub(super) fn frame(&mut self) {
        self.pump();
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        if std::thread::panicking() {
            writeln!(
                std::io::stderr(),
                "what the replaying engine saw:\n{}",
                self.engine.log()
            )
            .expect("the test writes to stderr");
        }
    }
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("engine", &self.engine)
            .finish_non_exhaustive()
    }
}
