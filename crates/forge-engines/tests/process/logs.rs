use super::*;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn engine_stderr_stays_out_of_the_logs() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(captured.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    let home = tempfile::tempdir().unwrap();
    let turn = Turn {
        thread_id: artisan_api::ThreadId::from("stderr-logs"),
        engine: artisan_api::EngineId::Codex,
        binary: "/bin/sh".into(),
        home: home.path().into(),
        cwd: home.path().into(),
        config: artisan_api::EngineConfig::new(artisan_api::EngineId::Codex),
        session_id: None,
        input: crate::UserInput::default(),
        launch: crate::Launch::default(),
        trace_dir: None,
    };
    let mut process = spawn(
        &turn,
        &[
            "-c".into(),
            "echo 'Authorization: Bearer engine-secret' >&2".into(),
        ],
    )
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), process.wait())
        .await
        .expect("the engine exits within the bound");
    if let Some(stderr) = process.stderr_task.take() {
        stderr.await.unwrap();
    }
    assert!(process.stderr_tail().contains("engine-secret"));
    let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains("stderr"),
        "the stderr read is still logged as metadata: {logged}"
    );
    assert!(
        !logged.contains("engine-secret"),
        "what the engine wrote to stderr reached the logs: {logged}"
    );
}
