use super::{Phase, section_title, status_copy, turn_phase};
use artisan_api::{
    EngineId, Item, ItemId, ItemKind, LiveTarget, Run, RunId, RunOutcome, RunState, Status,
    ThreadId, ToolKind, Usage,
};
use std::collections::HashMap;

fn run() -> Run {
    Run {
        config: None,
        id: RunId("run".into()),
        thread_id: ThreadId("thread".into()),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        state: RunState::Running,
        usage: Usage::default(),
        session_id: None,
        seq: 1,
        started_at: 0,
        continuation: None,
    }
}

fn item(position: u64, status: Status, kind: ItemKind) -> Item {
    Item {
        id: ItemId(format!("item-{position}")),
        thread_id: ThreadId("thread".into()),
        position,
        seq: position,
        run_id: Some(RunId("run".into())),
        parent_id: None,
        status,
        created_at: 0,
        updated_at: 0,
        kind,
    }
}

fn shell(position: u64, status: Status, background: bool) -> Item {
    item(
        position,
        status,
        ItemKind::Tool {
            tool: ToolKind::Shell {
                command: "tail -f build.log".into(),
                cwd: None,
                background,
                watch: background,
            },
            title: None,
            output: None,
            exit_code: None,
            approval: None,
        },
    )
}

fn reply(position: u64) -> Item {
    item(
        position,
        Status::Done,
        ItemKind::AssistantMessage {
            text: "Built.".into(),
            phase: artisan_api::Phase::Final,
        },
    )
}

#[test]
fn a_job_running_behind_the_turn_is_not_read_as_a_wait() {
    let live: HashMap<LiveTarget, crate::conversation::thread::LiveText> = HashMap::new();
    let job = shell(1, Status::Running, true);
    let answer = reply(2);
    assert_eq!(
        turn_phase(&run(), &[&job, &answer], &live),
        Phase::Thinking,
        "only Forge knows whether the engine is between turns; the Editor does not guess a wait"
    );
    let command = shell(3, Status::Running, false);
    assert_eq!(
        turn_phase(&run(), &[&job, &answer, &command], &live),
        Phase::Working,
        "a foreground command is work whatever runs behind it"
    );
}

#[test]
fn a_stopped_run_reads_as_stopped_even_while_its_command_still_runs() {
    let live: HashMap<LiveTarget, crate::conversation::thread::LiveText> = HashMap::new();
    let mut stopped = run();
    stopped.state = RunState::Finished {
        outcome: RunOutcome::Cancelled,
        error: None,
        ended_at: 3_000,
    };
    let command = shell(1, Status::Running, false);
    let phase = turn_phase(&stopped, &[&command], &live);
    assert_eq!(phase, Phase::Settled(RunOutcome::Cancelled));
    assert_eq!(section_title(phase, true, 3_000), "Stopped after 3s");
    assert_eq!(status_copy(phase), None);
}
