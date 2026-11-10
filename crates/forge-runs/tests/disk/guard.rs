use super::*;
async fn fixture_on_disk(script: Script, free: Arc<AtomicU64>) -> (Fixture, Arc<AtomicU64>) {
    let checks = Arc::new(AtomicU64::new(0));
    let observed = Arc::clone(&checks);
    let harness: Arc<dyn Harness> = Arc::new(Scripted(script));
    let probe = disk::Probe {
        free_space: Arc::new(move |_| {
            let available = free.load(Ordering::Relaxed);
            observed.fetch_add(1, Ordering::Release);
            Ok(available)
        }),
        every: Duration::from_millis(10),
    };
    let fixture = fixture_with_parts(
        EngineId::Claude,
        Arc::new(move |_| Arc::clone(&harness)),
        probe,
    )
    .await;
    (fixture, checks)
}

async fn observed_disk(checks: &AtomicU64) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while checks.load(Ordering::Acquire) < 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

const GIB: u64 = 1 << 30;

fn notices(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Notice { text, .. } = &item.kind {
                text.inline().map(str::to_owned)
            } else {
                None
            }
        })
        .collect()
}

fn errors(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| {
            if let ItemKind::Error { message, .. } = &item.kind {
                message.inline().map(str::to_owned)
            } else {
                None
            }
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_low_disk_tells_the_agent_once_to_free_space() {
    let free = Arc::new(AtomicU64::new(7 * GIB + GIB / 2));
    let (fixture, _checks) = fixture_on_disk(
        Arc::new(|_turn, mut io| {
            Box::pin(async move {
                let Some(Input::Steer(steer)) = io.inbox.recv().await else {
                    return Outcome::failed("no steer");
                };
                tokio::time::sleep(Duration::from_millis(200)).await;
                if io.inbox.try_recv().is_ok() {
                    return Outcome::failed("told twice");
                }
                io.emit(reply("m1", &steer.text)).await;
                Outcome::done()
            })
        }),
        free,
    )
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("build it"))
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs[0].status, RunStatus::Done, "{:?}", runs[0].error);
    let items = fixture.items().await;
    let told = texts(&items)
        .into_iter()
        .map(|(text, _)| text)
        .find(|text| text.starts_with("Disk low:"))
        .expect("the agent read the steer");
    assert!(told.contains("has 7.5 GiB free"), "{told}");
    assert!(told.contains("Never delete source, uncommitted or unmerged work"));
    let shown = notices(&items);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(
        shown[0].starts_with("Disk low: 7.5 GiB free on "),
        "{shown:?}"
    );
    assert!(
        !items.iter().any(|item| matches!(
            &item.kind,
            ItemKind::UserMessage { text, .. } if text.inline().is_some_and(|text| text.starts_with("Disk low"))
        )),
        "the steer is not shown as the user's message"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_disk_stops_the_run_and_says_why() {
    let free = Arc::new(AtomicU64::new(50 * GIB));
    let (fixture, checks) = fixture_on_disk(
        Arc::new(|_turn, io| {
            Box::pin(async move {
                io.cancel.cancelled().await;
                Outcome::cancelled()
            })
        }),
        Arc::clone(&free),
    )
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("build it"))
        .await
        .unwrap();
    observed_disk(&checks).await;
    assert!(fixture.hub.is_live(&fixture.thread_id));
    free.store(2 * GIB, Ordering::Relaxed);
    let runs = fixture.settled().await;
    assert_eq!(runs[0].status, RunStatus::Failed);
    let shown = errors(&fixture.items().await);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(
        shown[0].starts_with("Stopped: only 2 GiB free on "),
        "{shown:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_sent_on_a_full_disk_can_clean_it_up() {
    let free = Arc::new(AtomicU64::new(2 * GIB));
    let disk = Arc::clone(&free);
    let (fixture, _checks) = fixture_on_disk(
        Arc::new(move |_turn, mut io| {
            let disk = Arc::clone(&disk);
            Box::pin(async move {
                let Some(Input::Steer(_)) = io.inbox.recv().await else {
                    return Outcome::failed("no steer");
                };
                disk.store(40 * GIB, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(100)).await;
                io.emit(reply("m1", "freed 38 GiB")).await;
                Outcome::done()
            })
        }),
        free,
    )
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("clear the build caches"))
        .await
        .unwrap();
    let runs = fixture.settled().await;
    assert_eq!(runs[0].status, RunStatus::Done, "{:?}", runs[0].error);
    assert!(errors(&fixture.items().await).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_sent_on_a_full_disk_stops_if_it_keeps_filling_it() {
    let free = Arc::new(AtomicU64::new(2 * GIB));
    let (fixture, checks) = fixture_on_disk(
        Arc::new(|_turn, io| {
            Box::pin(async move {
                io.cancel.cancelled().await;
                Outcome::cancelled()
            })
        }),
        Arc::clone(&free),
    )
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("build it"))
        .await
        .unwrap();
    observed_disk(&checks).await;
    assert!(fixture.hub.is_live(&fixture.thread_id));
    free.store(GIB / 2, Ordering::Relaxed);
    let runs = fixture.settled().await;
    assert_eq!(runs[0].status, RunStatus::Failed);
    let shown = errors(&fixture.items().await);
    assert!(
        shown[0].starts_with("Stopped: only 0.5 GiB free on "),
        "{shown:?}"
    );
}
