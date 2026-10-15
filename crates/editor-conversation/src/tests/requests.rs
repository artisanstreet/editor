//! Questions, approvals and goals.

use super::*;

/// Asks one question with two options, then stores the answer it gets.
struct Asker;

impl Harness for Asker {
    fn run(&self, _turn: Turn, mut io: RunIo) -> BoxFuture<'static, Outcome> {
        Box::pin(async move {
            let question = |answers| ItemKind::Question {
                blocking: true,
                questions: vec![Question {
                    id: "colour".into(),
                    header: Some("Colour".into()),
                    text: "Which colour?".into(),
                    multi_select: false,
                    options: ["Red", "Blue"]
                        .into_iter()
                        .enumerate()
                        .map(|(index, label)| QuestionOption {
                            id: format!("option-{index}"),
                            label: label.into(),
                            description: None,
                        })
                        .collect::<Vec<_>>()
                        .into(),
                }]
                .into(),
                answer_text: None,
                answers,
            };
            io.emit(EngineEvent::Item {
                key: "ask".into(),
                parent: None,
                status: Status::AwaitingUser,
                kind: question(None),
            })
            .await;
            let Some(forge_engines::Input::Answer {
                answer: Answer::Question { answers },
                ..
            }) = io.inbox.recv().await
            else {
                return Outcome::failed("no answer");
            };
            io.emit(EngineEvent::Item {
                key: "ask".into(),
                parent: None,
                status: Status::Done,
                kind: question(Some(answers.into())),
            })
            .await;
            Outcome::done()
        })
    }
}

#[gpui::test]
fn a_question_is_answered_in_your_own_words_from_the_composer(cx: &mut TestAppContext) {
    let forge = Forge::with_engine(Arc::new(Asker));
    let thread = forge.thread();
    let dir = tempfile::tempdir().unwrap();
    cx.update(editor_ui::text_editor::TextEditor::bind_actions);
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    let client = cx.read(|cx| core.read(cx).host(&host).unwrap().read(cx).client.clone());
    let message = SendMessage {
        draft: None,
        id: ItemId::new(),
        text: "pick a colour".into(),
        images: Vec::new(),
        config: None,
    };
    editor_core::runtime::runtime()
        .block_on(client.send_message(&thread, &message))
        .unwrap();

    let asked = |cx: &mut VisualTestContext| {
        loaded(&conversation, cx)
            .into_iter()
            .find(|item| matches!(item.kind, ItemKind::Question { .. }))
    };
    wait(cx, "the question", |cx| {
        asked(cx).is_some_and(|item| item.status == Status::AwaitingUser)
    });
    let question = asked(cx).unwrap().id;
    wait(cx, "the shared questionnaire summary", |cx| {
        draw(cx);
        cx.debug_bounds("composer-questionnaire").is_some()
    });
    assert!(
        cx.debug_bounds("question-status").is_some(),
        "the transcript points to the composer: \"Answer it above the message box.\""
    );
    assert!(
        cx.debug_bounds("composer-questionnaire").is_some(),
        "the composer asks the question above the message box"
    );
    let composer = cx.read(|cx| conversation.read(cx).composer.clone().unwrap());
    cx.update(|window, cx| {
        let focus = composer.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
    cx.simulate_input("Green, please");
    cx.simulate_keystrokes("enter");

    let stored = || {
        forge
            .runtime
            .block_on(forge.store.item(&question))
            .unwrap()
            .unwrap()
    };
    wait(cx, "Forge to record the answer", |_| {
        stored().status == Status::Done
    });
    let ItemKind::Question { answers, .. } = stored().kind else {
        unreachable!()
    };
    assert_eq!(
        answers,
        Some(
            vec![Some(artisan_api::QuestionAnswer::Text {
                text: "Green, please".to_owned()
            })]
            .into()
        )
    );
}

/// Goal lifecycle belongs to one thread's composer, never its transcript.
#[gpui::test]
fn goals_are_thread_scoped_and_not_transcript_announcements(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let first = forge.thread();
    let second = forge.thread();
    let notice = ItemId::new();
    forge.put(
        &first,
        &notice,
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::Goal {
                state: "paused".into(),
            },
            text: "Finish the work in this thread only.".into(),
        },
    );
    forge.put(
        &first,
        &ItemId::new(),
        Status::Done,
        reply("A normal reply."),
    );
    forge.put(
        &second,
        &ItemId::new(),
        Status::Done,
        reply("A different thread."),
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &first, cx);
    wait(cx, "the first thread goal strip", |cx| {
        draw(cx);
        cx.debug_bounds("composer-goal-label").is_some()
    });
    cx.read(|cx| {
        assert!(
            !conversation
                .read(cx)
                .rows
                .iter()
                .any(|(row, _)| row.anchor() == Some(&notice)),
            "a nonempty goal lifecycle notice must not render in the transcript"
        );
    });
    core.update(cx, |core, cx| {
        core.navigate(
            Screen::NewThread {
                host: host.clone(),
                project: Some(forge.project.clone()),
            },
            cx,
        )
    });
    draw(cx);
    assert!(
        cx.debug_bounds("composer-goal-label").is_none(),
        "New Thread must clear the goal immediately, without a retracting old goal"
    );
    open(&core, &host, &second, cx);
    settle(cx, &conversation);
    assert!(
        cx.debug_bounds("composer-goal-label").is_none(),
        "the other thread has no goal"
    );
    open(&core, &host, &first, cx);
    wait(cx, "the first thread's own goal restored", |cx| {
        draw(cx);
        cx.debug_bounds("composer-goal-label").is_some()
    });
}
