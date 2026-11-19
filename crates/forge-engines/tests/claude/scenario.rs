use super::ClaudeHarness;
use crate::{Harness as _, Input, Outcome, RunIo, Turn, UserInput, fake_engine};
use artisan_api::{EngineConfig, EngineId, ThreadId};
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub(super) fn turn(dir: &Path, scenario: &str, env: &[(&str, &str)]) -> Turn {
    Turn {
        thread_id: ThreadId::from("thread"),
        engine: EngineId::Claude,
        binary: fake_engine::binary(),
        home: dir.join("data/engines/claude/home"),
        cwd: dir.to_owned(),
        config: EngineConfig::new(EngineId::Claude),
        session_id: Some("s1".to_owned()),
        input: UserInput {
            text: "go".to_owned(),
            ..UserInput::default()
        },
        launch: fake_engine::launch(scenario, env),
        trace_dir: None,
    }
}

pub(super) struct Running {
    outcome: JoinHandle<Outcome>,
    inputs: mpsc::Sender<Input>,
    cancel: CancellationToken,
    drained: JoinHandle<()>,
}

impl Running {
    pub(super) fn cancel(&self) {
        self.cancel.cancel();
    }

    pub(super) async fn steer(&self, text: &str) {
        let steer = UserInput {
            text: text.to_owned(),
            ..UserInput::default()
        };
        self.inputs.send(Input::Steer(steer)).await.unwrap();
    }

    pub(super) async fn outcome(&mut self, within: Duration) -> Option<Outcome> {
        tokio::time::timeout(within, &mut self.outcome)
            .await
            .ok()
            .map(|joined| joined.expect("the harness task completes"))
    }

    pub(super) async fn drained(self) {
        self.drained.await.unwrap();
    }
}

pub(super) fn start(turn: Turn) -> Running {
    let (inputs, inbox) = mpsc::channel(8);
    let (events, mut received) = mpsc::channel(256);
    let cancel = CancellationToken::new();
    let outcome = tokio::spawn(ClaudeHarness.run(
        turn,
        RunIo {
            inbox,
            events,
            cancel: cancel.clone(),
        },
    ));
    let drained = tokio::spawn(async move { while received.recv().await.is_some() {} });
    Running {
        outcome,
        inputs,
        cancel,
        drained,
    }
}
