use crate::client::Editor;
use artisan_api::{Envelope, Event, Item, ItemId, Run, RunId, RunStatus, Seq, ThreadId};
use futures::StreamExt as _;
use sse_core::{SseEvent, SseStream};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::time::Instant;

const SETTLE: Duration = Duration::from_secs(60);

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Screen {
    items: BTreeMap<ItemId, Item>,
    runs: BTreeMap<RunId, Run>,
}

impl Screen {
    fn apply(&mut self, event: &Event) {
        if let Event::Item { item } = event {
            self.items.insert(item.id.clone(), item.clone());
        } else if let Event::Run { run } = event {
            self.runs.insert(run.id.clone(), run.clone());
        }
    }

    pub(crate) fn runs(&self, thread: &ThreadId) -> impl Iterator<Item = &Run> {
        self.runs
            .values()
            .filter(move |run| &run.thread_id == thread)
    }

    pub(crate) fn settled(&self, thread: &ThreadId) -> usize {
        self.runs(thread)
            .filter(|run| run.status() != RunStatus::Running)
            .count()
    }

    pub(crate) fn live(&self, thread: &ThreadId) -> bool {
        self.runs(thread)
            .any(|run| run.status() == RunStatus::Running)
    }

    pub(crate) fn thread_items(&self, thread: &ThreadId) -> Vec<&Item> {
        let mut items: Vec<&Item> = self
            .items
            .values()
            .filter(|item| &item.thread_id == thread)
            .collect();
        items.sort_by_key(|item| item.position);
        items
    }
}

pub(crate) struct Observer {
    start: Seq,
    cursor: Seq,
    screen: Screen,
    cut_after: Option<usize>,
    reconnects: usize,
}

#[derive(Debug)]
enum Read {
    Done,
    Ended,
    Cut,
}

impl Observer {
    pub(crate) const fn new(start: Seq) -> Self {
        Self {
            start,
            cursor: start,
            screen: Screen {
                items: BTreeMap::new(),
                runs: BTreeMap::new(),
            },
            cut_after: None,
            reconnects: 0,
        }
    }

    pub(crate) const fn screen(&self) -> &Screen {
        &self.screen
    }

    pub(crate) const fn cut_next_stream(&mut self, after: usize) {
        self.cut_after = Some(after);
    }

    pub(crate) const fn reconnects(&self) -> usize {
        self.reconnects
    }

    pub(crate) async fn until(
        &mut self,
        editor: &Editor,
        address: SocketAddr,
        what: &str,
        done: impl Fn(Seq, &Screen) -> bool + Send + Sync,
    ) {
        let deadline = Instant::now() + SETTLE;
        while !done(self.cursor, &self.screen) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "waited {SETTLE:?} for {what}; the Editor sees {:#?}",
                self.screen
            );
            match tokio::time::timeout(left, self.read(editor, address, &done)).await {
                Ok(Read::Done) => return,
                Ok(Read::Ended | Read::Cut) => self.reconnects += 1,
                Err(_elapsed) => {}
            }
        }
    }

    async fn read(
        &mut self,
        editor: &Editor,
        address: SocketAddr,
        done: &(impl Fn(Seq, &Screen) -> bool + Send + Sync),
    ) -> Read {
        let path = format!("/events?since={}", self.cursor);
        let response = editor
            .http()
            .get(Editor::url(address, &path))
            .send()
            .await
            .unwrap_or_else(|error| panic!("open the event stream at {}: {error}", self.cursor));
        assert!(
            response.status().is_success(),
            "the event stream opens at {}: {}",
            self.cursor,
            response.status()
        );
        let mut events = std::pin::pin!(SseStream::new(response.bytes_stream()));
        let mut budget = self.cut_after.take();
        while let Some(message) = events.next().await {
            let Ok(message) = message else {
                return Read::Ended;
            };
            let SseEvent::Message(message) = message else {
                continue;
            };
            let envelope: Envelope = serde_json::from_str(&message.data)
                .unwrap_or_else(|error| panic!("decode the event {:?}: {error}", message.data));
            assert!(
                envelope.event != Event::Reset,
                "Forge reset the Editor's cursor {} that it handed out itself",
                self.cursor
            );
            if let Some(seq) = envelope.seq {
                assert!(
                    seq > self.cursor,
                    "the event stream went from seq {} back to {seq}",
                    self.cursor
                );
                self.cursor = seq;
            }
            self.screen.apply(&envelope.event);
            if done(self.cursor, &self.screen) {
                return Read::Done;
            }
            if let Some(left) = budget.as_mut() {
                if *left == 0 {
                    return Read::Cut;
                }
                *left -= 1;
            }
        }
        Read::Ended
    }

    pub(crate) async fn fresh(&self, editor: &Editor, address: SocketAddr, head: Seq) -> Screen {
        let mut fresh = Self::new(self.start);
        fresh
            .until(
                editor,
                address,
                "a fresh read up to Forge's head",
                |cursor, _| cursor >= head,
            )
            .await;
        fresh.screen
    }
}
