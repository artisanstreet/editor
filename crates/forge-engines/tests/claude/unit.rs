use super::ClaudeDecoder;
use artisan_api::ItemId;

#[tokio::test]
async fn generated_title_is_persisted_without_replacing_the_claude_transcript() {
    use crate::{Harness as _, Launch, Turn, UserInput};
    use artisan_api::{EngineConfig, EngineId, ThreadId};
    let dir = tempfile::tempdir().unwrap();
    let turn = Turn {
        thread_id: ThreadId::new(),
        engine: EngineId::Claude,
        binary: dir.path().join("unused-claude"),
        home: dir.path().join("home"),
        cwd: dir.path().to_owned(),
        config: EngineConfig::new(EngineId::Claude),
        session_id: Some("working-session".to_owned()),
        input: UserInput::default(),
        launch: Launch::default(),
        trace_dir: None,
    };
    let path = super::config_dir(&turn)
        .join("projects")
        .join(super::side::project_dir(&turn))
        .join("working-session.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let history = "{\"type\":\"user\",\"message\":{\"content\":\"repair the sidebar\"}}\n";
    std::fs::write(&path, history).unwrap();
    super::ClaudeHarness
        .set_title(turn, "Fix sidebar titles".to_owned())
        .await
        .unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().starts_with(history));
    assert_eq!(
        super::title_from_transcript(&path).as_deref(),
        Some("Fix sidebar titles")
    );
}

#[tokio::test]
async fn protocol_output_errors_fail_even_after_a_successful_result() {
    use crate::fake_engine;
    use artisan_api::RunStatus;
    for (after_result, malformed) in [(false, false), (true, false), (false, true), (true, true)] {
        let directory = tempfile::tempdir().unwrap();
        let after = after_result.to_string();
        let oversized = (crate::process::MAX_LINE_BYTES + 1).to_string();
        let mut env = vec![("ARTISAN_TEST_AFTER_RESULT", after.as_str())];
        if !malformed {
            env.push(("ARTISAN_TEST_LINE_BYTES", &oversized));
        }
        let mut running = super::scenario::start(super::scenario::turn(
            directory.path(),
            "claude-protocol-error",
            &env,
        ));
        let outcome = running
            .outcome(std::time::Duration::from_secs(5))
            .await
            .expect("protocol failure must not await a blocked process");
        assert_eq!(outcome.status(), RunStatus::Failed);
        assert!(outcome.error().unwrap().contains(if malformed {
            "protocol JSON"
        } else {
            "wrote a line over"
        }));
        for marker in ["parent", "child"] {
            assert!(
                fake_engine::ended(directory.path(), marker).await,
                "the owned process and its descendant stop"
            );
        }
        #[cfg(unix)]
        assert!(
            fake_engine::reaped(directory.path(), "parent"),
            "the owned process is reaped"
        );
        running.drained().await;
    }
}

#[tokio::test]
async fn input_stays_open_while_a_background_agent_works() {
    let dir = tempfile::tempdir().unwrap();
    let mut turn = super::scenario::turn(dir.path(), "claude-background-agent", &[]);
    turn.trace_dir = Some(dir.path().join("data/traces"));
    let mut running = super::scenario::start(turn);
    let outcome = running
        .outcome(std::time::Duration::from_secs(20))
        .await
        .expect("the run ends once the agent reported");
    assert_eq!(
        outcome.status(),
        artisan_api::RunStatus::Done,
        "{outcome:?}"
    );
    let traces = dir.path().join("data/traces");
    let mut trace = String::new();
    for _ in 0..200 {
        trace = std::fs::read_dir(&traces)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
            .collect();
        if trace.contains("exited") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    for expected in [
        "background_tasks_changed",
        "input stays open for background work (1 background task(s)",
        "closing stdin: result (cancelling: false; 0 background task(s)",
        "exited",
    ] {
        assert!(
            trace.contains(expected),
            "the trace lacks {expected:?}:\n{trace}"
        );
    }
}

#[tokio::test]
async fn input_stays_open_through_a_long_turn_after_an_agent_reports() {
    let dir = tempfile::tempdir().unwrap();
    let mut running =
        super::scenario::start(super::scenario::turn(dir.path(), "claude-long-turn", &[]));
    let outcome = running
        .outcome(std::time::Duration::from_secs(20))
        .await
        .expect("the run ends once the second agent reported");
    assert_eq!(
        outcome.status(),
        artisan_api::RunStatus::Done,
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_side_query_answers_from_a_fork_and_leaves_the_thread_alone() {
    use crate::Harness as _;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cwd = dir.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let project: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let projects = home.join(".claude").join("projects").join(project);
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(projects.join("s1.jsonl"), "{}\n").unwrap();
    let mut turn = super::scenario::turn(&cwd, "claude-side-fork", &[]);
    turn.home = home;
    turn.config.permissions = artisan_api::Permissions::Full;
    turn.input.text = "What next?".into();
    let reply = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        super::ClaudeHarness.side_query(turn, tokio_util::sync::CancellationToken::new()),
    )
    .await
    .expect("the fork answers")
    .expect("the side query succeeds");
    assert_eq!(reply.text, "Ship it.");
    assert_eq!(reply.usage.cached_input, 9000);
    assert_eq!(reply.usage.input, 9010);
    assert!(
        projects.join("s1.jsonl").exists(),
        "the thread's transcript stays"
    );
    assert!(
        !projects.join("f1.jsonl").exists(),
        "the fork's transcript is gone"
    );
}

#[test]
fn the_fast_speed_turns_on_fast_mode() {
    let mut config = artisan_api::EngineConfig::new(artisan_api::EngineId::Claude);
    config.model = Some("claude-opus-5-5".into());
    let settings = |config: &artisan_api::EngineConfig| {
        let args = super::launch_args(config, None, &[]);
        args.iter()
            .position(|arg| arg == "--settings")
            .map(|at| args[at + 1].clone())
    };
    assert_eq!(settings(&config), None);
    config.speed = Some("standard".into());
    assert_eq!(settings(&config).as_deref(), Some(r#"{"fastMode":false}"#));
    config.speed = Some("fast".into());
    assert_eq!(settings(&config).as_deref(), Some(r#"{"fastMode":true}"#));
}

#[test]
fn compaction_window_keeps_model_identity() {
    let mut config = artisan_api::EngineConfig::new(artisan_api::EngineId::Claude);
    config.model = Some("claude-opus-test".to_owned());
    assert_eq!(super::context_window_env(&config), None);
    config
        .options
        .insert("compaction_window".to_owned(), serde_json::json!(400_000));
    assert_eq!(
        super::context_window_env(&config),
        Some(("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "400000".to_owned()))
    );
    let args = super::launch_args(&config, None, &[]);
    let model = args.iter().position(|arg| arg == "--model").unwrap();
    assert_eq!(args[model + 1], "claude-opus-test");
}

#[tokio::test]
async fn ultra_commands_reach_claude_as_keywords_of_their_turn() {
    for (typed, sent) in [
        ("/ultrathink audit this", "ultrathink\naudit this"),
        ("/ultracode ship the parser", "ultracode\nship the parser"),
        ("ship the parser", "ship the parser"),
    ] {
        let input = crate::UserInput {
            text: typed.to_owned(),
            ..crate::UserInput::default()
        };
        let line = super::user_line(&input).await.unwrap();
        assert_eq!(line["message"]["content"][0]["text"], sent);
    }
}

#[test]
fn permissions_pick_auto_mode_or_skip_prompts() {
    let mut config = artisan_api::EngineConfig::new(artisan_api::EngineId::Claude);
    let args = super::launch_args(&config, None, &[]);
    let mode = args.iter().position(|arg| arg == "--permission-mode");
    assert_eq!(mode.map(|at| args[at + 1].as_str()), Some("auto"));
    assert!(
        !args
            .iter()
            .any(|arg| arg == "--dangerously-skip-permissions")
    );

    config.permissions = artisan_api::Permissions::Full;
    let args = super::launch_args(&config, None, &[]);
    assert!(
        args.iter()
            .any(|arg| arg == "--dangerously-skip-permissions")
    );
    assert!(!args.iter().any(|arg| arg == "--permission-mode"));
}

#[test]
fn transcripts() {
    let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/transcripts/claude");
    let mut files: Vec<_> = std::fs::read_dir(directory)
        .expect("transcripts directory")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect();
    files.sort();
    assert!(!files.is_empty());
    for path in files {
        let name = path
            .file_stem()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        let text = crate::transcript_tests::read_path(&path);
        let mut decoder = ClaudeDecoder::default();
        let mut events = Vec::new();
        let mut user_lines = 0;
        if text.contains("\"artisan-title\"") {
            decoder.title_request("first message");
        }
        let mut protocol_errors = Vec::new();
        for line in text.lines() {
            if line.contains("\"isReplay\":true") {
                user_lines += 1;
                decoder.expect_replay(
                    (user_lines > 1).then(|| ItemId::from(format!("steer-{user_lines}"))),
                );
            }
            match decoder.feed(line) {
                Ok(line_events) => events.extend(line_events),
                Err(error) => protocol_errors.push(diagnostics::chain(&error)),
            }
        }
        events.extend(decoder.finish());
        let outcome = decoder.take_outcome();
        if protocol_errors.is_empty() {
            insta::assert_debug_snapshot!(name, (events, outcome));
        } else {
            insta::assert_debug_snapshot!(name, (events, outcome, protocol_errors));
        }
    }
}

fn turn_end(case: &str) -> crate::Outcome {
    let text = crate::transcript_tests::read("claude", case);
    let mut decoder = ClaudeDecoder::default();
    for line in text.lines() {
        decoder.feed(line).unwrap();
    }
    decoder.take_outcome().expect("the turn ends")
}

#[test]
fn a_turn_the_usage_limit_rejected_waits_for_the_reset_it_named() {
    let outcome = turn_end("usage-limit");
    assert_eq!(outcome.status(), artisan_api::RunStatus::Failed);
    assert_eq!(
        outcome.retry(),
        Some(crate::Retry::UsageLimit {
            resets_at: Some(1_791_931_800_000)
        })
    );
}

#[test]
fn an_overloaded_turn_can_be_retried() {
    let outcome = turn_end("overloaded");
    assert_eq!(outcome.status(), artisan_api::RunStatus::Failed);
    assert_eq!(outcome.retry(), Some(crate::Retry::Overloaded));
}

#[test]
fn a_command_moved_to_the_background_runs_until_its_notification() {
    use artisan_api::Status::{Done, Running};
    let text = crate::transcript_tests::read("claude", "command-moved-to-background");
    let mut decoder = ClaudeDecoder::default();
    let mut states = Vec::new();
    for line in text.lines() {
        for event in decoder.feed(line).unwrap() {
            if let crate::EngineEvent::Item {
                status,
                kind:
                    artisan_api::ItemKind::Tool {
                        tool: artisan_api::ToolKind::Shell { background, .. },
                        output,
                        ..
                    },
                ..
            } = event
            {
                states.push((status, background, output.is_some()));
            }
        }
    }
    assert_eq!(
        states,
        [
            (Running, false, false),
            (Running, true, false),
            (Running, true, true),
            (Done, true, true),
        ],
    );
}

#[test]
fn an_agent_waiting_on_its_own_shell_keeps_the_session_open_and_reports_at_the_end() {
    let text = crate::transcript_tests::read("claude", "agent-waits-on-its-shell");
    let mut decoder = ClaudeDecoder::default();
    let mut results = Vec::new();
    let mut open_while_waiting = true;
    for line in text.lines() {
        for event in decoder.feed(line).unwrap() {
            if let crate::EngineEvent::Item {
                status,
                kind: artisan_api::ItemKind::Agent { result, .. },
                ..
            } = event
            {
                results.push((status, result));
            }
        }
        if line.contains("\"local_bash\"") && line.contains("background_tasks_changed") {
            open_while_waiting &= decoder.background_running();
        }
    }
    assert!(
        open_while_waiting,
        "the session keeps taking input while the agent's shell runs"
    );
    assert_eq!(
        results.last(),
        Some(&(artisan_api::Status::Done, Some("FINISHED".into()))),
        "the agent's report is what it said once resumed: {results:?}"
    );
    assert!(
        results
            .iter()
            .all(|(status, _)| *status == artisan_api::Status::Running
                || results.last().is_some_and(|(last, _)| last == status)),
        "the agent never shows finished while it waits: {results:?}"
    );
}
