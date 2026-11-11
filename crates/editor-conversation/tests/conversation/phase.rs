use super::{Phase, turn_phase};
use artisan_api::{
    EngineId, Item, ItemId, ItemKind, LiveTarget, Run, RunId, RunStatus, Status, ThreadId,
    ToolKind, Usage,
};
use std::collections::HashMap;
use std::sync::Arc;

fn run() -> Run {
    Run {
        config: None,
        id: RunId("run".into()),
        thread_id: ThreadId("thread".into()),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        status: RunStatus::Running,
        usage: Usage::default(),
        session_id: None,
        error: None,
        seq: 1,
        started_at: 0,
        ended_at: None,
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
    let live: HashMap<LiveTarget, Arc<ropey::Rope>> = HashMap::new();
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
