#![cfg(target_os = "linux")]

#[path = "support/editor.rs"]
mod client;
#[path = "simulation/engine.rs"]
mod engine;
#[path = "simulation/forge.rs"]
mod forge;
#[path = "simulation/observer.rs"]
mod observer;
#[path = "simulation/plan.rs"]
mod plan;

use artisan_api::{
    CreateProject, CreateThread, DraftSnapshot, DraftWriteResult, EngineConfig, EngineId, Item,
    ItemId, ItemKind, Permissions, ProjectId, RunStatus, SendMessage, State, ThreadId, WriteDraft,
};
use client::{Editor, Outcome};
use forge::Forge;
use observer::{Observer, Screen};
use plan::{Action, Delivery, Ending, Policy, Toggle};
use std::io::Write as _;
use std::time::Duration;
use test_support::replay::ReplayingEngine;
use test_support::transcript::Transcript;

const FIXED_SEEDS: [u64; 8] = [1, 3, 5, 7, 42, 45, 375, 0x5EED];

#[test]
fn forge_keeps_its_promises_under_seeded_faults() {
    for seed in seeds() {
        simulate(seed);
    }
}

fn seeds() -> Vec<u64> {
    let parse = |name: &str, text: String| -> u64 {
        text.parse()
            .unwrap_or_else(|error| panic!("{name}={text} is not a number: {error}"))
    };
    if let Ok(seed) = std::env::var("SIMULATION_SEED") {
        return vec![parse("SIMULATION_SEED", seed)];
    }
    if let Ok(count) = std::env::var("SIMULATION_SWEEP") {
        let count = parse("SIMULATION_SWEEP", count);
        let base = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is past 1970")
            .as_secs();
        writeln!(std::io::stdout(), "sweeping {count} seeds from {base}")
            .expect("the simulation writes to the test log");
        return (0..count).map(|offset| base.wrapping_add(offset)).collect();
    }
    FIXED_SEEDS.to_vec()
}

struct Reproduce {
    seed: u64,
    actions: Vec<Action>,
    engine: Option<ReplayingEngine>,
}

impl Drop for Reproduce {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let engine = self
                .engine
                .as_ref()
                .map_or_else(String::new, ReplayingEngine::log);
            writeln!(
                std::io::stderr(),
                "simulation seed {seed} failed\n\
                 replay it with: cargo build -p forge-http --example replay_engine && SIMULATION_SEED={seed} cargo nextest run -p forge-http --test simulation\n\
                 actions: {actions:#?}\n\
                 what the replaying engine saw:\n{engine}",
                seed = self.seed,
                actions = self.actions,
            )
            .expect("the simulation writes to the test log");
        }
    }
}

struct Session {
    runtime: tokio::runtime::Runtime,
    forge: Forge,
    editor: Editor,
    observer: Observer,
    thread: ThreadId,
    acknowledged: Vec<(ItemId, String)>,
    draft: DraftSnapshot,
    expected_replies: Vec<String>,
    policy: Policy,
    resumes: bool,
}

fn simulate(seed: u64) {
    let actions = plan::plan(seed);
    let dir = tempfile::tempdir().expect("create the simulation folder");
    let mut reproduce = Reproduce {
        seed,
        actions: actions.clone(),
        engine: None,
    };
    let data = dir.path().join("forge");
    let script = engine::script(&actions);
    reproduce.engine = Some(ReplayingEngine::install_transcript(
        &data,
        "replay_engine",
        EngineId::Claude,
        &script,
    ));
    let mut session = Session::open(&data, &dir.path().join("work"));
    for action in &actions {
        session.act(*action);
    }
    session.check(
        &script,
        reproduce
            .engine
            .as_ref()
            .expect("invariant: the engine was installed above"),
    );
}

impl Session {
    fn open(data: &std::path::Path, work: &std::path::Path) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("the Editor's runtime starts");
        let (forge, invite) = Forge::start(data);
        let editor = Editor::new(&invite.forge_certificate);
        let address = forge.address();
        std::fs::create_dir_all(work).expect("create the project folder");
        let (thread, head, draft) = runtime.block_on(async {
            editor.pair(address, invite.code).await;
            let project = ProjectId::new();
            accepted::<serde_json::Value>(
                editor
                    .post(
                        address,
                        "/projects",
                        &CreateProject {
                            id: project.clone(),
                            path: work.display().to_string(),
                            name: None,
                        },
                        None,
                    )
                    .await,
                "create the project",
            );
            let mut config = EngineConfig::new(EngineId::Claude);
            config.permissions = Permissions::Auto;
            let thread = CreateThread {
                id: ThreadId::new(),
                project_id: project,
                config,
                title: Some("Simulation".to_owned()),
                parent_thread_id: None,
                spawned_by: None,
            };
            accepted::<serde_json::Value>(
                editor.post(address, "/threads", &thread, None).await,
                "create the thread",
            );
            let head = editor.get::<State>(address, "/state").await.head;
            let draft: DraftSnapshot = editor.get(address, &format!("/drafts/{}", thread.id)).await;
            (thread.id, head, draft)
        });
        Self {
            runtime,
            forge,
            editor,
            observer: Observer::new(head),
            thread,
            acknowledged: Vec::new(),
            draft,
            expected_replies: Vec::new(),
            policy: Policy::default(),
            resumes: false,
        }
    }

    fn act(&mut self, action: Action) {
        match action {
            Action::Turn {
                number,
                delivery,
                ending,
            } => self.turn(number, delivery, ending),
            Action::Draft { number, disk_fails } => self.save_draft(number, disk_fails),
            Action::Restart(stop) => self.forge.restart(stop),
            Action::StepClock { by_ms } => self.forge.clock().step(by_ms),
            Action::DropEvents { after } => self.observer.cut_next_stream(after),
            Action::Toggle(toggle) => self.toggle(toggle),
        }
    }

    fn toggle(&mut self, toggle: Toggle) {
        let address = self.forge.address();
        let request = match toggle {
            Toggle::ContinueAfterRestart(on) => self
                .editor
                .http()
                .put(Editor::url(address, "/settings/continue_after_restart"))
                .json(&on),
            Toggle::Archived(on) => self
                .editor
                .http()
                .patch(Editor::url(address, &format!("/threads/{}", self.thread)))
                .json(&artisan_api::UpdateThread {
                    archived: Some(on),
                    ..artisan_api::UpdateThread::default()
                }),
        };
        let status = self
            .runtime
            .block_on(request.send())
            .unwrap_or_else(|error| panic!("{toggle:?} reaches a running Forge: {error}"))
            .status();
        assert!(status.is_success(), "{toggle:?} answered {status}");
        self.policy.toggle(toggle);
    }

    fn turn(&mut self, number: usize, delivery: Delivery, ending: Ending) {
        let text = engine::message(number);
        let message = SendMessage {
            draft: None,
            id: ItemId::new(),
            text: text.clone(),
            images: Vec::new(),
            config: None,
        };
        let thread = self.thread.clone();
        let path = format!("/threads/{thread}/messages");
        let before = self.observer.screen().settled(&thread);
        let address = self.forge.address();
        self.runtime.block_on(async {
            match delivery {
                Delivery::Clean => {}
                Delivery::ResponseLost => {
                    drop(
                        self.editor
                            .post::<Item>(address, &path, &message, None)
                            .await,
                    );
                }
                Delivery::Aborted { after_ms } => {
                    let cut = Some(Duration::from_millis(after_ms));
                    let outcome = self
                        .editor
                        .post::<Item>(address, &path, &message, cut)
                        .await;
                    assert!(
                        !matches!(outcome, Outcome::Refused { .. }),
                        "a send on a healthy Forge was refused: {outcome:?}"
                    );
                }
                Delivery::DiskFails => {
                    self.forge.disk().fail_next_commit();
                    let outcome = self
                        .editor
                        .post::<Item>(address, &path, &message, None)
                        .await;
                    self.forge.disk().heal();
                    assert!(
                        !matches!(outcome, Outcome::Lost),
                        "invariant: an uncut send is answered"
                    );
                }
            }
        });
        let sent: Item = self.runtime.block_on(async {
            accepted(
                self.editor.post(address, &path, &message, None).await,
                "send the message",
            )
        });
        assert_eq!(
            sent.id, message.id,
            "Forge acknowledges the message it was sent"
        );
        self.acknowledged.push((message.id, text));
        self.end_turn(&thread, number, ending, before);
    }

    fn end_turn(&mut self, thread: &ThreadId, number: usize, ending: Ending, before: usize) {
        let thread = thread.clone();
        match ending {
            Ending::Replies => {
                self.expected_replies.push(engine::reply(number));
                self.wait_settled(&thread, before + 1, "the turn to finish");
            }
            Ending::EngineDies => {
                self.wait_settled(&thread, before + 1, "the turn whose engine died to end");
            }
            Ending::ForgeDies(stop) => {
                let progress = engine::progress(number);
                self.wait(
                    &format!("the engine's progress {progress:?}"),
                    |_, screen| assistant_says(screen, &thread, &progress),
                );
                self.forge.restart(stop);
                if self.policy.carries_on() {
                    self.expected_replies.push(engine::reply(number));
                    let reply = engine::reply(number);
                    self.wait("the turn to carry on after Forge restarted", |_, screen| {
                        assistant_says(screen, &thread, &reply) && !screen.live(&thread)
                    });
                } else {
                    self.wait_settled(
                        &thread,
                        before + 1,
                        "the interrupted turn to stay ended after Forge restarted",
                    );
                }
            }
            Ending::ForgeDiesBeforeSession(stop) => {
                let id = self
                    .acknowledged
                    .last()
                    .map(|(id, _)| id.clone())
                    .expect("invariant: the message was just acknowledged");
                self.wait("the engine to take the message", |_, screen| {
                    taken(screen, &thread, &id)
                });
                self.forge.restart(stop);
                if self.resumes && self.policy.carries_on() {
                    let reply = engine::reply(number);
                    self.expected_replies.push(reply.clone());
                    self.wait(
                        "the resumed turn to carry on after Forge restarted",
                        |_, screen| {
                            assistant_says(screen, &thread, &reply) && !screen.live(&thread)
                        },
                    );
                } else {
                    self.wait_settled(
                        &thread,
                        before + 1,
                        "the turn with no session to carry on to stay ended after Forge restarted",
                    );
                }
            }
        }
        self.resumes = ending.leaves_session(self.resumes);
    }

    fn wait_settled(&mut self, thread: &ThreadId, runs: usize, what: &str) {
        self.wait(what, |_, screen| {
            screen.settled(thread) >= runs && !screen.live(thread)
        });
    }

    fn wait(&mut self, what: &str, done: impl Fn(artisan_api::Seq, &Screen) -> bool + Send + Sync) {
        let address = self.forge.address();
        self.runtime
            .block_on(self.observer.until(&self.editor, address, what, done));
    }

    fn save_draft(&mut self, number: usize, disk_fails: bool) {
        let address = self.forge.address();
        let path = format!("/drafts/{}", self.thread);
        let write = WriteDraft {
            expected_revision: self.draft.revision,
            body: serde_json::json!({ "text": format!("Draft {number}") }),
        };
        if disk_fails {
            self.forge.disk().fail_next_commit();
        }
        let outcome = self
            .runtime
            .block_on(self.editor.put::<DraftWriteResult>(address, &path, &write));
        self.forge.disk().heal();
        match outcome {
            Outcome::Accepted(result) => {
                assert!(
                    result.saved,
                    "a draft written at its current revision is saved"
                );
                self.draft = result.current;
            }
            Outcome::Refused { status, error } => assert!(
                disk_fails,
                "a draft write refused with {status} ({error:?}) while the disk was healthy"
            ),
            Outcome::Lost => unreachable!("invariant: an uncut draft write is answered"),
        }
    }

    fn check(mut self, script: &Transcript, engine: &ReplayingEngine) {
        let address = self.forge.address();
        let thread = self.thread.clone();
        let head = self
            .runtime
            .block_on(self.editor.get::<State>(address, "/state"))
            .head;
        self.wait("the Editor to catch up with Forge", |cursor, _| {
            cursor >= head
        });
        let fresh = self
            .runtime
            .block_on(self.observer.fresh(&self.editor, address, head));
        let resumed = self.observer.screen();
        assert_eq!(
            summary(&fresh, &thread),
            summary(resumed, &thread),
            "the Editor that reconnected {} times sees the same thread as a fresh one",
            self.observer.reconnects()
        );

        let state: State = self.runtime.block_on(self.editor.get(address, "/state"));
        let live: Vec<_> = state
            .runs
            .iter()
            .filter(|run| run.status() == RunStatus::Running)
            .collect();
        assert!(
            live.is_empty(),
            "runs left running with no engine: {live:#?}"
        );
        for run in fresh.runs(&thread) {
            if run.status() == RunStatus::Failed {
                assert!(
                    run.error().is_some(),
                    "a {:?} run tells the user why: {run:#?}",
                    run.status()
                );
            }
        }

        let items = fresh.thread_items(&thread);
        for (id, text) in &self.acknowledged {
            let copies: Vec<_> = items
                .iter()
                .filter(|item| user_text(item) == Some(text.as_str()))
                .collect();
            assert!(
                copies.len() == 1 && &copies[0].id == id,
                "the acknowledged message {text:?} ({id}) is in the thread exactly once: {copies:#?}"
            );
        }
        for reply in &self.expected_replies {
            let copies = items
                .iter()
                .filter(|item| assistant_text(item) == Some(reply.as_str()))
                .count();
            assert_eq!(copies, 1, "the reply {reply:?} is shown exactly once");
        }

        let log = engine.log();
        assert!(
            turn_failures(&log).is_empty(),
            "Forge sent the engine something other than the user's messages:\n{log}"
        );
        assert_eq!(
            engine.launches(),
            script.launches(),
            "Forge started the engine once per message and once per carried-on turn"
        );

        let draft: DraftSnapshot = self
            .runtime
            .block_on(self.editor.get(address, &format!("/drafts/{thread}")));
        assert_eq!(draft, self.draft, "the last saved draft survives");
    }
}

fn turn_failures(log: &str) -> Vec<&str> {
    let mut in_turn = false;
    log.lines()
        .filter(|line| {
            if line.starts_with("started with") {
                in_turn = false;
            } else if line.starts_with("replaying launch") {
                in_turn = true;
            }
            in_turn && line.starts_with("failed:")
        })
        .collect()
}

fn accepted<T>(outcome: Outcome<T>, what: &str) -> T {
    match outcome {
        Outcome::Accepted(value) => value,
        Outcome::Refused { status, error } => panic!("{what}: refused with {status}: {error:?}"),
        Outcome::Lost => unreachable!("invariant: {what} was not cut"),
    }
}

fn user_text(item: &Item) -> Option<&str> {
    match &item.kind {
        ItemKind::UserMessage { text, .. } => text.inline(),
        ItemKind::AssistantMessage { .. }
        | ItemKind::Tool { .. }
        | ItemKind::Agent { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => None,
    }
}

fn assistant_text(item: &Item) -> Option<&str> {
    match &item.kind {
        ItemKind::AssistantMessage { text, .. } => text.inline(),
        ItemKind::UserMessage { .. }
        | ItemKind::Tool { .. }
        | ItemKind::Agent { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => None,
    }
}

fn taken(screen: &Screen, thread: &ThreadId, id: &ItemId) -> bool {
    screen
        .thread_items(thread)
        .iter()
        .any(|item| &item.id == id && item.status == artisan_api::Status::Done)
}

fn assistant_says(screen: &Screen, thread: &ThreadId, text: &str) -> bool {
    screen
        .thread_items(thread)
        .iter()
        .any(|item| assistant_text(item) == Some(text))
}

fn summary(screen: &Screen, thread: &ThreadId) -> Vec<String> {
    let items = screen
        .thread_items(thread)
        .into_iter()
        .map(|item| format!("{} {:?} {:?}", item.id, item.status, item.kind));
    let runs = screen
        .runs(thread)
        .map(|run| format!("{} {:?}", run.id, run.status()));
    items.chain(runs).collect()
}
