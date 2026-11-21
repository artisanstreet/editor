use super::*;
use crate::engines::host_spec;
use crate::install::{self, Layout};

fn claude_that_cannot_report_usage() -> (tempfile::TempDir, Engines) {
    let directory = tempfile::tempdir().unwrap();
    let layout = Layout::new(directory.path());
    let engine = EngineId::Claude;
    std::fs::create_dir_all(layout.home(engine)).unwrap();
    let binary = layout
        .version_dir(engine, "1.0.0")
        .join(host_spec(engine).artifact.unwrap().entry);
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::write(&binary, "").unwrap();
    install::switch(&layout, engine, "1.0.0").unwrap();
    let engines = Engines::new(directory.path());
    (directory, engines)
}

async fn next_status(
    changes: &mut broadcast::Receiver<crate::EngineChange>,
) -> tokio::time::Instant {
    changes.recv().await.unwrap();
    tokio::time::Instant::now()
}

#[tokio::test(start_paused = true)]
async fn a_failed_usage_read_is_retried_a_minute_later_without_waking_in_between() {
    let (_directory, engines) = claude_that_cannot_report_usage();
    let mut changes = engines.subscribe();
    let refresher = engines.clone();
    let refreshing = tokio::spawn(async move { refresher.keep_usage_fresh().await });

    let first = next_status(&mut changes).await;
    let retried = next_status(&mut changes).await;
    let again = next_status(&mut changes).await;

    assert_eq!(retried - first, Duration::from_mins(1));
    assert_eq!(again - retried, Duration::from_mins(1));
    refreshing.abort();
}
