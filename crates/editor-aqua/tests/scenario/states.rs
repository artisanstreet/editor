use artisan_api::EngineId;
use test_support::scenario::{Key, Modifiers, NamedKey, Opening, Role, Scenario};

#[test]
fn a_thread_forge_does_not_have_says_so_without_an_empty_state_or_a_retry() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("missing"));
    let settled = editor.settle("the missing thread", |view| {
        view.shows("This thread isn't on this Forge.")
    });
    assert!(
        !settled.contains("No messages yet."),
        "a missing thread isn't shown as an empty one:\n{settled}"
    );
    assert!(
        !settled.contains("Try again"),
        "nothing can bring a missing thread back:\n{settled}"
    );
    insta::assert_snapshot!(settled);
}

#[test]
fn a_message_sent_while_forge_is_down_stays_in_the_composer_with_a_retry() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    editor.shows("No messages yet.");
    editor.stop_forge();
    editor.click(Role::TextInput, "Message");
    editor.type_text("Say hello");
    editor.press(Key::Named(NamedKey::Enter), Modifiers::NONE);
    let settled = editor.settle("the failure to show", |view| {
        view.shows("Couldn't") && view.find(Role::Button, "Try again").is_some()
    });
    insta::assert_snapshot!(settled);
    assert_eq!(editor.engine().launches(), 0, "nothing reached the engine");
}

#[test]
fn a_thread_tells_assistive_tech_when_forge_goes_offline() {
    let mut editor = Scenario::replaying(EngineId::Claude, "reply")
        .thread("thread-a", "Greeting")
        .open(Opening::Thread("thread-a"));
    editor.shows("No messages yet.");
    assert!(
        editor.view(|view| !view.shows("Lost the connection to Forge")),
        "a live Forge is not reported as lost"
    );
    editor.stop_forge();
    insta::assert_snapshot!(editor.settle("the offline status", |view| {
        view.shows("Lost the connection to Forge")
    }));
}

#[test]
fn a_transcript_page_forge_fails_to_send_shows_one_problem_in_the_thread_with_a_retry() {
    const HEADLINE: &str = "Couldn't load earlier messages. Forge couldn't be reached.";
    let mut scenario =
        Scenario::replaying(EngineId::Claude, "reply").thread("thread-a", "Greeting");
    for turn in 1..=150 {
        scenario = scenario.exchange(
            "thread-a",
            &format!("Question {turn}"),
            &format!("Answer {turn}"),
        );
    }
    let mut editor = scenario.open(Opening::Thread("thread-a"));
    editor.shows("Answer 150");
    editor.stop_forge();
    for _ in 0..80 {
        if editor.view(|view| view.shows(HEADLINE)) {
            break;
        }
        editor.wheel(Role::List, "Answer", -400.0);
    }
    let settled = editor.settle("the earlier page to fail", |view| {
        view.shows(HEADLINE) && view.find(Role::Button, "Try again").is_some()
    });
    for raw in [
        "reach Forge",
        "error sending request",
        "https://",
        "/transcript",
    ] {
        assert!(
            !settled.contains(raw),
            "the technical chain is not painted ({raw}):\n{settled}"
        );
    }
    assert_eq!(
        settled.matches(HEADLINE).count(),
        1,
        "the failure shows once:\n{settled}"
    );
    let problem = editor
        .view(|view| {
            view.find(Role::Label, HEADLINE)
                .and_then(|found| found.bounds)
        })
        .expect("the problem headline is laid out");
    let transcript = editor
        .view(|view| {
            view.find_showing(Role::List, "Answer")
                .and_then(|found| found.bounds)
        })
        .expect("the transcript is laid out");
    let composer = editor
        .view(|view| {
            view.find(Role::TextInput, "Message")
                .and_then(|found| found.bounds)
        })
        .expect("the composer is laid out");
    assert!(
        editor.view(|view| view.part_showing(Role::List, HEADLINE).is_none()),
        "the problem is not painted into the transcript's rows"
    );
    assert!(
        problem.x0 >= transcript.x0 && problem.x1 <= transcript.x1,
        "the problem stays in the thread's column: {problem:?} in {transcript:?}"
    );
    assert!(
        problem.y1 <= composer.y0
            && problem.y0 > transcript.y0 + (transcript.y1 - transcript.y0) / 2.0,
        "the problem sits in the thread's flow just above the composer, not over the top of the transcript: {problem:?} above {composer:?}"
    );
    insta::assert_snapshot!(
        settled
            .lines()
            .skip_while(|line| !line.contains(HEADLINE))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
