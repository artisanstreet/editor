#[cfg(not(all(unix, not(target_os = "macos"))))]
mod cpal;
#[cfg(all(unix, not(target_os = "macos")))]
mod pulse;

#[cfg(not(all(unix, not(target_os = "macos"))))]
use self::cpal as platform;
#[cfg(all(unix, not(target_os = "macos")))]
use self::pulse as platform;

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SendError, Sender};
use std::thread;

use crate::state::Shared;

#[derive(Clone, Debug)]
pub(crate) struct Format {
    #[cfg_attr(
        not(all(unix, not(target_os = "macos"))),
        expect(dead_code, reason = "cpal maps channels per sample instead")
    )]
    pub(crate) channels: usize,
    pub(crate) rate: u32,
    #[cfg_attr(
        not(all(unix, not(target_os = "macos"))),
        expect(dead_code, reason = "cpal streams carry no title")
    )]
    pub(crate) title: Option<String>,
}

enum Command {
    Sync,
    Flush,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum OpenOutputError {
    #[error("Couldn't start the audio output")]
    Spawn {
        generation: u64,
        #[source]
        source: std::io::Error,
    },
}

pub(crate) struct Output {
    commands: Sender<Command>,
}

impl Output {
    pub(crate) fn open(
        shared: Arc<Shared>,
        generation: u64,
        format: Format,
    ) -> Result<Self, OpenOutputError> {
        let (commands, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("artisan-audio-output".to_owned())
            .spawn(move || {
                if let Err(error) = platform::run(&shared, generation, &format, &receiver) {
                    shared.lock().fail(generation, &error);
                }
            })
            .map_err(|source| OpenOutputError::Spawn { generation, source })?;
        Ok(Self { commands })
    }

    pub(crate) fn sync(&self) {
        self.send(Command::Sync);
    }

    fn send(&self, command: Command) {
        match self.commands.send(command) {
            Ok(()) | Err(SendError(Command::Sync | Command::Flush)) => {}
        }
    }

    pub(crate) fn flush(&self) {
        self.send(Command::Flush);
    }
}

fn control<E>(
    shared: &Shared,
    generation: u64,
    commands: &Receiver<Command>,
    mut running: bool,
    mut set_running: impl FnMut(bool) -> Result<(), E>,
    mut flush: impl FnMut() -> Result<(), E>,
) -> Result<(), E> {
    loop {
        let Some(wanted) = shared.lock().wants_running(generation) else {
            return Ok(());
        };
        if wanted != running {
            set_running(wanted)?;
            running = wanted;
        }
        match commands.recv() {
            Ok(Command::Flush) => flush()?,
            Ok(Command::Sync) => {}
            Err(_) => return Ok(()),
        }
    }
}
