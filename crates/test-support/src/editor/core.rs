use artisan_api::ThreadId;
use editor_conversation::controller::Conversation;
use editor_conversation::presentation::PresentationScope;
use editor_core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use std::path::Path;
use tempfile::TempDir;

pub fn test_core(directory: &Path) -> Core {
    Core::for_tests(EditorFiles {
        dir: directory.to_owned(),
    })
    .expect("set up a component-test Core")
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

pub fn client(core: &Core, config: &HostConfig) -> ForgeClient {
    let identity = core
        .identity
        .as_ref()
        .expect("invariant: a component-test Core has an identity");
    ForgeClient::new(&config.address, &config.fingerprint, identity)
        .unwrap_or_else(|error| panic!("build a client for host {}: {error:#}", config.id))
}

pub fn connect(core: &mut Core, config: &HostConfig) {
    let client = client(core, config);
    core.add_test_host(config, client);
}

pub struct OpenThread {
    pub conversation: Conversation,
    pub core: Core,
    pub host: HostConfig,
    pub thread: ThreadId,
    pub directory: TempDir,
}

impl OpenThread {
    pub fn new(host: &str, thread: &str) -> Self {
        let directory = tempfile::tempdir().expect("create a test data folder");
        let mut core = test_core(directory.path());
        let host = remote_host(host);
        connect(&mut core, &host);
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
