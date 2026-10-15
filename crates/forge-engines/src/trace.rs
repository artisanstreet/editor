//! Optional raw diagnostic traces. Normal engine runs do not enable them.
//! A bounded queue drops diagnostic records under pressure; it never delays a run.

use std::io::Write as _;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc::{SyncSender, sync_channel},
};

const MAX_TRACE_BYTES: usize = 64 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Out,
    In,
    Stderr,
    Note,
}
impl Kind {
    const fn name(self) -> &'static str {
        match self {
            Self::Out => "out",
            Self::In => "in",
            Self::Stderr => "stderr",
            Self::Note => "note",
        }
    }
}

struct Record {
    kind: Kind,
    at: i64,
    text: String,
}
struct Sink {
    sender: SyncSender<Record>,
    remaining: AtomicUsize,
}
#[derive(Clone, Default)]
pub(crate) struct Trace {
    sink: Option<Arc<Sink>>,
}

impl Trace {
    pub(crate) fn start(dir: Option<&Path>, name: &str) -> Self {
        let Some(dir) = dir else {
            return Self::default();
        };
        let file = (|| -> std::io::Result<_> {
            std::fs::create_dir_all(dir)?;
            std::fs::File::create(dir.join(format!("{name}-{}.jsonl", artisan_api::now())))
        })();
        let Ok(file) = file else {
            tracing::warn!(directory = %dir.display(), "could not open diagnostic engine trace");
            return Self::default();
        };
        let (sender, receiver) = sync_channel::<Record>(32);
        let sink = Arc::new(Sink {
            sender,
            remaining: AtomicUsize::new(MAX_TRACE_BYTES),
        });
        let worker = std::thread::Builder::new().name("engine-trace".into()).spawn(move || {
            let mut file = std::io::BufWriter::new(file);
            while let Ok(record) = receiver.recv() {
                let value = serde_json::json!({ "at": record.at, "kind": record.kind.name(), "text": record.text });
                let boundary = matches!(record.kind, Kind::Note);
                if serde_json::to_writer(&mut file, &value).is_err() || writeln!(file).is_err() { break; }
                if boundary && file.flush().is_err() { break; }
            }
            let _ = file.flush();
        });
        if worker.is_err() {
            return Self::default();
        }
        Self { sink: Some(sink) }
    }

    pub(crate) fn record(&self, kind: Kind, text: &str) {
        let Some(sink) = &self.sink else { return };
        if text.len() > MAX_RECORD_BYTES {
            return;
        }
        // JSON escaping expands a byte by at most six, plus record metadata.
        let bytes = text.len().saturating_mul(6).saturating_add(128);
        if sink
            .remaining
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(bytes)
            })
            .is_err()
        {
            return;
        }
        let record = Record {
            kind,
            at: artisan_api::now(),
            text: text.to_owned(),
        };
        if sink.sender.try_send(record).is_err() {
            sink.remaining.fetch_add(bytes, Ordering::Relaxed);
        }
    }
}
