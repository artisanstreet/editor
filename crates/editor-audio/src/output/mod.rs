#[cfg(not(all(unix, not(target_os = "macos"))))]
mod cpal;
#[cfg(all(unix, not(target_os = "macos")))]
mod pulse;

#[cfg(not(all(unix, not(target_os = "macos"))))]
use self::cpal as platform;
#[cfg(all(unix, not(target_os = "macos")))]
use self::pulse as platform;

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::state::Shared;

#[derive(Clone, Debug)]
pub struct Format {
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

pub struct Output {
    commands: Sender<Command>,
}

impl Output {
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

    pub(crate) fn sync(&self) {
        self.commands.send(Command::Sync).ok();
    }

    pub(crate) fn flush(&self) {
        self.commands.send(Command::Flush).ok();
    }
}

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
        match commands.recv() {
            Ok(Command::Flush) => flush()?,
            Ok(Command::Sync) => {}
            Err(_) => return Ok(()),
        }
    }
}

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
