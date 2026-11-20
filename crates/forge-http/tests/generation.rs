use crate::{ServeConfig, start_generation_with_harness};
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
    RunStatus, SendMessage, Status, ThreadId,
};
use forge_engines::{EngineEvent, Harness, Input, LiveKind, Outcome, RunIo, Turn};
use forge_runs::{
    ErrorClass, Hub,
    runtime::{self, Command, Registry},
};
use forge_store::Store;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

struct Script {
    name: &'static str,
    starts: Arc<AtomicUsize>,
    release: Arc<Notify>,
}
impl Harness for Script {
    fn run(&self, turn: Turn, mut io: RunIo) -> futures::future::BoxFuture<'static, Outcome> {
        let name = self.name;
        let starts = Arc::clone(&self.starts);
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            starts.fetch_add(1, Ordering::SeqCst);
            io.emit(EngineEvent::Session {
                id: format!("{name}-session"),
            })
            .await;
            io.emit(EngineEvent::InputAccepted { id: turn.input.id })
                .await;
            if turn.input.text == "linger" {
                io.cancel.cancelled().await;
                release.notified().await;
                return Outcome::cancelled();
            }
            if turn.input.text == "hold" {
                io.emit(EngineEvent::Item {
                    key: "question".into(),
                    parent: None,
                    status: Status::AwaitingUser,
                    kind: ItemKind::Question {
                        blocking: true,
                        questions: vec![artisan_api::Question {
                            id: "ready".into(),
                            header: None,
                            text: "Continue?".into(),
                            multi_select: false,
                            options: vec![artisan_api::QuestionOption {
                                id: "yes".into(),
                                label: "Yes".into(),
                                description: None,
                            }]
                            .into(),
                        }]
                        .into(),
                        answers: None,
                        answer_text: None,
                    },
                })
                .await;
                loop {
                    tokio::select! {
                        ()=io.cancel.cancelled()=>return Outcome::cancelled(),
                        input=io.inbox.recv()=>match input {
                            Some(Input::Steer(input)) if input.text == "unheard" => {}
                            Some(Input::Steer(input))=>{
                                io.emit(EngineEvent::InputAccepted {id:input.id}).await;
                                io.emit(EngineEvent::Live {target:LiveKind::Reasoning,text:format!("{name}:{}",input.text),append:false}).await;
                            }
                            Some(Input::Answer {key,answer})=>{
                                assert_eq!(key,"question");
                                assert!(matches!(answer,artisan_api::Answer::Question {..}));
                                io.emit(EngineEvent::Status {key,status:Status::Done}).await;
                                io.emit(EngineEvent::Live {target:LiveKind::Reasoning,text:format!("{name}:answered"),append:false}).await;
                            }
                            Some(Input::ClearGoal|Input::SetPermissions(_))=>{}
                            None=>return Outcome::failed("inbox closed during handoff"),
                        }
                    }
                }
            }
            io.emit(EngineEvent::Item {
                key: "result".into(),
                parent: None,
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: format!("{name}:{}", turn.input.text).into(),
                    phase: Phase::Final,
                },
            })
            .await;
            Outcome::done()
        })
    }
}

fn message(text: &str) -> SendMessage {
    SendMessage {
        id: ItemId::new(),
        text: text.into(),
        images: vec![],
        config: None,
        draft: None,
    }
}

async fn wait(store: &Store, thread: &ThreadId, status: RunStatus) {
    test_support::wait::eventually(
        "the thread's latest run",
        Duration::from_secs(10),
        || async {
            store
                .latest_run(thread)
                .await
                .unwrap()
                .is_some_and(|run| run.status() == status)
        },
    )
    .await;
}

async fn generation(
    data: &std::path::Path,
    registry: &Registry,
    name: &'static str,
    starts: Arc<AtomicUsize>,
    release: Arc<Notify>,
) -> (Hub, std::net::SocketAddr) {
    let harness: Arc<dyn Harness> = Arc::new(Script {
        name,
        starts,
        release,
    });
    let server = start_generation_with_harness(
        ServeConfig {
            data_dir: data.to_path_buf(),
            listen: "127.0.0.1:0".parse().unwrap(),
        },
        registry.clone(),
        Arc::new(move |_| Arc::clone(&harness)),
    )
    .await
    .unwrap();
    let hub = server.hub().clone();
    let address = server.local_addr().unwrap();
    tokio::spawn(server.run());
    (hub, address)
}

#[tokio::test(flavor = "multi_thread")]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario preserves an original run across promotion, steering, answering, cancellation and retirement"
)]
async fn new_turns_use_the_replacement_while_old_turns_keep_their_identity_and_control_channels() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = Store::open(data).unwrap();
    let binary = data
        .join("engines/claude/versions/1.0.0")
        .join(forge_engines::host_entry(EngineId::Claude).unwrap());
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(binary, "").unwrap();
    std::fs::write(data.join("engines/claude/current"), "1.0.0").unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: data.display().to_string(),
            name: None,
        })
        .await
        .unwrap();
    let mut threads = Vec::new();
    for _ in 0..2 {
        threads.push(
            store
                .create_thread(CreateThread {
                    id: ThreadId::new(),
                    project_id: project.id.clone(),
                    config: EngineConfig::new(EngineId::Claude),
                    title: None,
                    parent_thread_id: None,
                    spawned_by: None,
                })
                .await
                .unwrap()
                .id,
        );
    }
    let registry = Registry::open(data).unwrap();
    let old_starts = Arc::new(AtomicUsize::new(0));
    let new_starts = Arc::new(AtomicUsize::new(0));
    let (old, _) = generation(
        data,
        &registry,
        "old",
        Arc::clone(&old_starts),
        Arc::default(),
    )
    .await;
    let old_member = old.generation().unwrap().member().clone();
    runtime::request(&old_member, &Command::Activate)
        .await
        .unwrap();
    registry.promote(&old_member).unwrap();
    old.send_message(&threads[0], message("hold"))
        .await
        .unwrap();
    wait(&store, &threads[0], RunStatus::Running).await;
    let original = store.latest_run(&threads[0]).await.unwrap().unwrap();
    let (replacement, _) = generation(
        data,
        &registry,
        "new",
        Arc::clone(&new_starts),
        Arc::default(),
    )
    .await;
    let new_member = replacement.generation().unwrap().member().clone();
    let mut events = replacement.store().subscribe();
    assert_eq!(
        store.latest_run(&threads[0]).await.unwrap().unwrap().id,
        original.id
    );
    runtime::request(&new_member, &Command::Activate)
        .await
        .unwrap();
    registry.promote(&new_member).unwrap();
    runtime::request(&old_member, &Command::Drain)
        .await
        .unwrap();
    old.send_message(&threads[1], message("fresh"))
        .await
        .unwrap();
    wait(&store, &threads[1], RunStatus::Done).await;
    assert_eq!(
        store.last_reply(&threads[1]).await.unwrap().unwrap(),
        "new:fresh"
    );
    assert_eq!(new_starts.load(Ordering::SeqCst), 1);
    assert_eq!(old_starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime::request(&old_member, &Command::Retire)
            .await
            .unwrap(),
        false
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while !matches!(
            events.recv().await.unwrap().event,
            artisan_api::Event::Reset
        ) {}
    })
    .await
    .unwrap();
    replacement
        .send_message(&threads[0], message("steered"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10),async {loop {if matches!(events.recv().await.unwrap().event,artisan_api::Event::Live {text,..} if text=="old:steered") {break;}}}).await.unwrap();
    let question = awaiting_question(&store).await;
    replacement
        .answer(
            &question.id,
            artisan_api::Answer::Question {
                answers: vec![Some(artisan_api::QuestionAnswer::Choices {
                    ids: vec!["yes".into()].into(),
                })],
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10),async {loop {if matches!(events.recv().await.unwrap().event,artisan_api::Event::Live {text,..} if text=="old:answered") {break;}}}).await.unwrap();
    assert_eq!(
        store.latest_run(&threads[0]).await.unwrap().unwrap().id,
        original.id
    );
    replacement.cancel(&threads[0]).await.unwrap();
    wait(&store, &threads[0], RunStatus::Cancelled).await;
    test_support::wait::eventually(
        "the old generation to let go of its threads",
        Duration::from_secs(10),
        || async { old.local_active_count() == 0 },
    )
    .await;
    assert_eq!(
        runtime::request(&old_member, &Command::Retire)
            .await
            .unwrap(),
        true
    );
    runtime::request(&new_member, &Command::Stop).await.unwrap();
}

struct Overlap {
    directory: tempfile::TempDir,
    store: Store,
    threads: Vec<ThreadId>,
    old: Hub,
    new: Hub,
    new_address: std::net::SocketAddr,
    new_member: runtime::Member,
    new_starts: Arc<AtomicUsize>,
    release: Arc<Notify>,
}

async fn overlap(held: &[&str]) -> Overlap {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path();
    let store = Store::open(data).unwrap();
    let binary = data
        .join("engines/claude/versions/1.0.0")
        .join(forge_engines::host_entry(EngineId::Claude).unwrap());
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(binary, "").unwrap();
    std::fs::write(data.join("engines/claude/current"), "1.0.0").unwrap();
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: data.display().to_string(),
            name: None,
        })
        .await
        .unwrap();
    let mut threads = Vec::new();
    for _ in 0..3 {
        threads.push(
            store
                .create_thread(CreateThread {
                    id: ThreadId::new(),
                    project_id: project.id.clone(),
                    config: EngineConfig::new(EngineId::Claude),
                    title: None,
                    parent_thread_id: None,
                    spawned_by: None,
                })
                .await
                .unwrap()
                .id,
        );
    }
    let registry = Registry::open(data).unwrap();
    let release = Arc::new(Notify::new());
    let (old, _) = generation(
        data,
        &registry,
        "old",
        Arc::new(AtomicUsize::new(0)),
        Arc::clone(&release),
    )
    .await;
    let old_member = old.generation().unwrap().member().clone();
    runtime::request(&old_member, &Command::Activate)
        .await
        .unwrap();
    registry.promote(&old_member).unwrap();
    for (thread, text) in threads.iter().zip(held) {
        old.send_message(thread, message(text)).await.unwrap();
        wait(&store, thread, RunStatus::Running).await;
    }
    let new_starts = Arc::new(AtomicUsize::new(0));
    let (new, new_address) = generation(
        data,
        &registry,
        "new",
        Arc::clone(&new_starts),
        Arc::default(),
    )
    .await;
    let new_member = new.generation().unwrap().member().clone();
    runtime::request(&new_member, &Command::Activate)
        .await
        .unwrap();
    registry.promote(&new_member).unwrap();
    Overlap {
        directory,
        store,
        threads,
        old,
        new,
        new_address,
        new_member,
        new_starts,
        release,
    }
}

async fn awaiting_question(store: &Store) -> artisan_api::PendingItem {
    test_support::wait::eventually("a question", Duration::from_secs(10), || async {
        !store.items_awaiting_user().await.unwrap().is_empty()
    })
    .await;
    store.items_awaiting_user().await.unwrap().remove(0)
}

async fn overlapping(hub: &Hub) {
    test_support::wait::eventually(
        "the generations to overlap",
        Duration::from_secs(10),
        || async { hub.generation().unwrap().overlapping() },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_the_owning_generation_refuses_reaches_the_editor_as_a_conflict() {
    let overlap = overlap(&["hold"]).await;
    let question = awaiting_question(&overlap.store).await;
    overlapping(&overlap.new).await;
    let approval = artisan_api::Answer::Approval {
        decision: artisan_api::Decision::AllowOnce,
    };
    let refused = overlap
        .new
        .answer(&question.id, approval.clone())
        .await
        .unwrap_err();
    assert!(
        matches!(refused, forge_runs::AnswerError::Peer { .. }),
        "{}",
        diagnostics::chain(&refused)
    );
    assert_eq!(refused.class(), ErrorClass::Conflict);
    let editor = crate::tests::paired(&overlap.store, overlap.new_address, "routed answer").await;
    let response = editor
        .http()
        .post(editor.url(&format!("/items/{}/answer", question.id)))
        .json(&approval)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let body: artisan_api::ApiError = response.json().await.unwrap();
    assert_eq!(
        body.failure,
        artisan_api::Failure::Run(artisan_api::RunFailure::AnswerRefused {
            item: Some(question.id.clone()),
            reason: artisan_api::AnswerRefusal::NotAnApproval,
        })
    );
    runtime::request(&overlap.new_member, &Command::Stop)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_editor_reply_that_cannot_reach_the_other_generation_fails_instead_of_expiring() {
    let overlap = overlap(&["hold"]).await;
    overlapping(&overlap.new).await;
    let editor = crate::tests::paired(&overlap.store, overlap.new_address, "editor reply").await;
    let reply = artisan_api::EditorReply::Ok(serde_json::json!(null));
    let unknown = editor
        .http()
        .post(editor.url("/editor/calls/no-such-call"))
        .json(&reply)
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 410);
    let old = overlap.old.generation().unwrap().member().id.clone();
    std::fs::write(
        overlap
            .directory
            .path()
            .join("runtime/generations")
            .join(format!("{old}.json")),
        "not a generation",
    )
    .unwrap();
    let unreachable = editor
        .http()
        .post(editor.url("/editor/calls/no-such-call"))
        .json(&reply)
        .send()
        .await
        .unwrap();
    assert_eq!(unreachable.status(), 500);
    let error: artisan_api::ApiError = unreachable.json().await.unwrap();
    assert_eq!(error.code, artisan_api::ErrorCode::Internal);
    runtime::request(&overlap.new_member, &Command::Stop)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_the_old_generation_has_not_taken_does_not_hold_up_other_threads() {
    let overlap = overlap(&["hold"]).await;
    overlap
        .old
        .send_message(&overlap.threads[0], message("unheard"))
        .await
        .unwrap();
    overlap
        .new
        .send_message(&overlap.threads[1], message("fresh"))
        .await
        .unwrap();
    wait(&overlap.store, &overlap.threads[1], RunStatus::Done).await;
    assert_eq!(
        overlap
            .store
            .last_reply(&overlap.threads[1])
            .await
            .unwrap()
            .unwrap(),
        "new:fresh"
    );
    assert_eq!(overlap.new_starts.load(Ordering::SeqCst), 1);
    runtime::request(&overlap.new_member, &Command::Stop)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn generations_cancelling_each_others_threads_at_once_both_finish() {
    let overlap = overlap(&["hold"]).await;
    overlap
        .new
        .send_message(&overlap.threads[1], message("hold"))
        .await
        .unwrap();
    wait(&overlap.store, &overlap.threads[1], RunStatus::Running).await;
    let (on_old, on_new) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            overlap.new.cancel(&overlap.threads[0]),
            overlap.old.cancel(&overlap.threads[1]),
        )
    })
    .await
    .unwrap();
    on_old.unwrap();
    on_new.unwrap();
    wait(&overlap.store, &overlap.threads[0], RunStatus::Cancelled).await;
    wait(&overlap.store, &overlap.threads[1], RunStatus::Cancelled).await;
    runtime::request(&overlap.new_member, &Command::Stop)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn continue_pressed_on_the_new_generation_reaches_the_one_winding_down_the_stop() {
    let overlap = overlap(&["linger"]).await;
    overlapping(&overlap.new).await;
    overlap.new.cancel(&overlap.threads[0]).await.unwrap();
    wait(&overlap.store, &overlap.threads[0], RunStatus::Cancelled).await;
    assert!(
        overlap.old.is_live(&overlap.threads[0]),
        "the old generation still winds the stop down"
    );

    overlap
        .new
        .continue_turn(&overlap.threads[0])
        .await
        .expect("Continue goes to the generation that owns the thread");
    overlap.release.notify_one();

    test_support::wait::eventually(
        "the stopped turn to carry on",
        Duration::from_secs(10),
        || async {
            overlap
                .store
                .latest_run(&overlap.threads[0])
                .await
                .unwrap()
                .is_some_and(|run| {
                    run.status() == RunStatus::Done
                        && run.continuation == Some(artisan_api::Continuation::Requested)
                })
        },
    )
    .await;
    runtime::request(&overlap.new_member, &Command::Stop)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_the_old_generation_finishes_stops_showing_as_working_on_the_new_one() {
    let overlap = overlap(&["hold"]).await;
    let thread = &overlap.threads[0];
    let mut watching = overlap.new.generation().unwrap().watch();
    assert!(overlap.new.is_live(thread));

    overlap.old.cancel(thread).await.unwrap();

    test_support::wait::within(
        "the new generation to see the old one let go of the thread",
        test_support::wait::LIVENESS,
        async {
            while overlap.new.is_live(thread) {
                watching.changed().await.unwrap();
            }
        },
    )
    .await;
}
