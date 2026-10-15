//! Bounded decoder producer. The maintained decoder owns format/seek logic;
//! the channel owns backpressure and keeps cancellation responsive while paused.
use crate::decode::StreamDecoder;
use crate::output::Output;
use crate::state::Shared;
use crossbeam_channel::{Receiver, Sender, bounded, select};
use std::sync::Arc;
use std::time::Duration;

pub(crate) enum PcmEvent {
    Data { epoch: u64, samples: Vec<f32> },
    End { epoch: u64 },
}
pub(crate) struct StreamState {
    pub(crate) pcm: Receiver<PcmEvent>,
    pub(crate) signal: Sender<()>,
    pub(crate) epoch: u64,
    pub(crate) seek: Option<Duration>,
    pub(crate) duration: Option<Duration>,
    pub(crate) start: usize,
    pub(crate) ended: bool,
}
impl StreamState {
    pub(crate) fn new(
        pcm: Receiver<PcmEvent>,
        signal: Sender<()>,
        duration: Option<Duration>,
    ) -> Self {
        Self {
            pcm,
            signal,
            duration,
            epoch: 0,
            seek: None,
            start: 0,
            ended: false,
        }
    }
}
const QUEUE_CHUNKS: usize = 8;

pub(crate) fn play(shared: Arc<Shared>, generation: u64, mut decoder: StreamDecoder) {
    let first = match decoder.chunk() {
        Ok(samples) if !samples.is_empty() => decoder.audio(samples),
        Ok(_) => {
            shared
                .lock()
                .fail(generation, "Audio contains no samples".to_owned());
            return;
        }
        Err(error) => {
            shared.lock().fail(generation, error);
            return;
        }
    };
    let (pcm, received) = bounded(QUEUE_CHUNKS);
    // One wakeup is sufficient; pending seek position lives in the shared state.
    let (signal, commands) = bounded(1);
    let state = StreamState::new(received, signal, decoder.duration);
    {
        let mut inner = shared.lock();
        let Some(format) = inner.stream_ready(generation, first, state) else {
            return;
        };
        match Output::open(shared.clone(), generation, format) {
            Ok(output) => inner.attach(output),
            Err(error) => {
                inner.fail(generation, error);
                return;
            }
        }
    }
    produce(&shared, generation, &mut decoder, pcm, commands);
}

fn produce(
    shared: &Shared,
    generation: u64,
    decoder: &mut StreamDecoder,
    pcm: Sender<PcmEvent>,
    commands: Receiver<()>,
) {
    let mut epoch = 0;
    let mut ended = false;
    loop {
        if ended {
            if commands.recv().is_err() {
                return;
            }
            match apply_seek(shared, generation, decoder) {
                Ok(Some(next)) => {
                    epoch = next;
                    ended = false;
                }
                Ok(None) => continue,
                Err(error) => {
                    shared.lock().fail(generation, error);
                    return;
                }
            }
        }
        if commands.try_recv().is_ok() {
            match apply_seek(shared, generation, decoder) {
                Ok(Some(next)) => epoch = next,
                Ok(None) => {}
                Err(error) => {
                    shared.lock().fail(generation, error);
                    return;
                }
            }
        }
        if !shared.lock().is_current(generation) {
            return;
        }
        let samples = match decoder.chunk() {
            Ok(samples) => samples,
            Err(error) => {
                shared.lock().fail(generation, error);
                return;
            }
        };
        let eof = samples.is_empty();
        let event = if eof {
            PcmEvent::End { epoch }
        } else {
            PcmEvent::Data { epoch, samples }
        };
        // A seek or stop interrupts a full queue. No polling heartbeat or busy wait.
        select! {
            send(pcm, event) -> result => { if result.is_err() { return; } ended = eof; }
            recv(commands) -> command => {
                if command.is_err() { return; }
                match apply_seek(shared, generation, decoder) {
                    Ok(Some(next)) => { epoch = next; ended = false; }
                    Ok(None) => {}
                    Err(error) => { shared.lock().fail(generation, error); return; }
                }
            }
        }
    }
}
fn apply_seek(
    shared: &Shared,
    generation: u64,
    decoder: &mut StreamDecoder,
) -> Result<Option<u64>, String> {
    let Some((epoch, position)) = shared.lock().take_seek(generation) else {
        return Ok(None);
    };
    decoder.seek(position)?;
    Ok(Some(epoch))
}
