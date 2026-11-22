use serde::Deserialize;
use std::convert::Infallible;
use std::time::Duration;
use test_support::sse::read_until;

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Note {
    text: String,
}

fn chunked(
    parts: Vec<&'static [u8]>,
) -> impl futures::TryStream<Ok = &'static [u8], Error = Infallible> {
    futures::stream::iter(parts.into_iter().map(Ok))
}

#[tokio::test]
async fn a_character_split_across_chunks_arrives_whole() {
    let stream = chunked(vec![
        b"data: {\"text\":\"caf\xC3",
        b"\xA9 \xE2\x9C",
        b"\x93\"}\n\n: keep-alive\n\ndata: {\"text\":\"end\"}\n\n",
    ]);
    let notes: Vec<Note> = read_until(
        stream,
        "the end note",
        Duration::from_secs(5),
        |note: &Note| note.text == "end",
    )
    .await;
    assert_eq!(
        notes,
        [
            Note {
                text: "café ✓".to_owned()
            },
            Note {
                text: "end".to_owned()
            }
        ],
        "split UTF-8 must decode without replacement characters"
    );
}

#[tokio::test]
#[should_panic(expected = "the event stream ended before the end note after 1 events")]
async fn a_stream_that_closes_before_the_awaited_event_fails() {
    let stream = chunked(vec![b"data: {\"text\":\"first\"}\n\n"]);
    read_until(
        stream,
        "the end note",
        Duration::from_secs(5),
        |note: &Note| note.text == "end",
    )
    .await;
}
