use crate::{ServeConfig, start_generation_with_harness};
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
    RunStatus, SendMessage, Status, ThreadId,
};
use forge_engines::{EngineEvent, Harness, Input, LiveKind, Outcome, RunIo, Turn};
use forge_runs::{
    Hub,
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

struct Script {
    name: &'static str,
    starts: Arc<AtomicUsize>,
}
impl Harness for Script {
    fn run(&self, turn: Turn, mut io: RunIo) -> futures::future::BoxFuture<'static, Outcome> {
        let name = self.name;
        let starts = Arc::clone(&self.starts);
        Box::pin(async move {
            starts.fetch_add(1, Ordering::SeqCst);
            io.emit(EngineEvent::Session {
                id: format!("{name}-session"),
            })
            .await;
            io.emit(EngineEvent::InputAccepted { id: turn.input.id })
                .await;
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
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if store
                .latest_run(thread)
                .await
                .unwrap()
                .is_some_and(|run| run.status == status)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn generation(
    data: &std::path::Path,
    registry: &Registry,
    name: &'static str,
    starts: Arc<AtomicUsize>,
) -> Hub {
    let harness: Arc<dyn Harness> = Arc::new(Script { name, starts });
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
    tokio::spawn(server.run());
    hub
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
    let binary = data.join("engines/claude/versions/1.0.0/claude");
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
    let old = generation(data, &registry, "old", Arc::clone(&old_starts)).await;
    let old_member = old.generation().unwrap().member.clone();
    runtime::request(&old_member, &Command::Activate)
        .await
        .unwrap();
    registry.promote(&old_member).unwrap();
    old.send_message(&threads[0], message("hold"))
        .await
        .unwrap();
    wait(&store, &threads[0], RunStatus::Running).await;
    let original = store.latest_run(&threads[0]).await.unwrap().unwrap();
    let replacement = generation(data, &registry, "new", Arc::clone(&new_starts)).await;
    let new_member = replacement.generation().unwrap().member.clone();
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
    let mut events = replacement.store().subscribe();
    replacement
        .send_message(&threads[0], message("steered"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10),async {loop {if matches!(events.recv().await.unwrap().event,artisan_api::Event::Live {text,..} if text=="old:steered") {break;}}}).await.unwrap();
    let question = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let items = store.items_awaiting_user().await.unwrap();
            if let Some(item) = items.first() {
                break item.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
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
    tokio::time::timeout(Duration::from_secs(10), async {
        while old.local_active_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        runtime::request(&old_member, &Command::Retire)
            .await
            .unwrap(),
        true
    );
    runtime::request(&new_member, &Command::Stop).await.unwrap();
}
