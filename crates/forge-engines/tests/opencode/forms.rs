use super::decoder::{Answered, Decoder};
use crate::{EngineEvent, Input, RunIo};
use artisan_api::{Answer, ItemKind, QuestionAnswer, Status};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;

fn asked(field: &str) -> Decoder {
    let mut decoder = Decoder::new(Some("root".to_owned()));
    decoder
        .feed(
            &json!({ "id": "evt_form", "type": "form.created", "data": { "form": {
                "id": "frm_1", "sessionID": "root", "title": "Questions",
                "metadata": { "kind": "question", "tool": { "id": "call_1" } },
                "fields": [{ "key": "count", "title": "How many?", "type": field }],
            }}})
            .to_string(),
        )
        .unwrap();
    decoder
}

fn typed(text: &str) -> Answer {
    Answer::Question {
        answers: vec![Some(QuestionAnswer::Text {
            text: text.to_owned(),
        })],
    }
}

fn sent(answered: Answered) -> Value {
    match answered {
        Answered::Reply { reply, .. } => reply.body.expect("a form reply has a body"),
        Answered::Invalid(_) => panic!("the answer was refused"),
        Answered::Unexpected => panic!("the form was not pending"),
    }
}

#[test]
fn an_integer_field_refuses_fractions_and_words_and_stays_answerable() {
    let mut decoder = asked("integer");
    for wrong in ["1.5", "tomorrow"] {
        let Answered::Invalid(events) = decoder.answer("call_1", &typed(wrong)) else {
            panic!("{wrong} must be refused");
        };
        assert!(
            events
                .iter()
                .any(|event| matches!(event, EngineEvent::Item { kind: ItemKind::Error { .. }, .. })),
            "the user is told why {wrong} was refused"
        );
    }
    assert_eq!(
        sent(decoder.answer("call_1", &typed("3"))),
        json!({ "answer": { "count": 3 } })
    );
}

#[test]
fn a_number_field_keeps_fractions_and_refuses_words() {
    let mut decoder = asked("number");
    assert!(matches!(
        decoder.answer("call_1", &typed("tomorrow")),
        Answered::Invalid(_)
    ));
    assert_eq!(
        sent(decoder.answer("call_1", &typed("1.5"))),
        json!({ "answer": { "count": 1.5 } })
    );
}

#[tokio::test]
async fn a_form_reply_opencode_rejects_does_not_mark_the_question_answered() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = super::Api::new(
        format!("http://{}", listener.local_addr().unwrap()),
        "secret".to_owned(),
    )
    .unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        while !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            let mut chunk = [0; 4096];
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
        }
        let head = String::from_utf8_lossy(&bytes).into_owned();
        assert!(
            head.starts_with("POST /api/session/root/form/frm_1/reply HTTP/1.1\r\n"),
            "{head}"
        );
        socket
            .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 7\r\nConnection: close\r\n\r\ninvalid")
            .await
            .unwrap();
    });
    let (_, inbox) = mpsc::channel(8);
    let (events, mut output) = mpsc::channel(8);
    let io = RunIo {
        inbox,
        events,
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    let mut decoder = asked("integer");
    let answered = super::handle_input(
        &api,
        "root",
        &mut decoder,
        &io,
        Input::Answer {
            key: "call_1".to_owned(),
            answer: typed("3"),
        },
    )
    .await;
    assert!(matches!(answered, Err(super::RunError::Answer { .. })));
    server.await.unwrap();
    drop(io);
    while let Some(event) = output.recv().await {
        assert!(
            !matches!(event, EngineEvent::Item { status: Status::Done, kind: ItemKind::Question { .. }, .. }),
            "a rejected reply must not show the question as answered"
        );
    }
}
