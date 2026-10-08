//! Follows the output files of an engine's background shell jobs.
//!
//! Claude Code runs a background `Bash` job with its output redirected to a
//! file and never streams that output. The follower reads what each job
//! appended every [`POLL_INTERVAL`] and sends it as shell job output on the
//! turn's observation channel. Settling a job reads its file to the end
//! first, so the job's last words arrive before it shows as stopped.
//!
//! One follower serves one turn and must be closed before the turn's
//! terminal observation: closing reads every file to the end and drops the
//! follower's sender, so the channel can close behind the turn.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use artisan_domain::RunId;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::observation::{EngineObservation, ShellJobChange, ShellJobEnd, ShellJobRow};

/// How often appended output is read.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Most bytes read from one file per poll, so one chatty job cannot hold the
/// channel; the rest follows on the next poll.
const READ_MAX_BYTES: usize = 256 * 1024;

/// Longest a close waits for the final reads.
const CLOSE_BUDGET: Duration = Duration::from_secs(2);

/// The follower of one turn's background shell jobs.
#[derive(Debug)]
pub(crate) struct ShellTail {
    commands: mpsc::UnboundedSender<Command>,
    task: JoinHandle<()>,
}

#[derive(Debug)]
enum Command {
    Follow {
        run_id: RunId,
        job_id: String,
        path: PathBuf,
    },
    Settle {
        run_id: RunId,
        job_id: String,
        end: ShellJobEnd,
    },
    Close(oneshot::Sender<()>),
}

struct Followed {
    run_id: RunId,
    path: PathBuf,
    read: u64,
}

impl ShellTail {
    /// Starts a follower that sends output on `observations`.
    pub(crate) fn spawn(observations: mpsc::Sender<EngineObservation>) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let task = tokio::spawn(follow_loop(observations, receiver));
        Self { commands, task }
    }

    /// Follows `job_id`'s output file at `path`. Following a job twice keeps
    /// the first file.
    pub(crate) fn follow(&self, run_id: &RunId, job_id: &str, path: &str) {
        let _ = self.commands.send(Command::Follow {
            run_id: run_id.clone(),
            job_id: job_id.to_owned(),
            path: PathBuf::from(path),
        });
    }

    /// Reads the rest of `job_id`'s output, then reports it stopped. A job
    /// not followed is reported stopped at once.
    pub(crate) fn settle(&self, run_id: &RunId, job_id: &str, end: ShellJobEnd) {
        let _ = self.commands.send(Command::Settle {
            run_id: run_id.clone(),
            job_id: job_id.to_owned(),
            end,
        });
    }

    /// Reads every followed file to the end and stops, waiting at most
    /// [`CLOSE_BUDGET`].
    pub(crate) async fn close(self) {
        let (done, closed) = oneshot::channel();
        if self.commands.send(Command::Close(done)).is_ok() {
            let _ = tokio::time::timeout(CLOSE_BUDGET, closed).await;
        }
        self.task.abort();
    }
}

async fn follow_loop(
    observations: mpsc::Sender<EngineObservation>,
    mut commands: mpsc::UnboundedReceiver<Command>,
) {
    let mut followed: HashMap<String, Followed> = HashMap::new();
    let mut poll = tokio::time::interval(POLL_INTERVAL);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Follow { run_id, job_id, path }) => {
                    followed.entry(job_id).or_insert(Followed { run_id, path, read: 0 });
                }
                Some(Command::Settle { run_id, job_id, end }) => {
                    if let Some(mut job) = followed.remove(&job_id)
                        && !drain(&observations, &job_id, &mut job).await
                    {
                        return;
                    }
                    let row = ShellJobRow {
                        run_id,
                        job_id,
                        change: ShellJobChange::Settled(end),
                    };
                    if observations.send(EngineObservation::Shell(row)).await.is_err() {
                        return;
                    }
                }
                Some(Command::Close(done)) => {
                    for (job_id, job) in &mut followed {
                        if !drain(&observations, job_id, job).await {
                            break;
                        }
                    }
                    let _ = done.send(());
                    return;
                }
                None => return,
            },
            _ = poll.tick() => {
                for (job_id, job) in &mut followed {
                    if !read_appended(&observations, job_id, job).await.unwrap_or(true) {
                        return;
                    }
                }
            }
        }
    }
}

/// Reads a job's file to its current end. Returns `false` once the channel
/// closed.
async fn drain(
    observations: &mpsc::Sender<EngineObservation>,
    job_id: &str,
    job: &mut Followed,
) -> bool {
    loop {
        match read_appended(observations, job_id, job).await {
            Some(true) => {}
            Some(false) => return false,
            None => return true,
        }
    }
}

/// Sends what the job's file gained since the last read, at most
/// [`READ_MAX_BYTES`]. Returns `None` when nothing was read (no file yet, or
/// no new bytes), else whether the channel is still open.
async fn read_appended(
    observations: &mpsc::Sender<EngineObservation>,
    job_id: &str,
    job: &mut Followed,
) -> Option<bool> {
    let mut file = tokio::fs::File::open(&job.path).await.ok()?;
    let length = file.metadata().await.ok()?.len();
    // A file that shrank was rewritten: follow it from its start again.
    if length < job.read {
        job.read = 0;
    }
    if length == job.read {
        return None;
    }
    file.seek(std::io::SeekFrom::Start(job.read)).await.ok()?;
    let wanted = usize::try_from(length - job.read)
        .unwrap_or(usize::MAX)
        .min(READ_MAX_BYTES);
    let mut bytes = vec![0; wanted];
    let read = file.read(&mut bytes).await.ok()?;
    if read == 0 {
        return None;
    }
    bytes.truncate(read);
    job.read += read as u64;
    let row = ShellJobRow {
        run_id: job.run_id.clone(),
        job_id: job_id.to_owned(),
        change: ShellJobChange::Output(bytes),
    };
    Some(
        observations
            .send(EngineObservation::Shell(row))
            .await
            .is_ok(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> RunId {
        RunId::parse("run-1".to_owned()).expect("run id")
    }

    async fn next_change(receiver: &mut mpsc::Receiver<EngineObservation>) -> ShellJobChange {
        let observation = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("an observation in time")
            .expect("an open channel");
        match observation {
            EngineObservation::Shell(row) => row.change,
            other => panic!("unexpected observation {other:?}"),
        }
    }

    #[tokio::test]
    async fn follows_appended_output_and_settles_after_the_rest() {
        let directory = std::env::temp_dir().join(format!("shell-tail-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("directory");
        let path = directory.join("job.output");
        std::fs::write(&path, b"\x1b[31mred\x1b[0m\n").expect("first write");
        let (sender, mut receiver) = mpsc::channel(16);
        let tail = ShellTail::spawn(sender);
        tail.follow(&run(), "job", path.to_str().expect("utf-8 path"));
        assert_eq!(
            next_change(&mut receiver).await,
            ShellJobChange::Output(b"\x1b[31mred\x1b[0m\n".to_vec())
        );
        std::fs::write(&path, b"\x1b[31mred\x1b[0m\ndone\n").expect("second write");
        tail.settle(&run(), "job", ShellJobEnd::Exited(Some(3)));
        assert_eq!(
            next_change(&mut receiver).await,
            ShellJobChange::Output(b"done\n".to_vec())
        );
        assert_eq!(
            next_change(&mut receiver).await,
            ShellJobChange::Settled(ShellJobEnd::Exited(Some(3)))
        );
        tail.close().await;
        let _ = std::fs::remove_dir_all(&directory);
    }
}
