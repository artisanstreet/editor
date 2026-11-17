use super::*;

async fn in_generation(gate: Arc<tokio::sync::Notify>) -> Fixture {
    let harness: Arc<dyn Harness> = Arc::new(Scripted(Arc::new(move |turn, _| {
        let gate = Arc::clone(&gate);
        Box::pin(async move {
            match turn.input.text.as_str() {
                "Your reviewers finished." => {
                    gate.notified().await;
                    Outcome::done()
                }
                "Break." => Outcome::failed("broke"),
                _ => Outcome::done(),
            }
        })
    })));
    fixture_with_parts(
        EngineId::Claude,
        Arc::new(move |_| Arc::clone(&harness)),
        disk::Probe::system(),
        Membership::Generation,
    )
    .await
}

async fn reports(fixture: &Fixture, agent: &ItemId, count: usize) {
    fixture
        .wait_for(|items| {
            items
                .iter()
                .filter(|item| {
                    matches!(&item.kind, ItemKind::UserMessage { from_agent: Some(from), .. } if from == agent)
                })
                .count()
                == count
        })
        .await;
}

async fn item_reaches(fixture: &Fixture, agent: &ItemId, status: Status) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while fixture
            .hub
            .store()
            .item(agent)
            .await
            .unwrap()
            .unwrap()
            .status
            != status
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent status reached");
}

async fn held_elsewhere(
    fixture: &Fixture,
    thread: &ThreadId,
) -> (runtime::Generation, runtime::Lease) {
    let next = active_generation(fixture.dir.path());
    let lease = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(lease) = next.claim(thread).unwrap() {
                return lease;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the thread's lease");
    (next, lease)
}

async fn polled(fixture: &Fixture, agent: &ItemId) -> AgentState {
    fixture
        .hub
        .agent_status(&fixture.thread_id, agent.as_str())
        .await
        .unwrap()
        .state
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_settles_and_reports_when_its_run_ends() {
    let fixture = in_generation(Arc::default()).await;
    let (agent, child) = background_agent(&fixture).await;
    fixture
        .hub
        .send_message(&child, message("Review the diff."))
        .await
        .unwrap();
    reports(&fixture, &agent, 1).await;
    item_reaches(&fixture, &agent, Status::Done).await;
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_settled_agent_that_is_woken_works_again_and_reports_again() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let fixture = in_generation(Arc::clone(&gate)).await;
    let (agent, child) = background_agent(&fixture).await;
    fixture
        .hub
        .send_message(&child, message("Review the diff."))
        .await
        .unwrap();
    reports(&fixture, &agent, 1).await;
    fixture
        .hub
        .send_message(&child, message("Your reviewers finished."))
        .await
        .unwrap();
    item_reaches(&fixture, &agent, Status::Running).await;
    gate.notify_one();
    reports(&fixture, &agent, 2).await;
    item_reaches(&fixture, &agent, Status::Done).await;
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ended_agent_stays_settled_while_another_generation_holds_its_thread() {
    for (text, status, state) in [
        ("Review the diff.", Status::Done, AgentState::Done),
        ("Break.", Status::Failed, AgentState::Failed),
    ] {
        let fixture = in_generation(Arc::default()).await;
        let (agent, child) = background_agent(&fixture).await;
        fixture
            .hub
            .send_message(&child, message(text))
            .await
            .unwrap();
        reports(&fixture, &agent, 1).await;
        item_reaches(&fixture, &agent, status).await;
        let _held = held_elsewhere(&fixture, &child).await;
        assert_eq!(polled(&fixture, &agent).await, state);
        assert_eq!(
            fixture
                .hub
                .store()
                .item(&agent)
                .await
                .unwrap()
                .unwrap()
                .status,
            status
        );
        reports(&fixture, &agent, 1).await;
        fixture.hub.shutdown();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_another_generation_is_carrying_on_keeps_working() {
    let fixture = in_generation(Arc::default()).await;
    let (agent, child) = interrupted_agent(&fixture).await;
    let _held = held_elsewhere(&fixture, &child).await;
    assert_eq!(polled(&fixture, &agent).await, AgentState::Working);
    assert_eq!(
        fixture
            .hub
            .store()
            .item(&agent)
            .await
            .unwrap()
            .unwrap()
            .status,
        Status::Running
    );
    fixture.hub.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_follow_up_cancelled_before_it_runs_settles_without_reporting_again() {
    let fixture = in_generation(Arc::default()).await;
    let (agent, child) = background_agent(&fixture).await;
    fixture
        .hub
        .send_message(&child, message("Review the diff."))
        .await
        .unwrap();
    reports(&fixture, &agent, 1).await;
    item_reaches(&fixture, &agent, Status::Done).await;
    fixture.hub.generation().unwrap().set_accepting(false);
    let info = fixture
        .hub
        .message_agent(&fixture.thread_id, agent.as_str(), "One more thing.".into())
        .await
        .unwrap();
    assert_eq!(info.state, AgentState::Working);
    fixture.hub.cancel(&child).await.unwrap();
    item_reaches(&fixture, &agent, Status::Done).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    reports(&fixture, &agent, 1).await;
    fixture.hub.shutdown();
}
