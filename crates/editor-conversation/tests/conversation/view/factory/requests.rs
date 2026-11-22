use crate::conversation::adapter::Action;
use crate::conversation::controller::Conversation;
use crate::conversation::thread::ItemAnswer;
use crate::conversation::view::factory::realize::harness::{Transcript, deliver};
use artisan_api::{
    Approval, ApprovalRequest, Decision, EngineId, Event, Item, ItemId, ItemKind, Run, RunId,
    RunOutcome, RunState, Seq, Status, TextContent, ThreadId, Usage,
};
use editor_core::{Core, CoreEvent, HostEvent, Screen};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::{HeldRequest, StubForge, empty_state};
use test_support::items::{item, read_tool};
use test_support::wait::{LIVENESS, wait_until};

const APPROVE: &str = "approve-asked-0";

struct Approving {
    core: Core,
    forge: StubForge,
    conversation: Conversation,
    transcript: Transcript,
    thread: ThreadId,
    id: ItemId,
    seq: Seq,
    _directory: tempfile::TempDir,
}

fn run(thread: &ThreadId, state: RunState, seq: Seq) -> Run {
    Run {
        config: None,
        id: RunId("approval-run".into()),
        thread_id: thread.clone(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        state,
        usage: Usage::default(),
        session_id: None,
        seq,
        started_at: 0,
        continuation: None,
    }
}

fn asked(thread: &ThreadId, status: Status, seq: Seq) -> Item {
    let mut asked = item("asked", thread.clone(), 1, read_tool("src/main.rs"));
    asked.seq = seq;
    asked.status = status;
    asked.run_id = Some(RunId("approval-run".into()));
    if let ItemKind::Tool { approval, .. } = &mut asked.kind {
        *approval = Some(Approval {
            request: ApprovalRequest::Command {
                command: TextContent::Inline("cargo test".into()),
                cwd: None,
            },
            reason: None,
            options: vec![Decision::AllowOnce, Decision::DenyOnce],
            decision: None,
        });
    }
    asked
}

impl Approving {
    fn start() -> Self {
        let StubbedHost {
            mut core,
            forge,
            host,
            directory,
        } = StubbedHost::start(empty_state(), axum::Router::new());
        let thread = ThreadId("approval-thread".into());
        core.navigate(Screen::Thread {
            host: host.clone(),
            thread: thread.clone(),
        });
        let mut conversation = Conversation::new(&core);
        conversation.open_thread(&mut core, host, thread.clone());
        let transcript = Transcript::open(&conversation, &core);
        let mut approving = Self {
            core,
            forge,
            conversation,
            transcript,
            thread,
            id: ItemId("asked".into()),
            seq: 0,
            _directory: directory,
        };
        approving.forge.publish(Event::Run {
            run: run(&approving.thread, RunState::Running, 1),
        });
        approving.publish(Status::AwaitingUser);
        approving
    }

    fn end_run(&mut self) {
        self.forge.publish(Event::Run {
            run: run(
                &self.thread,
                RunState::Finished {
                    outcome: RunOutcome::Cancelled,
                    error: None,
                    ended_at: 1,
                },
                2,
            ),
        });
        let thread = self.thread.clone();
        wait_until("the ended run", LIVENESS, || {
            self.core.drain_with(|core, event| {
                self.conversation.core_event(core, &event, false);
            });
            self.conversation
                .open
                .as_ref()
                .and_then(|(host, _)| self.core.host(host)?.runs.get(&thread))
                .is_some_and(|run| !run.is_live())
        });
    }

    fn keeps_answer(&self) -> bool {
        self.conversation
            .view
            .as_ref()
            .is_some_and(|view| view.answers.get(&self.id).is_some())
    }

    fn publish(&mut self, status: Status) {
        self.seq += 1;
        self.forge.publish(Event::Item {
            item: asked(&self.thread, status, self.seq),
        });
        let id = self.id.clone();
        deliver(
            &mut self.core,
            &mut self.conversation,
            "the approval request update",
            |event| matches!(event, CoreEvent::Host { event: HostEvent::Item(item), .. } if item.id == id && item.status == status),
        );
    }

    fn approve(&mut self) -> bool {
        self.transcript.render(&mut self.conversation, &self.core);
        let actions = self.transcript.click(APPROVE);
        let pressed = !actions.is_empty();
        for action in actions {
            let Action::Answer { id, answer } = action else {
                panic!("the approve button only answers, it sent {action:?}");
            };
            self.conversation.answer(&self.core, id, answer);
        }
        pressed
    }

    fn held_answer(&self) -> HeldRequest {
        self.forge.take("POST", "/items/asked/answer")
    }

    fn settle_reply(&mut self) {
        let id = self.id.clone();
        wait_until("the answer reply", LIVENESS, || {
            self.conversation.drain(&mut self.core);
            !self
                .conversation
                .view
                .as_ref()
                .and_then(|view| view.answers.get(&id))
                .is_some_and(|answer| matches!(answer, ItemAnswer::Sending { .. }))
        });
    }

    fn texts(&mut self) -> Vec<String> {
        self.transcript.render(&mut self.conversation, &self.core);
        self.transcript.texts()
    }
}

#[test]
fn an_accepted_approval_keeps_its_buttons_disabled_until_forge_resolves_it() {
    let mut approving = Approving::start();
    assert!(
        approving.texts().iter().any(|text| text == "Approve"),
        "the request shows its buttons"
    );
    assert!(approving.approve(), "the request offers its buttons");
    let sent = approving.held_answer();
    assert!(
        !approving.approve(),
        "the buttons are disabled while the answer is in flight"
    );

    sent.respond(serde_json::json!({}));
    approving.settle_reply();
    assert!(
        !approving.approve(),
        "Forge accepted the answer, so the buttons stay disabled until the request updates"
    );
    assert!(
        !approving.forge.asked("POST", "/items/asked/answer"),
        "an accepted answer is not sent twice"
    );

    approving.publish(Status::Done);
    let texts = approving.texts();
    assert!(
        !texts.iter().any(|text| text == "Approve"),
        "the resolved request no longer offers its buttons: {texts:?}"
    );
}

#[test]
fn an_approval_resolved_before_its_reply_arrives_drops_its_buttons() {
    let mut approving = Approving::start();
    assert!(approving.approve());
    let sent = approving.held_answer();

    approving.publish(Status::Done);
    sent.respond(serde_json::json!({}));
    approving.settle_reply();
    let texts = approving.texts();
    assert!(
        !texts.iter().any(|text| text == "Approve"),
        "the resolved request no longer offers its buttons: {texts:?}"
    );
    assert!(
        approving
            .conversation
            .view
            .as_ref()
            .is_some_and(|view| view.answers.get(&approving.id).is_none()),
        "nothing local is kept for a request Forge already resolved"
    );
}

#[test]
fn an_approval_forge_raises_again_offers_its_buttons_again() {
    let mut approving = Approving::start();
    assert!(approving.approve());
    approving.held_answer().respond(serde_json::json!({}));
    approving.settle_reply();
    assert!(!approving.approve());

    approving.publish(Status::AwaitingUser);
    assert!(
        approving.approve(),
        "Forge asked again after the answer, so the buttons are enabled again"
    );
    approving.held_answer();
}

#[test]
fn an_approval_raised_again_while_the_answer_is_in_flight_stays_answerable() {
    let mut approving = Approving::start();
    assert!(approving.approve());
    let sent = approving.held_answer();

    approving.publish(Status::AwaitingUser);
    sent.respond(serde_json::json!({}));
    approving.settle_reply();
    assert!(
        approving.approve(),
        "the newer request was never answered, so its buttons stay enabled"
    );
    approving.held_answer();
}

#[test]
fn a_resolved_approval_forgets_its_local_answer_while_the_thread_stays_open() {
    let mut approving = Approving::start();
    assert!(approving.approve());
    approving.held_answer().respond(serde_json::json!({}));
    approving.settle_reply();
    assert!(approving.keeps_answer());

    approving.publish(Status::Done);
    approving.texts();
    assert!(
        !approving.keeps_answer(),
        "the resolved request leaves no local answer behind"
    );
}

#[test]
fn an_answered_approval_is_released_when_its_run_ends() {
    let mut approving = Approving::start();
    assert!(approving.approve());
    approving.held_answer().respond(serde_json::json!({}));
    approving.settle_reply();
    assert!(!approving.approve());

    approving.end_run();
    approving.texts();
    assert!(
        !approving.keeps_answer(),
        "the ended run releases the local answer"
    );
}
