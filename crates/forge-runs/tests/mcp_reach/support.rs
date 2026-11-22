use super::{Change, Endpoint, Entry, Probe, Reach, Reachability, Refresh, ServerList};
use crate::mcp_auth::ReadMcpServersError;
use artisan_api::McpServer;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

fn endpoint(address: std::net::SocketAddr) -> Endpoint {
    (address.ip().to_string(), address.port())
}

const fn reach(answers: bool) -> Reach {
    if answers {
        Reach::Answers
    } else {
        Reach::Silent
    }
}

pub(crate) struct Gate {
    endpoint: Endpoint,
    probe: Probe,
}

impl Gate {
    pub(crate) fn open(self, reachability: &Reachability, answers: bool) {
        reachability.record(&self.endpoint, reach(answers), self.probe);
    }

    pub(crate) fn publish_at(self, reachability: &Reachability, answers: bool, at: Instant) {
        reachability.record_at(&self.endpoint, reach(answers), self.probe, at);
    }

    pub(crate) fn turns_waiting(&self) -> usize {
        self.probe.publish.receiver_count().saturating_sub(1)
    }
}

pub(crate) type Read = Result<Vec<McpServer>, ReadMcpServersError>;

pub(crate) struct ScriptedServers {
    reads: Mutex<VecDeque<Read>>,
    count: Arc<AtomicUsize>,
    changes: tokio::sync::mpsc::UnboundedReceiver<()>,
}

pub(crate) struct Script {
    count: Arc<AtomicUsize>,
    changes: tokio::sync::mpsc::UnboundedSender<()>,
}

impl Script {
    pub(crate) fn reads(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    pub(crate) fn edit(&self) {
        self.changes
            .send(())
            .expect("the watcher listens for changes while the test runs");
    }
}

pub(crate) fn scripted(reads: impl IntoIterator<Item = Read>) -> (ScriptedServers, Script) {
    let reads = Mutex::new(reads.into_iter().collect());
    let count = Arc::new(AtomicUsize::new(0));
    let (send, changes) = tokio::sync::mpsc::unbounded_channel();
    (
        ScriptedServers {
            reads,
            count: Arc::clone(&count),
            changes,
        },
        Script {
            count,
            changes: send,
        },
    )
}

impl ServerList for ScriptedServers {
    fn read(&self) -> impl Future<Output = Read> + Send {
        self.count.fetch_add(1, Ordering::SeqCst);
        std::future::ready(
            self.reads
                .lock()
                .unwrap()
                .pop_front()
                .expect("the test scripted every read the watcher makes"),
        )
    }

    async fn changed(&mut self) -> Change {
        match self.changes.recv().await {
            Some(()) => Change::Edited,
            None => Change::Closed,
        }
    }
}

impl Reachability {
    pub(crate) fn probed(&self, endpoints: usize) -> bool {
        let book = self.book();
        book.entries.len() == endpoints
            && book
                .entries
                .values()
                .all(|entry| matches!(entry, Entry::Probed { .. }))
    }

    pub(crate) fn reprobing(&self, address: std::net::SocketAddr) -> bool {
        matches!(
            self.book().entries.get(&endpoint(address)),
            Some(Entry::Reprobing { .. })
        )
    }

    pub(crate) fn answers(&self, address: std::net::SocketAddr) -> Option<bool> {
        self.book()
            .entries
            .get(&endpoint(address))?
            .known()
            .map(|reach| reach == Reach::Answers)
    }

    pub(crate) fn checked_endpoints(&self) -> usize {
        self.book().entries.len()
    }

    pub(crate) fn due(&self, address: std::net::SocketAddr) -> bool {
        self.book()
            .entries
            .get(&endpoint(address))
            .and_then(|entry| entry.claim(Refresh::Due, Instant::now()))
            .is_some()
    }

    pub(crate) fn configure(self: &Arc<Self>, servers: &[McpServer]) {
        self.configured(servers, Refresh::Due);
    }

    pub(crate) fn first_probe_in_flight(&self, address: std::net::SocketAddr) -> Gate {
        let (probing, probe) = self.probing();
        self.book()
            .entries
            .insert(endpoint(address), Entry::FirstProbe(probing));
        Gate {
            endpoint: endpoint(address),
            probe,
        }
    }

    pub(crate) fn reprobe_in_flight(&self, address: std::net::SocketAddr) -> Gate {
        let (probing, probe) = self.probing();
        let mut book = self.book();
        let entry = book
            .entries
            .get_mut(&endpoint(address))
            .expect("only a checked server is checked again");
        let reach = entry.known().expect("only a finished check is repeated");
        *entry = Entry::Reprobing { reach, probing };
        Gate {
            endpoint: endpoint(address),
            probe,
        }
    }

    pub(crate) fn forget(&self, address: std::net::SocketAddr) {
        self.book().entries.remove(&endpoint(address));
    }

    #[expect(clippy::iter_over_hash_type, reason = "every check is aged alike")]
    pub(crate) fn age(&self, by: Duration) {
        for entry in self.book().entries.values_mut() {
            match entry {
                Entry::Probed { at, .. } => {
                    *at = at
                        .checked_sub(by)
                        .expect("the monotonic clock has run longer than the test ages a check");
                }
                Entry::FirstProbe(_) | Entry::Reprobing { .. } => {}
            }
        }
        self.rescheduled.notify_one();
    }
}
