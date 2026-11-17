use bytes::Buf;
use futures::{StreamExt as _, TryStream};
use serde::de::DeserializeOwned;
use sse_core::{SseEvent, SseStream};
use std::fmt::Display;
use std::time::Duration;

pub async fn read_until<S, T>(
    stream: S,
    what: &str,
    limit: Duration,
    mut done: impl FnMut(&T) -> bool,
) -> Vec<T>
where
    S: TryStream,
    S::Ok: Buf,
    S::Error: Display,
    T: DeserializeOwned,
{
    let mut events = std::pin::pin!(SseStream::new(stream));
    crate::wait::within(what, limit, async {
        let mut received = Vec::new();
        loop {
            let message = match events.next().await {
                None => panic!(
                    "the event stream ended before {what} after {} events",
                    received.len()
                ),
                Some(Err(error)) => panic!("reading the event stream before {what}: {error}"),
                Some(Ok(SseEvent::Retry(_))) => continue,
                Some(Ok(SseEvent::Message(message))) => message,
            };
            let event: T = serde_json::from_str(&message.data)
                .unwrap_or_else(|error| panic!("decoding {:?}: {error}", message.data));
            let finished = done(&event);
            received.push(event);
            if finished {
                return received;
            }
        }
    })
    .await
}
