use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc::{Receiver, SyncSender, sync_channel},
};

const MAX_TRACE_BYTES: usize = 64 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 256 * 1024;
const RECORD_OVERHEAD_BYTES: usize = 128;

#[derive(Clone, Copy)]
pub enum Kind {
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

enum Text {
    Whole(String),
    Truncated { kept: String, bytes: usize },
}

impl Text {
    fn bounded(text: &str) -> Self {
        if text.len() <= MAX_RECORD_BYTES {
            return Self::Whole(text.to_owned());
        }
        Self::Truncated {
            kept: text
                .split_at(text.floor_char_boundary(MAX_RECORD_BYTES))
                .0
                .to_owned(),
            bytes: text.len(),
        }
    }

    fn kept(&self) -> &str {
        match self {
            Self::Whole(kept) | Self::Truncated { kept, .. } => kept,
        }
    }
}

struct Record {
    kind: Kind,
    at: i64,
    text: Text,
}

impl Record {
    fn json(&self) -> serde_json::Value {
        let kind = self.kind.name();
        match &self.text {
            Text::Whole(text) => serde_json::json!({ "at": self.at, "kind": kind, "text": text }),
            Text::Truncated { kept, bytes } => serde_json::json!({
                "at": self.at,
                "kind": kind,
                "text": kept,
                "truncated_from": bytes,
            }),
        }
    }
}

#[derive(Clone, Default)]
pub struct Secrets(Arc<[String]>);

const MIN_SECRET_BYTES: usize = 8;
const SECRET_ENV_MARKERS: [&str; 7] = [
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "AUTH",
    "CREDENTIAL",
    "COOKIE",
];
const REDACTED: &str = "[redacted]";

impl Secrets {
    #[must_use]
    pub fn of(launch: &crate::Launch) -> Self {
        let servers = launch.mcp_servers.iter().flat_map(|server| {
            server
                .url
                .iter()
                .chain(server.headers.values())
                .chain(server.env.values())
        });
        let gateway = launch
            .gateway_mcp
            .iter()
            .flat_map(|(_, url)| std::iter::once(url.as_str()).chain(url.rsplit('/').next()));
        let env = launch
            .env
            .iter()
            .filter(|(key, _)| {
                let key = key.to_ascii_uppercase();
                key == "OPENCODE_CONFIG_CONTENT"
                    || SECRET_ENV_MARKERS.iter().any(|marker| key.contains(marker))
            })
            .map(|(_, value)| value.as_str());
        let mut secrets: Vec<String> = servers
            .map(String::as_str)
            .chain(gateway)
            .chain(env)
            .filter(|secret| secret.len() >= MIN_SECRET_BYTES)
            .flat_map(|secret| {
                let escaped = serde_json::Value::from(secret).to_string();
                let escaped = escaped[1..escaped.len() - 1].to_owned();
                [secret.to_owned(), escaped]
            })
            .collect();
        secrets.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        secrets.dedup();
        Self(secrets.into())
    }

    fn redact(&self, text: &str) -> String {
        self.0.iter().fold(text.to_owned(), |text, secret| {
            text.replace(secret.as_str(), REDACTED)
        })
    }
}

struct Sink {
    secrets: Secrets,
    sender: SyncSender<Record>,
    remaining: AtomicUsize,
    dropped: Arc<AtomicUsize>,
}

#[derive(Clone, Default)]
pub struct Trace {
    sink: Option<Arc<Sink>>,
}

pub struct Started {
    pub trace: Trace,
    pub writer: std::thread::JoinHandle<()>,
}

#[derive(Debug, thiserror::Error)]
pub enum StartTraceError {
    #[error("create the engine trace folder")]
    CreateFolder(#[source] artisan_paths::private::PrivateFileError),
    #[error("create the engine trace {path}")]
    CreateFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("start the writer for the engine trace {path}")]
    SpawnWriter {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl Trace {
    pub fn start(dir: &Path, name: &str, secrets: Secrets) -> Result<Started, StartTraceError> {
        artisan_paths::private::create_folder(dir).map_err(StartTraceError::CreateFolder)?;
        let path = dir.join(format!("{name}-{}.jsonl", artisan_api::now()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options
            .open(&path)
            .map_err(|source| StartTraceError::CreateFile {
                path: path.clone(),
                source,
            })?;
        let (sender, receiver) = sync_channel::<Record>(32);
        let dropped = Arc::new(AtomicUsize::new(0));
        let sink = Arc::new(Sink {
            secrets,
            sender,
            remaining: AtomicUsize::new(MAX_TRACE_BYTES),
            dropped: Arc::clone(&dropped),
        });
        let writer_path = path.clone();
        let writer = std::thread::Builder::new()
            .name("engine-trace".into())
            .spawn(move || write_records(file, &receiver, &dropped, &writer_path))
            .map_err(|source| StartTraceError::SpawnWriter { path, source })?;
        Ok(Started {
            trace: Self { sink: Some(sink) },
            writer,
        })
    }

    pub fn record(&self, kind: Kind, text: &str) {
        let Some(sink) = &self.sink else { return };
        let text = Text::bounded(&sink.secrets.redact(text));
        let bytes = text
            .kept()
            .len()
            .saturating_mul(6)
            .saturating_add(RECORD_OVERHEAD_BYTES);
        if sink
            .remaining
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(bytes)
            })
            .is_err()
        {
            sink.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let record = Record {
            kind,
            at: artisan_api::now(),
            text,
        };
        if sink.sender.try_send(record).is_err() {
            sink.remaining.fetch_add(bytes, Ordering::Relaxed);
            sink.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn write_line(file: &mut impl std::io::Write, value: &serde_json::Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *file, value)?;
    writeln!(file)
}

fn write_records(
    file: std::fs::File,
    receiver: &Receiver<Record>,
    dropped: &AtomicUsize,
    path: &Path,
) {
    let mut file = std::io::BufWriter::new(file);
    let mut written = || -> std::io::Result<()> {
        while let Ok(record) = receiver.recv() {
            let lost = dropped.swap(0, Ordering::Relaxed);
            if lost > 0 {
                write_line(
                    &mut file,
                    &serde_json::json!({ "at": record.at, "kind": "dropped", "records": lost }),
                )?;
            }
            write_line(&mut file, &record.json())?;
            if matches!(record.kind, Kind::Note) {
                file.flush()?;
            }
        }
        let lost = dropped.swap(0, Ordering::Relaxed);
        write_line(
            &mut file,
            &serde_json::json!({ "at": artisan_api::now(), "kind": "end", "dropped": lost }),
        )?;
        file.flush()
    };
    if let Err(failure) = written() {
        let error: &dyn std::error::Error = &failure;
        tracing::warn!(
            path = %path.display(),
            error,
            "the engine trace stopped writing"
        );
    }
}
