use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc::{Receiver, SyncSender, sync_channel},
};

const MAX_TRACE_BYTES: usize = 64 * 1024 * 1024;
const RETAINED_BYTES: u64 = 256 * 1024 * 1024;
const RETAINED_AGE: std::time::Duration = std::time::Duration::from_hours(7 * 24);
const MAX_RECORD_BYTES: usize = 256 * 1024;
const RECORD_OVERHEAD_BYTES: usize = 128;

#[derive(Debug, Clone, Copy)]
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
pub struct Secrets(Arc<[Secret]>);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Secret {
    Anywhere(String),
    WholeToken(String),
}

impl Secret {
    fn of(text: &str) -> Option<Self> {
        match text.len() {
            0 => None,
            length if length < WHOLE_TOKEN_BELOW_BYTES => Some(Self::WholeToken(text.to_owned())),
            _ => Some(Self::Anywhere(text.to_owned())),
        }
    }

    fn text(&self) -> &str {
        match self {
            Self::Anywhere(text) | Self::WholeToken(text) => text,
        }
    }

    fn escaped(&self) -> Self {
        let quoted = serde_json::Value::from(self.text()).to_string();
        let escaped = quoted
            .strip_prefix('"')
            .and_then(|quoted| quoted.strip_suffix('"'))
            .expect("invariant: a JSON string is quoted")
            .to_owned();
        match self {
            Self::Anywhere(_) => Self::Anywhere(escaped),
            Self::WholeToken(_) => Self::WholeToken(escaped),
        }
    }

    fn redact(&self, text: &str) -> String {
        match self {
            Self::Anywhere(secret) => text.replace(secret.as_str(), REDACTED),
            Self::WholeToken(secret) => {
                let bytes = text.as_bytes();
                let mut redacted = String::with_capacity(text.len());
                let mut kept = 0;
                for (start, _) in text.match_indices(secret.as_str()) {
                    let end = start + secret.len();
                    let bounded = start
                        .checked_sub(1)
                        .is_none_or(|before| !is_word(bytes[before]))
                        && bytes.get(end).is_none_or(|after| !is_word(*after));
                    if bounded && start >= kept {
                        redacted.push_str(
                            text.get(kept..start)
                                .expect("invariant: matches start and end on char boundaries"),
                        );
                        redacted.push_str(REDACTED);
                        kept = end;
                    }
                }
                redacted.push_str(
                    text.get(kept..)
                        .expect("invariant: a match ends on a char boundary"),
                );
                redacted
            }
        }
    }
}

const fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Secrets").field(&self.0.len()).finish()
    }
}

const WHOLE_TOKEN_BELOW_BYTES: usize = 8;
const SECRET_ENV_MARKERS: [&str; 7] = [
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "AUTH",
    "CREDENTIAL",
    "COOKIE",
];
const SWITCH_VALUES: [&str; 8] = ["0", "1", "true", "false", "yes", "no", "on", "off"];
const REDACTED: &str = "[redacted]";
const HELD_STREAM_BYTES: usize = 16 * 1024;

fn names_a_credential(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    key == "OPENCODE_CONFIG_CONTENT" || SECRET_ENV_MARKERS.iter().any(|marker| key.contains(marker))
}

fn is_switch(value: &str) -> bool {
    SWITCH_VALUES
        .iter()
        .any(|switch| value.trim().eq_ignore_ascii_case(switch))
}

#[must_use]
pub(crate) fn effective_environment(command: &std::process::Command) -> Vec<(String, String)> {
    let mut environment: std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> =
        std::env::vars_os().collect();
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => {
                environment.insert(key.to_owned(), value.to_owned());
            }
            None => {
                environment.remove(key);
            }
        }
    }
    environment
        .into_iter()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect()
}

impl Secrets {
    #[must_use]
    pub fn of(launch: &crate::Launch, environment: &[(String, String)]) -> Self {
        let configured = launch.mcp_servers.iter().flat_map(|server| {
            server
                .url
                .iter()
                .chain(server.headers.values())
                .chain(server.env.values())
                .map(String::as_str)
        });
        let gateway = launch
            .gateway_mcp
            .iter()
            .flat_map(|(_, url)| std::iter::once(url.as_str()).chain(url.rsplit('/').next()));
        let inherited = environment
            .iter()
            .chain(&launch.env)
            .filter(|(key, value)| names_a_credential(key) && !is_switch(value))
            .map(|(_, value)| value.as_str());
        let mut secrets: Vec<Secret> = configured
            .chain(gateway)
            .chain(inherited)
            .filter_map(Secret::of)
            .flat_map(|secret| {
                let escaped = secret.escaped();
                [secret, escaped]
            })
            .collect();
        secrets.sort_by(|left, right| {
            right
                .text()
                .len()
                .cmp(&left.text().len())
                .then_with(|| left.cmp(right))
        });
        secrets.dedup();
        Self(secrets.into())
    }

    fn redact(&self, text: &str) -> String {
        self.0
            .iter()
            .fold(text.to_owned(), |text, secret| secret.redact(&text))
    }

    fn clear_of_secrets(&self, bytes: &[u8], mut cut: usize) -> usize {
        loop {
            let straddling = self
                .0
                .iter()
                .filter_map(|secret| {
                    let secret = secret.text().as_bytes();
                    (cut.saturating_sub(secret.len())..cut).find(|&start| {
                        let seen = &bytes[start..bytes.len().min(start + secret.len())];
                        secret.starts_with(seen)
                    })
                })
                .min();
            match straddling {
                Some(start) => cut = start,
                None => return cut,
            }
        }
    }
}

const fn utf8_width(lead: u8) -> usize {
    match lead.leading_ones() {
        2 => 2,
        3 => 3,
        4 => 4,
        _ => 1,
    }
}

fn char_boundary(bytes: &[u8], cut: usize) -> usize {
    (cut.saturating_sub(3)..cut)
        .rev()
        .find(|&at| bytes[at].leading_ones() != 1)
        .filter(|&lead| lead + utf8_width(bytes[lead]) > cut)
        .unwrap_or(cut)
}

fn frame_end(bytes: &[u8]) -> usize {
    if bytes.len() >= HELD_STREAM_BYTES {
        return bytes.len();
    }
    bytes
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map_or(0, |newline| newline + 1)
}

#[derive(Debug)]
struct Sink {
    name: Arc<str>,
    secrets: Secrets,
    sender: SyncSender<Record>,
    remaining: AtomicUsize,
    dropped: Arc<AtomicUsize>,
}

#[derive(Debug, Clone, Default)]
pub struct Trace {
    sink: Option<Arc<Sink>>,
}

#[derive(Debug)]
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

#[derive(Debug, thiserror::Error)]
pub enum PruneTracesError {
    #[error("list the engine traces in {path}")]
    ReadFolder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read the size and age of the engine trace {path}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("delete the engine trace {path}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[must_use]
pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join("traces")
}

#[derive(Debug)]
struct Listed {
    path: PathBuf,
    bytes: u64,
    modified: std::time::SystemTime,
}

pub fn prune(dir: &Path, now: std::time::SystemTime) -> Result<usize, PruneTracesError> {
    prune_within(dir, now, RETAINED_BYTES)
}

fn prune_within(
    dir: &Path,
    now: std::time::SystemTime,
    budget: u64,
) -> Result<usize, PruneTracesError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(source) => {
            return Err(PruneTracesError::ReadFolder {
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    let mut traces = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| PruneTracesError::ReadFolder {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|extension| extension != "jsonl")
        {
            continue;
        }
        let metadata = match entry.metadata().and_then(|metadata| {
            metadata
                .modified()
                .map(|modified| (metadata.is_file(), metadata.len(), modified))
        }) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => return Err(PruneTracesError::Inspect { path, source }),
        };
        if let (true, bytes, modified) = metadata {
            traces.push(Listed {
                path,
                bytes,
                modified,
            });
        }
    }
    let doomed = expired(traces, now, budget);
    for path in &doomed {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(PruneTracesError::Remove {
                    path: path.clone(),
                    source,
                });
            }
        }
    }
    Ok(doomed.len())
}

fn expired(mut traces: Vec<Listed>, now: std::time::SystemTime, budget: u64) -> Vec<PathBuf> {
    traces.sort_by_key(|trace| std::cmp::Reverse(trace.modified));
    let oldest = now.checked_sub(RETAINED_AGE);
    let mut total: u64 = 0;
    let mut full = false;
    traces
        .into_iter()
        .filter_map(|trace| {
            total = total.saturating_add(trace.bytes);
            full = full || total > budget;
            let stale = oldest.is_some_and(|oldest| trace.modified < oldest);
            (full || stale).then_some(trace.path)
        })
        .collect()
}

impl Trace {
    pub fn start(dir: &Path, name: &str, secrets: Secrets) -> Result<Started, StartTraceError> {
        artisan_paths::private::create_folder(dir).map_err(StartTraceError::CreateFolder)?;
        let room = RETAINED_BYTES.saturating_sub(MAX_TRACE_BYTES as u64);
        if let Err(failure) = prune_within(dir, std::time::SystemTime::now(), room) {
            let error: &dyn std::error::Error = &failure;
            tracing::warn!(
                error,
                "older engine traces stay until the next trace starts"
            );
        }
        let file_name: Arc<str> = Arc::from(format!("{name}-{}.jsonl", artisan_api::now()));
        let path = dir.join(&*file_name);
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
            name: file_name,
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

    #[must_use]
    pub(crate) fn name(&self) -> Option<Arc<str>> {
        self.sink.as_ref().map(|sink| Arc::clone(&sink.name))
    }

    #[must_use]
    pub fn stream(&self, kind: Kind) -> Stream {
        Stream {
            trace: self.clone(),
            kind,
            held: Vec::new(),
        }
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
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
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

#[derive(Debug)]
pub struct Stream {
    trace: Trace,
    kind: Kind,
    held: Vec<u8>,
}

impl Stream {
    pub fn write(&mut self, bytes: &[u8]) {
        let Some(sink) = &self.trace.sink else { return };
        self.held.extend_from_slice(bytes);
        let cut = char_boundary(&self.held, frame_end(&self.held));
        let cut = sink.secrets.clear_of_secrets(&self.held, cut);
        if cut > 0 {
            let framed: Vec<u8> = self.held.drain(..cut).collect();
            self.trace
                .record(self.kind, &String::from_utf8_lossy(&framed));
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if !self.held.is_empty() {
            let rest = std::mem::take(&mut self.held);
            self.trace
                .record(self.kind, &String::from_utf8_lossy(&rest));
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
