//! Fetches the image and audio files replies name from the Forge's machine.
//!
//! The rich blocks' loaders ask through the [`MediaFetcher`] installed for
//! the open thread (`impl_rich_blocks.rs`). Their futures run off the UI
//! thread, so a request travels over a channel into the application, which
//! sends one `ReadMediaFile` per distinct read and answers every waiter for
//! it when the Forge replies. Waiters still pending when the service stops
//! are answered empty, so no block waits forever.

use std::collections::HashMap;
use std::sync::Arc;

use artisan_domain::{MediaFilePath, MediaPathAnchor, ReadMediaFile, ThreadId};
use artisan_ui::markdown_renderer::{MediaFetch, MediaFetcher, MediaRequest};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};

use super::*;

type Reply = oneshot::Sender<Option<Arc<[u8]>>>;

/// The channel blocks request files on, and the requests in flight.
pub(super) struct MediaFiles {
    requests: mpsc::UnboundedSender<(ReadMediaFile, Reply)>,
    waiters: HashMap<ReadMediaFile, Vec<Reply>>,
    _drain: Task<()>,
}

impl MediaFiles {
    pub(super) fn new(cx: &mut Context<NativeApplication>) -> Self {
        let (requests, mut incoming) = mpsc::unbounded::<(ReadMediaFile, Reply)>();
        let drain = cx.spawn(async move |view, cx| {
            while let Some((read, reply)) = incoming.next().await {
                if view
                    .update(cx, |application, _| {
                        application.request_media_file(read, reply)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            requests,
            waiters: HashMap::new(),
            _drain: drain,
        }
    }

    /// The fetcher the blocks of `thread` load files through.
    pub(super) fn fetcher(&self, thread: &ThreadId) -> MediaFetcher {
        let requests = self.requests.clone();
        let thread = thread.clone();
        MediaFetcher::new(thread.as_str().to_owned(), move |request: MediaRequest| {
            let anchor = if request.from_home {
                MediaPathAnchor::Home
            } else {
                MediaPathAnchor::Project
            };
            let Ok(path) = MediaFilePath::new(request.path, anchor) else {
                return empty();
            };
            let (reply, answer) = oneshot::channel();
            let read = ReadMediaFile::new(thread.clone(), path);
            if requests.unbounded_send((read, reply)).is_err() {
                return empty();
            }
            Box::pin(async move { answer.await.ok().flatten() }) as MediaFetch
        })
    }
}

fn empty() -> MediaFetch {
    Box::pin(async { None })
}

impl NativeApplication {
    /// Queues `reply` for `read`, asking the Forge only for the first waiter.
    fn request_media_file(&mut self, read: ReadMediaFile, reply: Reply) {
        let waiters = self.media_files.waiters.entry(read.clone()).or_default();
        waiters.push(reply);
        if waiters.len() > 1 {
            return;
        }
        if self
            .submit_command(NativeTransportCommand::ReadMediaFile(read.clone()))
            .is_err()
        {
            self.settle_media_file(&read, None);
        }
    }

    /// Answers every waiter for `read`.
    pub(super) fn settle_media_file(&mut self, read: &ReadMediaFile, bytes: Option<Arc<[u8]>>) {
        for reply in self.media_files.waiters.remove(read).unwrap_or_default() {
            let _ = reply.send(bytes.clone());
        }
    }

    /// Answers every waiting read empty: the service that would answer them
    /// has stopped.
    pub(super) fn settle_all_media_files(&mut self) {
        for (_, replies) in self.media_files.waiters.drain() {
            for reply in replies {
                let _ = reply.send(None);
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/media_files.rs"]
mod tests;
