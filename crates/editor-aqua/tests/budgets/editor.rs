use aqua::prelude::{AquaWindowId, ObservedWindowState, WindowOptions};
use aqua::{Application, Aqua, AquaConfig};
use aqua_a11y_uia::A11yBridge;
use aqua_core::{InputEvent, Point, ScrollDelta};
use aqua_protocol::AccessTreeUpdate;
use artisan_api::ThreadId;
use editor_aqua::core::diagnostics::rebuild_stats::take_tally;
use editor_aqua::core::{Core, EditorFiles, HostId, Screen, hosts, runtime::runtime};
use editor_aqua::qualification::fixture::{Fixture, ForgeServer};
use editor_aqua::qualification::observation::Observation;
use editor_aqua::shell::{application::ShellApplication, host::ShellResources, root::RootViews};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use test_support::http::StubServer;
use tokio::sync::broadcast::error::TryRecvError;

const ACCOUNT_ID: &str = "acct_budget00000000000000000";
const MACHINE_ID: &str = "mch_budget00000000000000000";
const QUIET_FRAMES: usize = 8;
const QUIET_FOR: Duration = Duration::from_millis(300);
const SETTLE_LIMIT: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, Default)]
pub(crate) struct Frame {
    reasons: BTreeMap<&'static str, u32>,
    rebuilds: u64,
    refreshes: u64,
    requested: u64,
    animating: bool,
    access_examined: u64,
    access_published: u64,
}

impl Frame {
    const fn quiet(&self) -> bool {
        self.rebuilds == 0
            && self.refreshes == 0
            && self.requested == 0
            && !self.animating
            && self.access_published == 0
    }
}

#[derive(Debug, Default)]
pub(crate) struct Span {
    frames: Vec<Frame>,
}

impl Span {
    pub(crate) fn rebuilds(&self) -> u64 {
        self.frames.iter().map(|frame| frame.rebuilds).sum()
    }

    pub(crate) fn refreshes(&self) -> u64 {
        self.frames.iter().map(|frame| frame.refreshes).sum()
    }

    pub(crate) fn requested(&self) -> u64 {
        self.frames.iter().map(|frame| frame.requested).sum()
    }

    pub(crate) fn animating(&self) -> u64 {
        self.frames.iter().filter(|frame| frame.animating).count() as u64
    }

    pub(crate) fn published(&self) -> u64 {
        self.frames.iter().map(|frame| frame.access_published).sum()
    }

    pub(crate) fn causes(&self) -> String {
        let mut reasons = BTreeMap::<&str, u32>::new();
        for (reason, count) in self.frames.iter().flat_map(|frame| &frame.reasons) {
            *reasons.entry(reason).or_default() += count;
        }
        format!("rebuild reasons {reasons:?}")
    }

    fn most(&self, of: impl Fn(&Frame) -> u64) -> u64 {
        self.frames.iter().map(of).max().unwrap_or(0)
    }

    pub(crate) fn most_refreshes(&self) -> u64 {
        self.most(|frame| frame.refreshes)
    }

    pub(crate) fn most_examined(&self) -> u64 {
        self.most(|frame| frame.access_examined)
    }

    pub(crate) fn most_published(&self) -> u64 {
        self.most(|frame| frame.access_published)
    }

    pub(crate) fn extend(&mut self, other: Self) {
        self.frames.extend(other.frames);
    }
}

pub(crate) struct Editor {
    forge: ForgeServer,
    _account: StubServer,
    aqua: Aqua,
    app: ShellApplication<RootViews>,
    id: AquaWindowId,
    observed: ObservedWindowState,
    host: HostId,
    thread: ThreadId,
    changes: tokio::sync::broadcast::Receiver<artisan_api::Envelope>,
    bridge: Arc<A11yBridge>,
    sequence: u64,
    _directory: tempfile::TempDir,
}

pub(crate) struct Seeded {
    forge: ForgeServer,
    account: StubServer,
    files: EditorFiles,
    host: HostId,
    directory: tempfile::TempDir,
}

pub(crate) fn seed(fixture: &Fixture) -> Seeded {
    let directory = tempfile::tempdir().expect("a scratch folder");
    let forge_dir = directory.path().join("forge");
    let editor_dir = directory.path().join("editor");
    std::fs::create_dir_all(&forge_dir).expect("the Forge folder");
    std::fs::create_dir_all(&editor_dir).expect("the Editor folder");
    let loopback = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0));
    let forge = runtime()
        .block_on(ForgeServer::start(forge_dir, loopback))
        .expect("Forge starts");
    runtime()
        .block_on(fixture.seed(&forge.store))
        .expect("the fixture seeds Forge");
    let pairing = runtime()
        .block_on(forge.pairing_input())
        .expect("Forge invites the Editor");
    let files = EditorFiles { dir: editor_dir };
    let identity = files.identity().expect("the Editor identity");
    let host = HostId(MACHINE_ID.to_owned());
    let mut config = runtime()
        .block_on(hosts::pair(
            &pairing.address,
            &pairing.code,
            &pairing.fingerprint,
            &identity,
            "budgets",
            "Forge".to_owned(),
            hosts::HostKind::Local,
        ))
        .expect("the Editor pairs with Forge");
    config.id = host.clone();
    files
        .save_hosts(&hosts::HostsFile {
            hosts: vec![config],
            last: Some(host.clone()),
        })
        .expect("the hosts file saves");
    let account = save_signed_in_account(&files);
    Seeded {
        forge,
        account,
        files,
        host,
        directory,
    }
}

fn save_signed_in_account(files: &EditorFiles) -> StubServer {
    use artisan_account::network::{AccountProfile, Jwk, MachineEntry};
    let profile = AccountProfile {
        account_id: ACCOUNT_ID.to_owned(),
        email: "budgets@example.invalid".to_owned(),
        display_name: "Budgets".to_owned(),
        username: None,
        avatar_url: None,
        totp_enrolled: false,
        machines: vec![MachineEntry {
            id: MACHINE_ID.to_owned(),
            label: "Budget Forge".to_owned(),
            public_key: Jwk {
                kty: "OKP".to_owned(),
                crv: "Ed25519".to_owned(),
                x: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            },
            created_at: "2026-01-01T00:00:00.000Z".to_owned(),
            revoked_at: None,
            last_seen_at: None,
        }],
        latest_config_version: None,
    };
    let me = serde_json::to_value(&profile).expect("the account profile encodes");
    let server = runtime().block_on(StubServer::serve(axum::Router::new().route(
        "/v1/me",
        axum::routing::get(move || std::future::ready(axum::Json(me.clone()))),
    )));
    let account = serde_json::json!({
        "base_url": server.url(),
        "session_token": "budgets-session",
        "profile": profile,
    });
    std::fs::write(
        files.dir.join("account.json"),
        serde_json::to_vec_pretty(&account).expect("the account encodes"),
    )
    .expect("the account saves");
    editor_aqua::core::save_onboarding_for_tests(files, server.url(), ACCOUNT_ID, MACHINE_ID)
        .expect("the onboarding saves");
    server
}

impl Editor {
    pub(crate) fn open(seeded: Seeded, thread: &ThreadId, width: f64, height: f64) -> Self {
        let Seeded {
            forge,
            account,
            files,
            host,
            directory,
        } = seeded;
        let changes = forge.store.subscribe();
        take_tally();
        let mut core = Core::for_tests(files).expect("the Editor core opens");
        core.navigate(Screen::Thread {
            host: host.clone(),
            thread: thread.clone(),
        });
        let mut aqua = Aqua::new(AquaConfig {
            headless: true,
            ..AquaConfig::default()
        })
        .expect("the headless window system starts");
        let resources = ShellResources::new(1.0).expect("the Editor's shared resources load");
        let mut app = ShellApplication::for_qualification(core, resources, RootViews::default());
        let id = AquaWindowId::new(1).expect("invariant: window id 1 is nonzero");
        let mut size = WindowOptions::default().inner_size;
        size.width = width;
        size.height = height;
        let observed = ObservedWindowState::new(id, "Artisan Editor".into(), size);
        aqua.add_headless_window(observed.clone());
        aqua.dispatch_start(&mut app, id);
        aqua.dispatch_input(&mut app, id, &InputEvent::Focused(true));
        let bridge = A11yBridge::new(Arc::new(|| {}));
        bridge.connect();
        Self {
            forge,
            _account: account,
            aqua,
            app,
            id,
            observed,
            host,
            thread: thread.clone(),
            changes,
            bridge,
            sequence: 0,
            _directory: directory,
        }
    }

    fn frame(&mut self) -> Frame {
        let requested = self.aqua.scheduler().stats().requested;
        self.aqua.dispatch_wake(&mut self.app, self.id);
        let now = self.aqua.clock();
        self.app.prepare_accessibility(&self.observed, now);
        let deadline = self.app.build_scene(&self.observed, now);
        self.aqua.flush_output(&mut self.app, self.id);
        let (root, focus, nodes) = self
            .app
            .a11y_nodes_for(self.id)
            .expect("the Editor window exposes an accessibility tree");
        let update = AccessTreeUpdate { root, focus, nodes };
        let published = self
            .bridge
            .snapshot(&update)
            .map_or(0, |changed| changed.nodes.len());
        self.sequence += 1;
        self.aqua
            .dispatch_frame_completed(&mut self.app, self.id, self.sequence);
        let tally = take_tally();
        let refreshes = tally.event("section_refresh") + tally.event("transcript_subtree");
        Frame {
            reasons: tally.reasons,
            rebuilds: u64::from(tally.rebuilds),
            refreshes: u64::from(refreshes),
            requested: self.aqua.scheduler().stats().requested - requested,
            animating: deadline.is_some(),
            access_examined: update.nodes.len() as u64,
            access_published: published as u64,
        }
    }

    pub(crate) fn store(&self) -> &forge_store::Store {
        &self.forge.store
    }

    pub(crate) fn observation(&self) -> Option<Observation> {
        self.app.qualification_observation()
    }

    pub(crate) fn settle(&mut self, what: &str, ready: impl Fn(&Observation) -> bool) -> Span {
        let limit = Instant::now() + SETTLE_LIMIT;
        let mut span = Span::default();
        let mut quiet = 0;
        let mut quiet_since = Instant::now();
        loop {
            let frame = self.frame();
            let still = frame.quiet();
            span.frames.push(frame);
            let observed = self.observation();
            let forge_moved = self.forge_moved();
            let caught_up =
                self.app.shell.core.host(&self.host).is_some_and(|host| {
                    host.is_live() && host.pending_read(&self.thread).is_none()
                }) && !self.app.shell.core.test_startup_pending();
            let done = caught_up && observed.as_ref().is_some_and(&ready);
            if done && still && !forge_moved {
                quiet += 1;
            } else {
                quiet = 0;
                quiet_since = Instant::now();
            }
            if quiet >= QUIET_FRAMES && quiet_since.elapsed() >= QUIET_FOR {
                span.frames.truncate(span.frames.len() - quiet);
                return span;
            }
            assert!(
                Instant::now() < limit,
                "the Editor never settled after {what}: {observed:#?}"
            );
            std::thread::yield_now();
        }
    }

    fn forge_moved(&mut self) -> bool {
        let mut moved = false;
        loop {
            match self.changes.try_recv() {
                Ok(_) | Err(TryRecvError::Lagged(_)) => moved = true,
                Err(TryRecvError::Empty | TryRecvError::Closed) => return moved,
            }
        }
    }

    pub(crate) fn idle(&mut self, frames: usize) -> Span {
        Span {
            frames: (0..frames).map(|_| self.frame()).collect(),
        }
    }

    pub(crate) fn scroll(&mut self, at: Point, notches: usize, lines: f32) -> Span {
        let mut span = Span::default();
        for _ in 0..notches {
            self.aqua.dispatch_input(
                &mut self.app,
                self.id,
                &InputEvent::Scroll {
                    at,
                    delta: ScrollDelta {
                        x: 0.0,
                        y: lines * 16.0,
                    },
                },
            );
            span.extend(self.settle("a wheel notch", |_| true));
        }
        span
    }
}
