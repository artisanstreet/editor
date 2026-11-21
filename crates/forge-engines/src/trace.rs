use std::collections::{BTreeSet, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, LazyLock, Mutex, PoisonError,
    atomic::{AtomicU64, AtomicUsize, Ordering},
    mpsc::{Receiver, SyncSender, sync_channel},
};

const MAX_TRACE_BYTES: usize = 64 * 1024 * 1024;
const RETAINED_BYTES: u64 = 256 * 1024 * 1024;
const RETAINED_AGE: std::time::Duration = std::time::Duration::from_hours(7 * 24);
const MAX_RECORD_BYTES: usize = 256 * 1024;
const RECORD_OVERHEAD_BYTES: usize = 256;
const END_LINE_BYTES: usize = 128;

static FOLDERS: LazyLock<Mutex<HashMap<PathBuf, Arc<Folder>>>> = LazyLock::new(Mutex::default);

#[derive(Debug, Default)]
struct Folder {
    live: Mutex<BTreeSet<Arc<str>>>,
    completed: AtomicU64,
    held_live: AtomicU64,
}

impl Folder {
    fn of(dir: &Path) -> Arc<Self> {
        Arc::clone(
            FOLDERS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(dir.to_path_buf())
                .or_default(),
        )
    }

    fn live(&self) -> std::sync::MutexGuard<'_, BTreeSet<Arc<str>>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn admit(&self, bytes: u64) -> bool {
        self.held_live
            .try_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                let held = held.checked_add(bytes)?;
                (held.saturating_add(self.completed.load(Ordering::Acquire)) <= RETAINED_BYTES)
                    .then_some(held)
            })
            .is_ok()
    }

    fn release(&self, bytes: u64) {
        self.held_live.fetch_sub(bytes, Ordering::AcqRel);
    }
}

#[derive(Debug)]
struct Account {
    folder: Arc<Folder>,
    name: Arc<str>,
    remaining: AtomicUsize,
    held: AtomicU64,
}

impl Account {
    fn reserve(&self, bytes: usize) -> bool {
        if self
            .remaining
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(bytes)
            })
            .is_err()
        {
            return false;
        }
        if !self.folder.admit(bytes as u64) {
            self.remaining.fetch_add(bytes, Ordering::Relaxed);
            return false;
        }
        self.held.fetch_add(bytes as u64, Ordering::AcqRel);
        true
    }

    fn refund(&self, bytes: usize) {
        self.remaining.fetch_add(bytes, Ordering::Relaxed);
        self.unhold(bytes);
    }

    fn settle(&self, reserved: usize, written: usize) {
        self.unhold(reserved.saturating_sub(written));
    }

    fn unhold(&self, bytes: usize) {
        self.held.fetch_sub(bytes as u64, Ordering::AcqRel);
        self.folder.release(bytes as u64);
    }

    fn finish(&self) {
        let mut live = self.folder.live();
        let held = self.held.swap(0, Ordering::AcqRel);
        self.folder.completed.fetch_add(held, Ordering::AcqRel);
        self.folder.release(held);
        live.remove(&self.name);
    }
}

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
    reserved: usize,
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
const SWITCH_NAMES: [&str; 2] = ["NODE_TLS_REJECT_UNAUTHORIZED", "TOKENIZERS_PARALLELISM"];
const SWITCH_SUFFIXES: [&str; 2] = ["_ENABLED", "_DISABLED"];
const REDACTED: &str = "[redacted]";
const HELD_STREAM_BYTES: usize = 16 * 1024;

fn names_a_credential(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    let switch = SWITCH_NAMES.contains(&key.as_str())
        || SWITCH_SUFFIXES.iter().any(|suffix| key.ends_with(suffix));
    !switch
        && (key == "OPENCODE_CONFIG_CONTENT"
            || SECRET_ENV_MARKERS.iter().any(|marker| key.contains(marker)))
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
            .filter(|(key, _)| names_a_credential(key))
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
    account: Arc<Account>,
    secrets: Secrets,
    sender: SyncSender<Record>,
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
    #[error(
        "the engine traces in {} still being written fill the {} MiB they may take together",
        dir.display(),
        RETAINED_BYTES / (1024 * 1024)
    )]
    NoRoom { dir: PathBuf },
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
    let folder = Folder::of(dir);
    let live = folder.live();
    prune_within(dir, &folder, &live, now, RETAINED_BYTES)
}

fn prune_within(
    dir: &Path,
    folder: &Folder,
    live: &BTreeSet<Arc<str>>,
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
        let is_trace = path
            .extension()
            .is_some_and(|extension| extension == "jsonl");
        let is_live = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| live.contains(name));
        if !is_trace || is_live {
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
    let for_completed = budget.saturating_sub(folder.held_live.load(Ordering::Acquire));
    let Kept { doomed, bytes } = expired(traces, now, for_completed);
    folder.completed.store(
        bytes.saturating_add(doomed.iter().map(|trace| trace.bytes).sum()),
        Ordering::Release,
    );
    for trace in &doomed {
        match std::fs::remove_file(&trace.path) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(PruneTracesError::Remove {
                    path: trace.path.clone(),
                    source,
                });
            }
        }
        folder.completed.fetch_sub(trace.bytes, Ordering::AcqRel);
    }
    Ok(doomed.len())
}

struct Kept {
    doomed: Vec<Listed>,
    bytes: u64,
}

fn expired(mut traces: Vec<Listed>, now: std::time::SystemTime, budget: u64) -> Kept {
    traces.sort_by_key(|trace| std::cmp::Reverse(trace.modified));
    let oldest = now.checked_sub(RETAINED_AGE);
    let mut total: u64 = 0;
    let mut full = false;
    let mut kept: u64 = 0;
    let doomed = traces
        .into_iter()
        .filter_map(|trace| {
            total = total.saturating_add(trace.bytes);
            full = full || total > budget;
            let stale = oldest.is_some_and(|oldest| trace.modified < oldest);
            if full || stale {
                Some(trace)
            } else {
                kept = kept.saturating_add(trace.bytes);
                None
            }
        })
        .collect();
    Kept {
        doomed,
        bytes: kept,
    }
}

impl Trace {
    pub fn start(dir: &Path, name: &str, secrets: Secrets) -> Result<Started, StartTraceError> {
        artisan_paths::private::create_folder(dir).map_err(StartTraceError::CreateFolder)?;
        let folder = Folder::of(dir);
        let file_name: Arc<str> = Arc::from(format!("{name}-{}.jsonl", artisan_api::now()));
        {
            let mut live = folder.live();
            let room = RETAINED_BYTES.saturating_sub(MAX_TRACE_BYTES as u64);
            if let Err(failure) =
                prune_within(dir, &folder, &live, std::time::SystemTime::now(), room)
            {
                let error: &dyn std::error::Error = &failure;
                tracing::warn!(
                    error,
                    "older engine traces stay until the next trace starts"
                );
            }
            live.insert(Arc::clone(&file_name));
        }
        let account = Arc::new(Account {
            folder,
            name: Arc::clone(&file_name),
            remaining: AtomicUsize::new(MAX_TRACE_BYTES),
            held: AtomicU64::new(0),
        });
        let path = dir.join(&*file_name);
        let opened = if account.reserve(END_LINE_BYTES) {
            open_trace(&path)
        } else {
            Err(StartTraceError::NoRoom {
                dir: dir.to_path_buf(),
            })
        };
        let file = opened.inspect_err(|_| account.finish())?;
        let (sender, receiver) = sync_channel::<Record>(32);
        let dropped = Arc::new(AtomicUsize::new(0));
        let sink = Arc::new(Sink {
            account: Arc::clone(&account),
            secrets,
            sender,
            dropped: Arc::clone(&dropped),
        });
        let writer_path = path.clone();
        let writer_account = Arc::clone(&account);
        let writer = std::thread::Builder::new()
            .name("engine-trace".into())
            .spawn(move || {
                write_records(file, &receiver, &dropped, &writer_account, &writer_path);
            })
            .map_err(|source| {
                account.finish();
                StartTraceError::SpawnWriter { path, source }
            })?;
        Ok(Started {
            trace: Self { sink: Some(sink) },
            writer,
        })
    }

    #[must_use]
    pub(crate) fn name(&self) -> Option<Arc<str>> {
        self.sink
            .as_ref()
            .map(|sink| Arc::clone(&sink.account.name))
    }

    #[must_use]
    pub fn stream(&self, kind: Kind) -> Stream {
        Stream {
            trace: self.clone(),
            kind,
            held: Vec::new(),
            inline: Inline::Text,
        }
    }

    pub fn record(&self, kind: Kind, text: &str) {
        let Some(sink) = &self.sink else { return };
        let text = Text::bounded(&sink.secrets.redact(&without_inline_payloads(text)));
        let reserved = text
            .kept()
            .len()
            .saturating_mul(6)
            .saturating_add(RECORD_OVERHEAD_BYTES);
        if !sink.account.reserve(reserved) {
            sink.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let record = Record {
            kind,
            at: artisan_api::now(),
            text,
            reserved,
        };
        if sink.sender.try_send(record).is_err() {
            sink.account.refund(reserved);
            sink.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

const INLINE_MARK: &str = ";base64,";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Inline {
    #[default]
    Text,
    Payload,
}

fn payload_len(text: &str) -> usize {
    text.bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
        .count()
}

fn without_inline_payloads(text: &str) -> std::borrow::Cow<'_, str> {
    elide_inline_payloads(Inline::Text, text).0
}

fn elide_inline_payloads(state: Inline, text: &str) -> (std::borrow::Cow<'_, str>, Inline) {
    if state == Inline::Text && !text.contains(INLINE_MARK) {
        return (std::borrow::Cow::Borrowed(text), Inline::Text);
    }
    let mut kept = String::with_capacity(text.len().min(MAX_RECORD_BYTES));
    let mut rest = text;
    let mut state = state;
    if state == Inline::Payload {
        let payload = payload_len(rest);
        if payload > 0 {
            kept.push('<');
            kept.push_str(&payload.to_string());
            kept.push_str(" more bytes>");
        }
        rest = rest.split_at(payload).1;
        if !rest.is_empty() {
            state = Inline::Text;
        }
    }
    while let Some((before, after)) = rest.split_once(INLINE_MARK) {
        let payload = payload_len(after);
        kept.push_str(before);
        kept.push_str(INLINE_MARK);
        if payload > 0 {
            kept.push('<');
            kept.push_str(&payload.to_string());
            kept.push_str(" bytes>");
        }
        rest = after.split_at(payload).1;
        state = if rest.is_empty() {
            Inline::Payload
        } else {
            Inline::Text
        };
    }
    kept.push_str(rest);
    (std::borrow::Cow::Owned(kept), state)
}

fn clear_of_inline_mark(bytes: &[u8], cut: usize) -> usize {
    let mark = INLINE_MARK.as_bytes();
    (cut.saturating_sub(mark.len() - 1)..cut)
        .find(|&start| mark.starts_with(&bytes[start..cut]))
        .unwrap_or(cut)
}

#[derive(Debug)]
pub struct Stream {
    trace: Trace,
    kind: Kind,
    held: Vec<u8>,
    inline: Inline,
}

impl Stream {
    pub fn write(&mut self, bytes: &[u8]) {
        let Some(sink) = &self.trace.sink else { return };
        self.held.extend_from_slice(bytes);
        let cut = char_boundary(&self.held, frame_end(&self.held));
        let cut = clear_of_inline_mark(&self.held, cut);
        let cut = sink.secrets.clear_of_secrets(&self.held, cut);
        if cut > 0 {
            let framed: Vec<u8> = self.held.drain(..cut).collect();
            self.emit(&framed);
        }
    }

    fn emit(&mut self, framed: &[u8]) {
        let text = String::from_utf8_lossy(framed);
        let (kept, inline) = elide_inline_payloads(self.inline, &text);
        self.inline = inline;
        self.trace.record(self.kind, &kept);
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if !self.held.is_empty() {
            let rest = std::mem::take(&mut self.held);
            self.emit(&rest);
        }
    }
}

fn open_trace(path: &Path) -> Result<std::fs::File, StartTraceError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(path)
        .map_err(|source| StartTraceError::CreateFile {
            path: path.to_path_buf(),
            source,
        })
}

fn write_line(file: &mut impl std::io::Write, value: &serde_json::Value) -> std::io::Result<usize> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    file.write_all(&line)?;
    Ok(line.len())
}

fn write_records(
    file: std::fs::File,
    receiver: &Receiver<Record>,
    dropped: &AtomicUsize,
    account: &Account,
    path: &Path,
) {
    let mut file = std::io::BufWriter::new(file);
    let mut written = || -> std::io::Result<()> {
        while let Ok(record) = receiver.recv() {
            let lost = dropped.swap(0, Ordering::Relaxed);
            let marker = if lost > 0 {
                write_line(
                    &mut file,
                    &serde_json::json!({ "at": record.at, "kind": "dropped", "records": lost }),
                )
            } else {
                Ok(0)
            };
            let line = marker.and_then(|marker| {
                write_line(&mut file, &record.json()).map(|line| marker.saturating_add(line))
            });
            account.settle(record.reserved, *line.as_ref().unwrap_or(&record.reserved));
            line?;
            if matches!(record.kind, Kind::Note) {
                file.flush()?;
            }
        }
        let lost = dropped.swap(0, Ordering::Relaxed);
        let end = write_line(
            &mut file,
            &serde_json::json!({ "at": artisan_api::now(), "kind": "end", "dropped": lost }),
        );
        account.settle(END_LINE_BYTES, *end.as_ref().unwrap_or(&END_LINE_BYTES));
        end?;
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
    account.finish();
}
