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
    let default = device
        .default_output_config()
        .map_err(|error| format!("No audio output: {}", error_chain(&error)))?;
    let config = default.config();
    macro_rules! stream {
        ($sample:ty) => {
            create_stream::<$sample>(&device, config, shared, generation, format.rate)?
        };
    }
    let stream = match default.sample_format() {
        ::cpal::SampleFormat::I8 => stream!(i8),
        ::cpal::SampleFormat::I16 => stream!(i16),
        ::cpal::SampleFormat::I24 => stream!(::cpal::I24),
        ::cpal::SampleFormat::I32 => stream!(i32),
        ::cpal::SampleFormat::I64 => stream!(i64),
        ::cpal::SampleFormat::U8 => stream!(u8),
        ::cpal::SampleFormat::U16 => stream!(u16),
        ::cpal::SampleFormat::U24 => stream!(::cpal::U24),
        ::cpal::SampleFormat::U32 => stream!(u32),
        ::cpal::SampleFormat::U64 => stream!(u64),
        ::cpal::SampleFormat::F32 => stream!(f32),
        ::cpal::SampleFormat::F64 => stream!(f64),
        other => {
            return Err(format!(
                "No audio output: unsupported PCM sample format {other:?}"
            ));
        }
    };

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

fn create_stream<T: ::cpal::SizedSample + ::cpal::FromSample<f32>>(
    device: &::cpal::Device,
    config: ::cpal::StreamConfig,
    shared: &Arc<Shared>,
    generation: u64,
    track_rate: u32,
) -> Result<::cpal::Stream, String> {
    let out_channels = usize::from(config.channels);
    let step = f64::from(track_rate) / f64::from(config.sample_rate.max(1));
    let callback_shared = shared.clone();
    let error_shared = shared.clone();
    let mut phase = 0.0;
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _: &::cpal::OutputCallbackInfo| {
                callback_shared.lock().fill_mapped(
                    generation,
                    data,
                    out_channels,
                    step,
                    &mut phase,
                    T::from_sample,
                );
            },
            move |error| {
                error_shared.lock().fail(
                    generation,
                    format!("Audio output stopped: {}", error_chain(&error)),
                )
            },
            None,
        )
        .map_err(|error| format!("No audio output: {}", error_chain(&error)))
}
