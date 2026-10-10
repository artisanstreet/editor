//! Output through `cpal`'s default device on Windows and macOS.
//!
//! The device keeps its own format; the callback resamples linearly from the
//! track's rate and maps its one or two channels onto the device's.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use ::cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};

use super::{Command, Format, control, error_chain};
use crate::state::Shared;

/// Opens the default output device for the track of `generation` and drives
/// it until the player drops the output.
///
/// # Errors
///
/// No output device exists, it refuses the stream, or a later play or pause
/// fails.
pub(super) fn run(
    shared: &Arc<Shared>,
    generation: u64,
    format: &Format,
    commands: &Receiver<Command>,
) -> Result<(), String> {
    let device = ::cpal::default_host()
        .default_output_device()
        .ok_or_else(|| "No audio output: no output device found".to_owned())?;
    let config = device
        .default_output_config()
        .map_err(|error| format!("No audio output: {}", error_chain(&error)))?
        .config();
    let out_channels = usize::from(config.channels);
    let step = f64::from(format.rate) / f64::from(config.sample_rate.max(1));

    let callback_shared = Arc::clone(shared);
    let error_shared = Arc::clone(shared);
    let mut phase = 0.0;
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |data: &mut [f32], _: &::cpal::OutputCallbackInfo| {
                callback_shared
                    .lock()
                    .fill(generation, data, out_channels, step, &mut phase);
            },
            move |error| {
                error_shared.lock().fail(
                    generation,
                    format!("Audio output stopped: {}", error_chain(&error)),
                );
            },
            None,
        )
        .map_err(|error| format!("No audio output: {}", error_chain(&error)))?;

    let stopped = |error: ::cpal::Error| format!("Audio output stopped: {}", error_chain(&error));
    control(
        shared,
        generation,
        commands,
        false,
        |running| {
            if running {
                stream.play().map_err(stopped)
            } else {
                stream.pause().map_err(stopped)
            }
        },
        || Ok(()),
    )
}
