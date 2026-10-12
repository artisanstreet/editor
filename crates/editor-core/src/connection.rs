//! The connection to one Forge: fetch `/state`, follow `/events?since=N`,
//! back off and resume after a failure. The only sync state is `since`.

use crate::client::ForgeClient;
use crate::runtime::runtime;
use anyhow::{Context, Result, bail};
use artisan_api::{Envelope, Event, Seq};
use futures::channel::mpsc::UnboundedSender;
use statig::blocking::{self, IntoStateMachineExt as _, Outcome};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// What the connection tells the host's store.
#[derive(Debug)]
pub enum Update {
    /// A full state: replace everything.
    State(Box<artisan_api::State>),
    Event(Box<Envelope>),
    Status(ConnectionStatus),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Live,
    /// Waiting before retry `attempt`, with the reason.
    Offline {
        attempt: u32,
        reason: String,
    },
}

/// Starts following a Forge. Ends when the receiver is dropped.
/// `retry` cuts a wait between attempts short (the reader asked to retry).
pub fn follow(client: ForgeClient, updates: UnboundedSender<Update>, retry: Arc<Notify>) {
    runtime().spawn(async move {
        let mut machine = Machine::default().state_machine();
        let mut effect = Effect::None;
        machine.handle_with_context(&Input::Start, &mut effect);
        loop {
            match std::mem::take(&mut effect) {
                Effect::None => return,
                Effect::Connect { since } => {
                    let _ = updates.unbounded_send(Update::Status(ConnectionStatus::Connecting));
                    match session(&client, since, &updates, &mut machine, &mut effect).await {
                        Ok(()) => machine.handle_with_context(&Input::Ended, &mut effect),
                        Err(error) if updates.is_closed() => {
                            tracing::debug!("stopped following Forge: {error:#}");
                            return;
                        }
                        Err(error) => {
                            tracing::warn!("Forge connection failed: {error:#}");
                            machine.handle_with_context(
                                &Input::Failed(format!("{error:#}")),
                                &mut effect,
                            );
                        }
                    }
                }
                Effect::Wait { attempt, reason } => {
                    let _ = updates.unbounded_send(Update::Status(ConnectionStatus::Offline {
                        attempt,
                        reason,
                    }));
                    tokio::select! {
                        () = tokio::time::sleep(backoff(attempt)) => {}
                        () = retry.notified() => {}
                    }
                    machine.handle_with_context(&Input::RetryDue, &mut effect);
                }
            }
            if updates.is_closed() {
                return;
            }
        }
    });
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(250 * 2_u64.saturating_pow(attempt.min(6))).min(Duration::from_secs(15))
}

/// One connected session: state when needed, then events until the stream
/// ends or Forge asks for a reset.
async fn session(
    client: &ForgeClient,
    since: Option<Seq>,
    updates: &UnboundedSender<Update>,
    machine: &mut blocking::StateMachine<Machine>,
    effect: &mut Effect,
) -> Result<()> {
    let since = if let Some(since) = since {
        since
    } else {
        let state = client.state().await?;
        if state.protocol != artisan_api::PROTOCOL_VERSION {
            bail!(
                "Forge {} speaks protocol {}; update Forge or the Editor",
                state.forge_version,
                state.protocol
            );
        }
        let head = state.head;
        updates
            .unbounded_send(Update::State(Box::new(state)))
            .ok()
            .context("store closed")?;
        machine.handle_with_context(&Input::Seen(head), effect);
        head
    };
    let mut response = client
        .raw()
        .get(client.events_url(since))
        .header("accept", "text/event-stream")
        .send()
        .await
        .context("open the event stream")?;
    if !response.status().is_success() {
        bail!("event stream refused: {}", response.status());
    }
    machine.handle_with_context(&Input::Connected, effect);
    updates
        .unbounded_send(Update::Status(ConnectionStatus::Live))
        .ok()
        .context("store closed")?;
    let mut buffer = String::new();
    while let Some(chunk) = response.chunk().await.context("read the event stream")? {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buffer.find("\n\n") {
            let block: String = buffer.drain(..end + 2).collect();
            let data: String = block
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|line| line.strip_prefix(' ').unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n");
            if data.is_empty() {
                continue;
            }
            let envelope: Envelope = match serde_json::from_str(&data) {
                Ok(envelope) => envelope,
                Err(error) => {
                    tracing::warn!("skipped an event Forge sent: {error}");
                    continue;
                }
            };
            if matches!(envelope.event, Event::Reset) {
                machine.handle_with_context(&Input::Reset, effect);
                return Ok(());
            }
            if let Some(seq) = envelope.seq {
                machine.handle_with_context(&Input::Seen(seq), effect);
            }
            updates
                .unbounded_send(Update::Event(Box::new(envelope)))
                .ok()
                .context("store closed")?;
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
enum Effect {
    #[default]
    None,
    /// Open a session; `None` means fetch the state first.
    Connect {
        since: Option<Seq>,
    },
    Wait {
        attempt: u32,
        reason: String,
    },
}

#[derive(Debug)]
enum Input {
    Start,
    Connected,
    Seen(Seq),
    /// The stream ended cleanly (Forge restarted, network dropped).
    Ended,
    Failed(String),
    /// Forge says the client fell too far behind.
    Reset,
    RetryDue,
}

/// Lifecycle data only: the last seq seen. Handlers put the next effect in
/// the context; the driver performs it.
#[derive(Default)]
struct Machine {
    since: Option<Seq>,
}

#[blocking::state_machine(initial = "State::disconnected()", state(derive(Debug)))]
impl Machine {
    #[state]
    fn disconnected(&mut self, context: &mut Effect, event: &Input) -> Outcome<State> {
        match event {
            Input::Start => {
                *context = Effect::Connect { since: self.since };
                Outcome::Transition(State::connecting())
            }
            _ => Outcome::Handled,
        }
    }

    #[state]
    fn connecting(&mut self, context: &mut Effect, event: &Input) -> Outcome<State> {
        match event {
            Input::Seen(seq) => {
                self.since = Some(*seq);
                Outcome::Handled
            }
            Input::Connected => Outcome::Transition(State::live()),
            Input::Failed(reason) => {
                *context = Effect::Wait {
                    attempt: 1,
                    reason: reason.clone(),
                };
                Outcome::Transition(State::backoff(1))
            }
            Input::Reset => {
                self.since = None;
                *context = Effect::Connect { since: None };
                Outcome::Handled
            }
            _ => Outcome::Handled,
        }
    }

    #[state]
    fn live(&mut self, context: &mut Effect, event: &Input) -> Outcome<State> {
        match event {
            Input::Seen(seq) => {
                self.since = Some(*seq);
                Outcome::Handled
            }
            Input::Reset => {
                self.since = None;
                *context = Effect::Connect { since: None };
                Outcome::Transition(State::connecting())
            }
            Input::Ended => {
                *context = Effect::Connect { since: self.since };
                Outcome::Transition(State::connecting())
            }
            Input::Failed(reason) => {
                *context = Effect::Wait {
                    attempt: 1,
                    reason: reason.clone(),
                };
                Outcome::Transition(State::backoff(1))
            }
            _ => Outcome::Handled,
        }
    }

    #[state]
    fn backoff(
        &mut self,
        attempt: &mut u32,
        context: &mut Effect,
        event: &Input,
    ) -> Outcome<State> {
        match event {
            Input::RetryDue => {
                *context = Effect::Connect { since: self.since };
                Outcome::Transition(State::retrying(*attempt))
            }
            _ => Outcome::Handled,
        }
    }

    /// Connecting again after `attempt` failures.
    #[state]
    fn retrying(
        &mut self,
        attempt: &mut u32,
        context: &mut Effect,
        event: &Input,
    ) -> Outcome<State> {
        match event {
            Input::Seen(seq) => {
                self.since = Some(*seq);
                Outcome::Handled
            }
            Input::Connected => Outcome::Transition(State::live()),
            Input::Failed(reason) => {
                let next = attempt.saturating_add(1);
                *context = Effect::Wait {
                    attempt: next,
                    reason: reason.clone(),
                };
                Outcome::Transition(State::backoff(next))
            }
            Input::Reset => {
                self.since = None;
                *context = Effect::Connect { since: None };
                Outcome::Handled
            }
            _ => Outcome::Handled,
        }
    }
}
