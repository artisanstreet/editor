use super::door::{Door, Ending, Session};

const IDLE: Session = Session {
    in_turn: false,
    awaiting_input: false,
    background: false,
};
const BACKGROUND: Session = Session {
    background: true,
    ..IDLE
};
const IN_TURN: Session = Session {
    in_turn: true,
    ..IDLE
};

#[test]
fn a_plain_reply_closes_input_at_once() {
    assert_eq!(
        Door::Open.after_frame(IN_TURN),
        Door::Open,
        "input stays open while the turn runs"
    );
    assert_eq!(
        Door::Open.after_result(IDLE, Ending::Replied),
        Door::Closed,
        "nothing is on its way after the reply"
    );
    assert!(
        !Door::Closed.after_frame(BACKGROUND).takes_input(),
        "closed input never reopens"
    );
}

#[test]
fn background_work_keeps_input_open_so_agents_live_and_steers_land() {
    let waiting = Door::Open.after_result(BACKGROUND, Ending::Replied);
    assert_eq!(
        waiting,
        Door::HeldForBackground,
        "a monitor, shell or agent outlives the reply"
    );
    assert!(waiting.takes_input(), "a steer can still be written");
    assert_eq!(
        waiting.after_frame(Session {
            in_turn: true,
            ..BACKGROUND
        }),
        Door::HeldForBackground,
        "the steer's own turn runs with the job still alive"
    );
    assert_eq!(
        waiting.after_result(BACKGROUND, Ending::Replied),
        Door::HeldForBackground,
        "the steer's reply does not end the wait"
    );
}

#[test]
fn the_last_job_ending_gives_its_report_time_to_start_a_turn() {
    let settling = Door::HeldForBackground.after_frame(IDLE);
    assert_eq!(settling, Door::Settling, "input stays open for the report");
    assert_eq!(
        settling.after_frame(IN_TURN),
        Door::Open,
        "the report's turn decides at its own reply"
    );
    assert_eq!(
        settling.after_frame(BACKGROUND),
        Door::HeldForBackground,
        "new work cancels the countdown"
    );
    assert_eq!(
        settling.after_grace(IDLE),
        Door::Closed,
        "no report came: the session is over"
    );
}

#[test]
fn the_countdown_never_closes_input_under_a_running_turn() {
    let long_turn = Door::Settling.after_frame(IN_TURN);
    assert_eq!(
        long_turn.after_grace(IN_TURN),
        Door::Open,
        "a turn that outlasts the countdown may still start an agent"
    );
    assert_eq!(
        long_turn.after_result(BACKGROUND, Ending::Replied),
        Door::HeldForBackground,
        "the agent that turn started keeps input open"
    );
    assert_eq!(
        Door::HeldForBackground.after_frame(IN_TURN),
        Door::Open,
        "work ending mid-turn leaves the decision to that turn's reply"
    );
}

#[test]
fn a_reply_waits_for_a_steer_or_title_already_on_its_way() {
    let pending = Session {
        awaiting_input: true,
        ..IDLE
    };
    let held = Door::Open.after_result(pending, Ending::Replied);
    assert_eq!(held, Door::HeldForReply, "the steer was already written");
    assert_eq!(
        held.after_frame(Session {
            awaiting_input: true,
            ..IDLE
        }),
        Door::HeldForReply,
        "still unanswered"
    );
    assert_eq!(
        held.after_frame(IN_TURN),
        Door::Open,
        "the steer started a turn"
    );
    assert_eq!(
        held.after_frame(IDLE),
        Door::Closed,
        "a title answer starts no turn, so nothing else would close input"
    );
    assert_eq!(
        Door::Settling.after_grace(Session {
            awaiting_input: true,
            ..IDLE
        }),
        Door::HeldForReply,
        "a steer written during the countdown is waited for"
    );
}

#[test]
fn a_cancelled_turn_closes_input_whatever_is_running() {
    assert_eq!(
        Door::HeldForBackground.after_result(
            Session {
                background: true,
                awaiting_input: true,
                ..IDLE
            },
            Ending::Cancelled
        ),
        Door::Closed,
        "the interrupt's result ends the session"
    );
}
