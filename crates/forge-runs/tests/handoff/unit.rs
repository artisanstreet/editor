use super::*;
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};

#[test]
fn overflow_retains_the_request_and_latest_evidence_at_utf8_boundaries() {
    let transcript = Transcript {
        entries: vec![
            Entry {
                user: true,
                text: "[User]: finish the parser\n".into(),
            },
            Entry {
                user: false,
                text: "🦀".repeat(20_000),
            },
            Entry {
                user: false,
                text: "[Tool result]: newest tests passed\n".into(),
            },
        ],
        omitted: false,
    };
    let text = serialized(&transcript, 2000);
    assert!(text.len() <= 2000);
    assert!(text.starts_with("[User]: finish the parser"));
    assert!(text.ends_with("[Tool result]: newest tests passed\n"));
    assert!(text.contains("[entry truncated]"));
    assert!(recent(&transcript.entries).is_empty());
}

struct Rejecting {
    prompts: Arc<Mutex<Vec<usize>>>,
    rejection: fn(&forge_engines::Turn) -> SideQueryError,
}

impl forge_engines::Harness for Rejecting {
    fn run(
        &self,
        _turn: forge_engines::Turn,
        _io: forge_engines::RunIo,
    ) -> BoxFuture<'static, forge_engines::Outcome> {
        Box::pin(async { forge_engines::Outcome::done() })
    }

    fn compact(
        &self,
        turn: forge_engines::Turn,
        _cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<forge_engines::SideReply, SideQueryError>> {
        self.prompts.lock().unwrap().push(turn.input.text.len());
        let rejection = (self.rejection)(&turn);
        Box::pin(async move { Err(rejection) })
    }
}

struct Summarizing {
    prompts: Arc<Mutex<Vec<usize>>>,
}

impl forge_engines::Harness for Summarizing {
    fn run(
        &self,
        _turn: forge_engines::Turn,
        _io: forge_engines::RunIo,
    ) -> BoxFuture<'static, forge_engines::Outcome> {
        Box::pin(async { forge_engines::Outcome::done() })
    }

    fn compact(
        &self,
        turn: forge_engines::Turn,
        _cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<forge_engines::SideReply, SideQueryError>> {
        self.prompts.lock().unwrap().push(turn.input.text.len());
        Box::pin(async {
            Ok(forge_engines::SideReply {
                text: "## Objective\n- finish the parser".to_owned(),
                usage: forge_engines::SideUsage::default(),
            })
        })
    }
}

fn install(data: &std::path::Path, engine: EngineId) {
    let engine_dir = data.join("engines").join(engine.to_string());
    let entry = engine_dir
        .join("versions")
        .join("1.0.0")
        .join(forge_engines::host_entry(engine).unwrap());
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(entry, "").unwrap();
    std::fs::write(engine_dir.join("current"), "1.0.0").unwrap();
}

async fn summarized_against(
    rejection: fn(&forge_engines::Turn) -> SideQueryError,
) -> (Result<String, ContextError>, Vec<usize>) {
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let harness: Arc<dyn forge_engines::Harness> = Arc::new(Rejecting {
        prompts: Arc::clone(&prompts),
        rejection,
    });
    let (summary, _dir) =
        summarized_on(Arc::new(move |_| Arc::clone(&harness)), EngineId::Codex).await;
    let prompts = prompts.lock().unwrap().clone();
    (summary, prompts)
}

async fn summarized_on(
    harness_for: crate::HarnessFor,
    target: EngineId,
) -> (Result<String, ContextError>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_path_buf();
    install(&data, EngineId::Codex);
    install(&data, target);
    let hub = Hub::build(
        forge_store::Store::open(&data.join("forge")).unwrap(),
        forge_engines::Engines::new(&data),
        data.clone(),
        harness_for,
        crate::Surroundings {
            disk: crate::disk::Probe::system(),
            stop_deadline: crate::STOP_DEADLINE,
        },
        None,
    );
    let project = hub
        .store()
        .create_project(artisan_api::CreateProject {
            id: artisan_api::ProjectId::new(),
            path: data.display().to_string(),
            name: None,
        })
        .await
        .unwrap();
    let thread = hub
        .store()
        .create_thread(artisan_api::CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Codex),
            title: None,
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    let transcript = Transcript {
        entries: (0..40)
            .flat_map(|exchange| {
                [
                    Entry {
                        user: true,
                        text: format!("[User]: step {exchange}\n"),
                    },
                    Entry {
                        user: false,
                        text: "a".repeat(30_000),
                    },
                ]
            })
            .collect(),
        omitted: false,
    };
    let mut switched = thread.clone();
    switched.config = EngineConfig::new(target);
    let summary = summarize_or_fall_back(
        &hub,
        &Source {
            thread,
            session: None,
        },
        switched,
        &transcript,
        true,
        CancellationToken::new(),
    )
    .await;
    (summary, dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_away_from_an_engine_without_side_queries_summarizes_on_the_new_engine() {
    let summarized = Arc::new(Mutex::new(Vec::new()));
    let unsupported: Arc<dyn forge_engines::Harness> = Arc::new(Rejecting {
        prompts: Arc::new(Mutex::new(Vec::new())),
        rejection: |turn| SideQueryError::Unsupported {
            engine: turn.engine,
        },
    });
    let summarizing: Arc<dyn forge_engines::Harness> = Arc::new(Summarizing {
        prompts: Arc::clone(&summarized),
    });
    let (summary, _dir) = summarized_on(
        Arc::new(move |engine| match engine {
            EngineId::Claude => Arc::clone(&summarizing),
            EngineId::Codex | EngineId::OpenCode | EngineId::Antigravity => {
                Arc::clone(&unsupported)
            }
        }),
        EngineId::Claude,
    )
    .await;
    assert_eq!(summary.unwrap(), "## Objective\n- finish the parser");
    assert_eq!(summarized.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_context_limit_retries_the_summary_with_less_history() {
    let (summary, prompts) = summarized_against(|turn| SideQueryError::Codex {
        thread: turn.thread_id.clone(),
        source: Box::new(forge_engines::codex::SideQueryError::ContextLimit {
            fork: "fork".to_owned(),
            message: "Codex ran out of room in the model's context window.".to_owned(),
        }),
    })
    .await;
    assert!(
        matches!(summary, Err(ContextError::Exhausted { .. })),
        "{summary:?}"
    );
    assert_eq!(prompts.len(), 4, "{prompts:?}");
    assert!(prompts.is_sorted_by(|a, b| a > b), "{prompts:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unrelated_failure_mentioning_context_fails_without_shrinking_the_history() {
    let (summary, prompts) = summarized_against(|turn| SideQueryError::Codex {
        thread: turn.thread_id.clone(),
        source: Box::new(forge_engines::codex::SideQueryError::Failed {
            fork: "fork".to_owned(),
            message: "unexpected status 400 Bad Request: Unsupported parameter: 'context_management' is not supported with this model.".to_owned(),
        }),
    })
    .await;
    assert!(
        matches!(summary, Err(ContextError::Summarize { .. })),
        "{summary:?}"
    );
    assert_eq!(prompts.len(), 1, "{prompts:?}");
}
