use super::*;
use artisan_api::{EngineId, RunOutcome};

fn antigravity() -> EngineStatus {
    EngineStatus {
        discovery: artisan_api::DiscoveryState::Fresh { observed_at: 0 },
        engine: EngineId::Antigravity,
        install: artisan_api::InstallState::Installed {
            version: "1.5.14".into(),
            update: None,
        },
        sign_in: artisan_api::SignInState::SignedIn { account: None },
        models: std::sync::Arc::from([
            artisan_api::Model {
                id: "gemini-4-neon".into(),
                name: "Gemini 4 Neon".into(),
                ..artisan_api::Model::default()
            },
            artisan_api::Model {
                id: "claude-opus-5-5".into(),
                name: "Claude Opus 5.5".into(),
                ..artisan_api::Model::default()
            },
        ]),
        default_model: Some("gemini-4-neon".into()),
        commands: std::sync::Arc::from([]),
        usage: None,
        integrity: None,
        versions: std::sync::Arc::from([]),
        claude_ai: None,
    }
}

fn run(context_used: u64, state: RunState) -> Run {
    Run {
        config: None,
        id: artisan_api::RunId("run".into()),
        thread_id: artisan_api::ThreadId("thread".into()),
        engine: EngineId::Antigravity,
        model: Some("gemini-4-neon".into()),
        input_cutoff: 0,
        state,
        usage: artisan_api::Usage {
            context_used: Some(context_used),
            ..artisan_api::Usage::default()
        },
        session_id: Some("session".into()),
        seq: 1,
        started_at: 1,
        continuation: None,
    }
}

fn done() -> RunState {
    RunState::Finished {
        outcome: RunOutcome::Done,
        error: None,
        ended_at: 2,
    }
}

fn config(engine: EngineId, model: &str) -> EngineConfig {
    EngineConfig {
        model: Some(model.into()),
        ..EngineConfig::new(engine)
    }
}

#[test]
fn switching_models_on_a_large_session_warns_before_the_uncached_reread() {
    let status = antigravity();
    let opus = config(EngineId::Antigravity, "claude-opus-5-5");
    assert_eq!(
        prompt(&run(461_495, done()), Some(&status), opus.clone()),
        Some(SwitchPrompt {
            config: opus.clone(),
            from: "Gemini 4 Neon".into(),
            to: "Claude Opus 5.5".into(),
            context_tokens: 461_495,
            compaction: Compaction::Available,
        })
    );
    assert_eq!(
        prompt(
            &run(461_495, RunState::Running),
            Some(&status),
            opus.clone()
        )
        .map(|prompt| prompt.compaction),
        Some(Compaction::BusyRun)
    );
    assert_eq!(
        prompt(&run(40_000, done()), Some(&status), opus),
        None,
        "a small context re-reads cheaply"
    );
    assert_eq!(
        prompt(
            &run(461_495, done()),
            Some(&status),
            config(EngineId::Antigravity, "gemini-4-neon")
        ),
        None,
        "the same model keeps its cache"
    );
    assert_eq!(
        prompt(
            &run(461_495, done()),
            Some(&status),
            config(EngineId::Claude, "claude-opus-5-5")
        ),
        None,
        "an engine switch hands off a summary instead of the history"
    );
}
