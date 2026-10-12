//! The platform audio output: one stream per playing track, owned by a
//! dedicated thread that follows the shared state.
//!
//! The thread opens the stream, then reconciles it with the state whenever the
//! player sends a command (and every [`POLL`], so a track that ran out is
//! paused): running while playing, paused (corked) otherwise, flushed after a
//! seek. Dropping the [`Output`] closes the command channel, which ends the
//! thread and releases the stream.

#[cfg(not(all(unix, not(target_os = "macos"))))]
mod cpal;
#[cfg(all(unix, not(target_os = "macos")))]
mod pulse;

#[cfg(not(all(unix, not(target_os = "macos"))))]
use self::cpal as platform;
#[cfg(all(unix, not(target_os = "macos")))]
use self::pulse as platform;

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crate::state::Shared;

/// How often the output thread re-checks the state without a command.
const POLL: Duration = Duration::from_millis(100);

/// The layout of the decoded track the output streams.
#[derive(Clone, Debug)]
pub(crate) struct Format {
    /// 1 or 2.
    #[cfg_attr(
        not(all(unix, not(target_os = "macos"))),
        expect(dead_code, reason = "cpal maps channels per sample instead")
    )]
    pub(crate) channels: usize,
    /// Frames per second.
    pub(crate) rate: u32,
    /// The track's file name, shown by sound servers that list streams.
    #[cfg_attr(
        not(all(unix, not(target_os = "macos"))),
        expect(dead_code, reason = "cpal streams carry no title")
    )]
    pub(crate) title: Option<String>,
}

/// A request from the player to the output thread.
enum Command {
    /// Re-read the state and run or pause the stream to match.
    Sync,
    /// Drop buffered audio because the cursor moved.
    Flush,
}

/// The handle to one track's output thread.
pub(crate) struct Output {
    commands: Sender<Command>,
}

impl Output {
    /// Starts the output thread for the track of `generation`. Opening the
    /// device happens on that thread; its failure fails the track.
    ///
    /// # Errors
    ///
    /// The thread couldn't be started.
    pub(crate) fn open(
        shared: Arc<Shared>,
        generation: u64,
        format: Format,
    ) -> Result<Self, String> {
        let (commands, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("artisan-audio-output".to_owned())
            .spawn(move || {
                if let Err(message) = platform::run(&shared, generation, &format, &receiver) {
                    shared.lock().fail(generation, message);
                }
            })
            .map_err(|error| format!("Couldn't start the audio output: {error}"))?;
        Ok(Self { commands })
    }

    /// Asks the thread to match the stream to the state.
    pub(crate) fn sync(&self) {
        // A send fails only once the thread has exited, when there is nothing to sync.
        let _ = self.commands.send(Command::Sync);
    }

    /// Asks the thread to drop buffered audio after a seek.
    pub(crate) fn flush(&self) {
        let _ = self.commands.send(Command::Flush);
    }
}

/// Drives an open stream until the player drops the output: calls
/// `set_running` whenever the wanted run state differs from the stream's, and
/// `flush` on seeks. `running` is whether the stream starts running.
///
/// # Errors
///
/// The first error of `set_running` or `flush`.
fn control(
    shared: &Shared,
    generation: u64,
    commands: &Receiver<Command>,
    mut running: bool,
    mut set_running: impl FnMut(bool) -> Result<(), String>,
    mut flush: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    loop {
        let Some(wanted) = shared.lock().wants_running(generation) else {
            return Ok(());
        };
        if wanted != running {
            set_running(wanted)?;
            running = wanted;
        }
        match commands.recv_timeout(POLL) {
            Ok(Command::Flush) => flush()?,
            Ok(Command::Sync) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

/// Renders an error and its sources as `error: cause: cause`.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
