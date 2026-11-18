use super::{Fake, Live, live};
use crate::{
    ComputerError,
    desktop::Input,
    runner::{Halted, RunError, StepError},
};
use forge_mcp::ToolSet;
use serde_json::json;
use std::time::Duration;

const WAIT_THEN_CLICK: &str = r#"{"steps":[{"move":[1,1]},{"wait":{"ms":30000}},{"click":[5,5]}]}"#;

async fn start_waiting_batch(
    live: &Live,
) -> tokio::task::JoinHandle<Result<Vec<forge_mcp::Content>, ComputerError>> {
    live.store
        .set_setting("computer_use".into(), json!(true))
        .await
        .unwrap();
    live.set
        .call(&live.thread, "screenshot", json!({}))
        .await
        .unwrap();
    let set = live.set.clone();
    let thread = live.thread.clone();
    let batch = tokio::spawn(async move {
        set.call(
            &thread,
            "input",
            serde_json::from_str(WAIT_THEN_CLICK).unwrap(),
        )
        .await
    });
    tokio::time::timeout(
        Duration::from_secs(10),
        live.desktop.sent.subscribe().wait_for(|sent| *sent >= 1),
    )
    .await
    .unwrap()
    .unwrap();
    batch
}

async fn halted_before_the_click(
    live: &Live,
    batch: tokio::task::JoinHandle<Result<Vec<forge_mcp::Content>, ComputerError>>,
) -> Halted {
    let error = tokio::time::timeout(Duration::from_secs(10), batch)
        .await
        .expect("the batch kept waiting after it was halted")
        .unwrap()
        .unwrap_err();
    let ComputerError::Run(RunError::Stopped {
        completed: 1,
        source: StepError::Halted(why),
    }) = error
    else {
        panic!("not halted after the move: {error:?}");
    };
    assert!(
        !live
            .desktop
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|input| matches!(input, Input::Button { .. })),
        "a click reached the desktop after the halt"
    );
    why
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_the_run_stops_its_batch_before_the_next_click() {
    let live = live(Fake::default()).await;
    let batch = start_waiting_batch(&live).await;
    live.hub.cancel(&live.thread).await.unwrap();
    assert_eq!(
        halted_before_the_click(&live, batch).await,
        Halted::RunStopped
    );
    let idle = artisan_api::ThreadId::new();
    let refused = live
        .set
        .call(
            &idle,
            "input",
            serde_json::from_str(WAIT_THEN_CLICK).unwrap(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(refused, ComputerError::Halted(Halted::RunStopped)),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn turning_computer_use_off_stops_a_running_batch_before_the_next_click() {
    let live = live(Fake::default()).await;
    let batch = start_waiting_batch(&live).await;
    live.store
        .set_setting("computer_use".into(), json!(false))
        .await
        .unwrap();
    assert_eq!(
        halted_before_the_click(&live, batch).await,
        Halted::ComputerUseOff
    );
}
