use std::ffi::{CString, OsStr};
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use futures::executor::block_on;
use pulseaudio::protocol::stream::BufferAttr;
use pulseaudio::protocol::{
    self, ChannelMap, ChannelVolume, PlaybackStreamParams, Prop, SampleFormat, SampleSpec,
};
use pulseaudio::{AsPlaybackSource as _, Client};

use super::{Command, Format, control, error_chain};
use crate::state::Shared;

const SAMPLE_BYTES: usize = 4;

const BUFFER_DIVISOR: u32 = 10;

pub(super) fn run(
    shared: &Arc<Shared>,
    generation: u64,
    format: &Format,
    commands: &Receiver<Command>,
) -> Result<(), String> {
    let client = connect()?;
    let channels = u8::try_from(format.channels)
        .map_err(|_too_many| format!("No audio output: {} channels", format.channels))?;
    let frame_bytes = format.channels * SAMPLE_BYTES;
    let target = format.rate.saturating_mul(u32::from(channels) * 4) / BUFFER_DIVISOR;
    let mut params = PlaybackStreamParams {
        sample_spec: SampleSpec {
            format: SampleFormat::Float32Le,
            channels,
            sample_rate: format.rate,
        },
        channel_map: if channels == 1 {
            ChannelMap::mono()
        } else {
            ChannelMap::stereo()
        },
        cvolume: Some(ChannelVolume::norm(channels)),
        sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
        buffer_attr: BufferAttr {
            max_length: u32::MAX,
            target_length: target,
            pre_buffering: u32::MAX,
            minimum_request_length: target / 4,
            fragment_size: u32::MAX,
        },
        ..PlaybackStreamParams::default()
    };
    if let Some(title) = format.title.as_deref().map(|title| {
        CString::new(title.replace('\0', "")).expect("invariant: NULs were removed from the title")
    }) {
        params.props.set(Prop::MediaName, title);
    }

    let source = {
        let shared = Arc::clone(shared);
        let channels = format.channels;
        let mut scratch = Vec::new();
        let mut phase = 0.0;
        move |bytes: &mut [u8]| {
            let whole = bytes.len() / frame_bytes * frame_bytes;
            if whole == 0 {
                bytes.fill(0);
                return bytes.len();
            }
            scratch.resize(whole / SAMPLE_BYTES, 0.0);
            shared
                .lock()
                .fill(generation, &mut scratch, channels, 1.0, &mut phase);
            for (chunk, sample) in bytes
                .as_chunks_mut::<SAMPLE_BYTES>()
                .0
                .iter_mut()
                .zip(&scratch)
            {
                chunk.copy_from_slice(&sample.to_le_bytes());
            }
            whole
        }
    };
    let stream = block_on(client.create_playback_stream(params, source.as_playback_source()))
        .map_err(|error| {
            format!(
                "No audio output: PulseAudio refused the stream ({})",
                error_chain(&error)
            )
        })?;

    let stopped =
        |error: pulseaudio::ClientError| format!("Audio output stopped: {}", error_chain(&error));
    let result = control(
        shared,
        generation,
        commands,
        true,
        |running| {
            if running {
                block_on(stream.uncork()).map_err(stopped)
            } else {
                block_on(stream.cork()).map_err(stopped)
            }
        },
        || block_on(stream.flush()).map_err(stopped),
    );
    drop(client);
    drop(stream);
    result
}

enum Cookie {
    Read(Vec<u8>),
    Missing,
    Unreadable(PathBuf, std::io::Error),
}

fn connect() -> Result<Client, String> {
    let socket =
        server_socket().ok_or_else(|| "No audio output: PulseAudio is not running".to_owned())?;
    let connection = UnixStream::connect(&socket).map_err(|error| {
        format!(
            "No audio output: couldn't connect to PulseAudio at {} ({error})",
            socket.display()
        )
    })?;
    let cookie = pulseaudio::cookie_path_from_env().map_or(Cookie::Missing, |path| {
        match std::fs::read(&path) {
            Ok(bytes) => Cookie::Read(bytes),
            Err(error) if error.kind() == ErrorKind::NotFound => Cookie::Missing,
            Err(error) => Cookie::Unreadable(path, error),
        }
    });
    let (bytes, unauthenticated) = match cookie {
        Cookie::Read(bytes) => (Some(bytes), None),
        Cookie::Missing => (None, Some("no auth cookie found".to_owned())),
        Cookie::Unreadable(path, error) => (
            None,
            Some(format!(
                "couldn't read the auth cookie at {}: {error}",
                path.display()
            )),
        ),
    };
    Client::new_unix(c"Artisan Editor", connection, bytes).map_err(|error| {
        unauthenticated.map_or_else(
            || {
                format!(
                    "No audio output: PulseAudio refused the connection ({})",
                    error_chain(&error)
                )
            },
            |reason| format!("No audio output: PulseAudio refused the connection ({reason})"),
        )
    })
}

fn server_socket() -> Option<PathBuf> {
    let configured = std::env::var_os("PULSE_SERVER").and_then(|server| {
        let bytes = server.as_bytes();
        let path = bytes.strip_prefix(b"unix:").unwrap_or(bytes);
        path.starts_with(b"/")
            .then(|| PathBuf::from(OsStr::from_bytes(path)))
    });
    let runtime =
        std::env::var_os("PULSE_RUNTIME_PATH").map(|path| PathBuf::from(path).join("native"));
    let session =
        std::env::var_os("XDG_RUNTIME_DIR").map(|path| PathBuf::from(path).join("pulse/native"));
    let user = std::fs::metadata("/proc/self")
        .ok()
        .map(|own| PathBuf::from(format!("/run/user/{}/pulse/native", own.uid())));
    let wslg = Some(PathBuf::from("/mnt/wslg/PulseServer"));
    [configured, runtime, session, user, wslg]
        .into_iter()
        .flatten()
        .find(|path| path.exists())
}
