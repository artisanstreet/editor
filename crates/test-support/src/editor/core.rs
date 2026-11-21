use super::account::{
    ScriptedAe, StubRelay, account_url, add_machine, machine, open_account, unenrolled_ae,
};
use super::forge::StubForge;
use editor_core::{
    Ae, Clock, Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, RelayRoute, Services,
};
use std::path::Path;
use std::time::Duration;

const CONNECT_WITHIN: Duration = crate::wait::LIVENESS;

pub fn test_core(directory: &Path) -> Core {
    test_core_with_clock(directory, Clock::system())
}

pub fn test_core_with_clock(directory: &Path, clock: Clock) -> Core {
    test_core_with(directory, clock, Ae::new(ScriptedAe(unenrolled_ae)))
}

fn test_core_with(directory: &Path, clock: Clock, ae: Ae) -> Core {
    let files = EditorFiles {
        dir: directory.to_owned(),
    };
    let saved = sign_in(&files);
    let mut core = Core::with_services(
        files,
        Services::new(account_url(), ae, RelayRoute::dial(StubRelay), clock),
    )
    .expect("open a signed-in component-test Core");
    let deadline = std::time::Instant::now() + CONNECT_WITHIN;
    loop {
        core.drain_with(|_, _| {});
        let connected = saved.iter().all(|id| core.host(id).is_some());
        let settled = matches!(
            core.account().phase(),
            editor_core::AccountPhase::Authenticated { verified: true, .. }
        ) && !core.account().reading_local_forge()
            && core.pending_work() == 0;
        if connected && settled {
            return core;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the component-test Core never settled: saved hosts connected {connected}, account settled {settled}"
        );
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

fn sign_in(files: &EditorFiles) -> Vec<HostId> {
    let saved = files.hosts().expect("read the test's saved hosts");
    let identity = files.identity().expect("open the test Editor's identity");
    let profile = open_account(Vec::new());
    for config in &saved.hosts {
        let client = ForgeClient::new(&config.address, &config.fingerprint, &identity)
            .unwrap_or_else(|error| {
                panic!("build a client for saved host {}: {error:#}", config.id)
            });
        add_machine(
            &profile.account_id,
            machine(&config.id.0, &config.name),
            client,
        );
    }
    let machines = saved
        .hosts
        .iter()
        .map(|config| machine(&config.id.0, &config.name))
        .collect::<Vec<_>>();
    let account = serde_json::json!({
        "base_url": account_url(),
        "session_token": profile.account_id,
        "profile": artisan_account::network::AccountProfile { machines, ..profile },
    });
    std::fs::create_dir_all(&files.dir).expect("create the test's Editor folder");
    std::fs::write(files.dir.join("account.json"), account.to_string())
        .expect("write the test's signed-in account");
    saved.hosts.into_iter().map(|config| config.id).collect()
}

pub fn remote_host(id: &str) -> HostConfig {
    HostConfig {
        id: HostId(id.into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    }
}

pub fn stub_host(id: &str, name: &str, forge: &StubForge) -> HostConfig {
    HostConfig {
        id: HostId(id.into()),
        name: name.into(),
        address: forge.address().to_owned(),
        fingerprint: forge.fingerprint().to_owned(),
        kind: HostKind::Remote,
    }
}

fn client(core: &Core, config: &HostConfig) -> ForgeClient {
    let identity = core
        .identity()
        .expect("invariant: a component-test Core has an identity");
    ForgeClient::new(&config.address, &config.fingerprint, identity)
        .unwrap_or_else(|error| panic!("build a client for host {}: {error:#}", config.id))
}

pub fn connect(core: &mut Core, config: &HostConfig) {
    let client = client(core, config);
    connect_client(core, config, client);
}

pub fn connect_client(core: &mut Core, config: &HostConfig, client: ForgeClient) {
    let account = core
        .account()
        .profile()
        .expect("invariant: a component-test Core is signed in")
        .account_id
        .clone();
    add_machine(&account, machine(&config.id.0, &config.name), client);
    core.account_mut().refresh();
    let deadline = std::time::Instant::now() + CONNECT_WITHIN;
    while core.host(&config.id).is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "host {} never connected through the account's machine list",
            config.id
        );
        core.drain_with(|_, _| {});
        std::thread::park_timeout(Duration::from_millis(1));
    }
}

pub struct StubbedHost {
    pub core: Core,
    pub forge: StubForge,
    pub host: HostId,
    pub directory: tempfile::TempDir,
}

impl StubbedHost {
    pub fn start(state: artisan_api::State, routes: axum::Router) -> Self {
        Self::start_with_clock(state, routes, Clock::system())
    }

    pub fn start_with_clock(state: artisan_api::State, routes: axum::Router, clock: Clock) -> Self {
        let directory = tempfile::tempdir().expect("create a test data folder");
        Self::start_in(directory, state, routes, clock)
    }

    pub fn start_in(
        directory: tempfile::TempDir,
        state: artisan_api::State,
        routes: axum::Router,
        clock: Clock,
    ) -> Self {
        let forge = editor_core::runtime::runtime().block_on(StubForge::serve(state, routes));
        let mut core = test_core_with_clock(directory.path(), clock);
        let config = stub_host("stub-forge", "Stub", &forge);
        connect(&mut core, &config);
        let mut stubbed = Self {
            core,
            forge,
            host: config.id,
            directory,
        };
        stubbed.settle("the stub Forge to go live", |core, host| {
            core.host(host).is_some_and(editor_core::HostData::is_live)
        });
        stubbed
    }

    pub fn host(&self) -> &editor_core::HostData {
        self.core
            .host(&self.host)
            .expect("invariant: the stubbed host stays connected")
    }

    pub fn host_mut(&mut self) -> &mut editor_core::HostData {
        self.core
            .host_mut(&self.host)
            .expect("invariant: the stubbed host stays connected")
    }

    #[track_caller]
    pub fn settle(&mut self, what: &str, mut done: impl FnMut(&Core, &HostId) -> bool) {
        let deadline = std::time::Instant::now() + CONNECT_WITHIN;
        loop {
            self.core.drain_with(|_, _| {});
            if done(&self.core, &self.host) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out after {CONNECT_WITHIN:?} waiting for {what}"
            );
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
}

impl StubbedHost {
    pub fn apply_published(&mut self) {
        let seq = self.forge.publish_barrier();
        self.settle("the Editor to apply the published events", |core, host| {
            core.host(host)
                .and_then(|data| data.setting(super::forge::BARRIER))
                .is_some_and(|value| value == &serde_json::json!(seq))
        });
    }

    #[track_caller]
    pub fn take(&mut self, method: &str, path: &str) -> super::forge::HeldRequest {
        let deadline = std::time::Instant::now() + CONNECT_WITHIN;
        loop {
            self.core.drain_with(|_, _| {});
            if let Some(request) = self.forge.try_take(method, path) {
                return request;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the Editor never asked the stub Forge for {method} {path}"
            );
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
}

impl std::fmt::Debug for StubbedHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StubbedHost")
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "editor")]
pub struct Delivered(std::sync::mpsc::Receiver<()>);

#[cfg(feature = "editor")]
impl Delivered {
    pub fn watch(mailbox: &aqua::mailbox::Mailbox<()>) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        mailbox.set_waker(std::sync::Arc::new(move || match sender.send(()) {
            Ok(()) | Err(_) => {}
        }));
        Self(receiver)
    }

    #[track_caller]
    pub fn wait(&self, what: &str) {
        assert!(
            self.0.recv_timeout(CONNECT_WITHIN).is_ok(),
            "{what} never reached its owner's mailbox within {CONNECT_WITHIN:?}"
        );
    }
}

#[cfg(feature = "editor")]
impl std::fmt::Debug for Delivered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Delivered").finish_non_exhaustive()
    }
}

#[cfg(feature = "editor")]
pub use thread::OpenThread;

#[cfg(feature = "editor")]
mod thread {
    use artisan_api::ThreadId;
    use editor_conversation::controller::Conversation;
    use editor_conversation::presentation::PresentationScope;
    use editor_core::{Clock, Core, HostConfig};
    use tempfile::TempDir;

    pub struct OpenThread {
        pub conversation: Conversation,
        pub core: Core,
        pub host: HostConfig,
        pub thread: ThreadId,
        pub directory: TempDir,
    }

    impl OpenThread {
        pub fn new(host: &str, thread: &str) -> Self {
            Self::with_clock(host, thread, Clock::system())
        }

        fn with_clock(host: &str, thread: &str, clock: Clock) -> Self {
            let directory = tempfile::tempdir().expect("create a test data folder");
            let mut core = super::test_core_with_clock(directory.path(), clock);
            let host = super::remote_host(host);
            super::connect(&mut core, &host);
            let thread = ThreadId::from(thread);
            let mut conversation = Conversation::new(&core);
            conversation.open_thread(&mut core, host.id.clone(), thread.clone());
            Self {
                conversation,
                core,
                host,
                thread,
                directory,
            }
        }

        pub fn scope(&self) -> PresentationScope {
            PresentationScope::capture(&self.conversation, &self.core).unwrap_or_else(|| {
                panic!(
                    "invariant: thread {} on host {} has a presentation scope",
                    self.thread, self.host.id
                )
            })
        }
    }

    impl std::fmt::Debug for OpenThread {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("OpenThread")
                .field("host", &self.host.id)
                .field("thread", &self.thread)
                .finish_non_exhaustive()
        }
    }
}
