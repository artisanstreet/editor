use crate::Hub;
use crate::error::ErrorClass;
use crate::scratch::ThreadFolderError;
use artisan_api::{EngineId, ItemId, Thread, ThreadId};
use forge_engines::claudeai::Rules;
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

const MAX_RESOURCE_BYTES: u64 = 128 * 1024 * 1024;

const SNAPSHOTS_PER_ITEM: usize = 32;
const KERNEL_RELEASE: &str = "/proc/sys/kernel/osrelease";
const WSL_CONFIG: &str = "/etc/wsl.conf";
const WSL_DRIVES: &str = "/mnt";
const BLOCK_PREFIX: &str = "artisan-";
const WSL_DISTRIBUTION: &str = "WSL_DISTRO_NAME";
const READ_TOOL: &str = "Read";

static HOST: LazyLock<Host> = LazyLock::new(Host::detect);

#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    #[error("resource not found")]
    Missing,
    #[error("{path} is not a file URL Forge can read")]
    InvalidUrl {
        path: String,
        #[source]
        source: url::ParseError,
    },
    #[error("the file is in the WSL distribution {distribution}, which Forge does not run in")]
    OtherDistribution { distribution: String },
    #[error("Forge has no home directory")]
    NoHome,
    #[error("resource must be a regular file no larger than 128 MiB")]
    NotRegular { path: PathBuf },
    #[error("outside the thread's folder or covered by a permission rule")]
    Outside { path: PathBuf },
    #[error("read resource {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("find the folder resources are read from")]
    Folder(#[source] ThreadFolderError),
    #[error("run the resource read task of thread {thread}")]
    Task {
        thread: ThreadId,
        #[source]
        source: tokio::task::JoinError,
    },
}

impl ResourceError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Missing => ErrorClass::NotFound,
            Self::InvalidUrl { .. }
            | Self::OtherDistribution { .. }
            | Self::NoHome
            | Self::NotRegular { .. }
            | Self::Outside { .. } => ErrorClass::BadRequest,
            Self::Read { .. } | Self::Task { .. } => ErrorClass::Internal,
            Self::Folder(source) => source.class(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Written {
    pub path: String,
    pub from_home: bool,
}

fn has_scheme(source: &str, scheme: &str) -> bool {
    source
        .get(..scheme.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(scheme))
}

impl Written {
    #[must_use]
    fn parse(source: &str) -> Option<Self> {
        let source = source.trim();
        let remote = ["http://", "https://", "data:"]
            .iter()
            .any(|scheme| has_scheme(source, scheme));
        if source.is_empty() || remote {
            return None;
        }
        let home = match source {
            "~" => Some(""),
            _ => source
                .strip_prefix("~/")
                .or_else(|| source.strip_prefix("~\\")),
        };
        Some(home.filter(|_| !has_scheme(source, "file://")).map_or_else(
            || Self {
                path: source.to_owned(),
                from_home: false,
            },
            |rest| Self {
                path: rest.to_owned(),
                from_home: true,
            },
        ))
    }
}

fn block_sources(value: &serde_json::Value, found: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                match (key.as_str(), value) {
                    ("src", serde_json::Value::String(source)) => found.push(source.clone()),
                    ("images" | "paths", serde_json::Value::Array(entries)) => {
                        for entry in entries {
                            if let serde_json::Value::String(source) = entry {
                                found.push(source.clone());
                            } else {
                                block_sources(entry, found);
                            }
                        }
                    }
                    (_, nested) => block_sources(nested, found),
                }
            }
        }
        serde_json::Value::Array(entries) => {
            for entry in entries {
                block_sources(entry, found);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

#[must_use]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "only images and fenced blocks carry sources; every other event is prose"
)]
pub(crate) fn written_sources(markdown: &str) -> Vec<Written> {
    let mut found = Vec::new();
    let mut block: Option<String> = None;
    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => found.push(dest_url.into_string()),
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                if info.trim_start().starts_with(BLOCK_PREFIX) =>
            {
                block = Some(String::new());
            }
            Event::Text(text) => {
                if let Some(body) = &mut block {
                    body.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(value) = block
                    .take()
                    .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
                {
                    block_sources(&value, &mut found);
                }
            }
            _ => {}
        }
    }
    let mut written: Vec<Written> = Vec::new();
    for source in found.iter().filter_map(|source| Written::parse(source)) {
        if !written.contains(&source) {
            written.push(source);
        }
    }
    written
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Host {
    pub(crate) home: Option<PathBuf>,
    pub(crate) windows_drives: Option<PathBuf>,
    pub(crate) wsl_distribution: Option<String>,
}

#[must_use]
pub(crate) fn automount_root(config: &str) -> PathBuf {
    let mut automount = false;
    let mut root = None;
    for line in config.lines().map(str::trim) {
        if let Some(section) = line.strip_prefix('[') {
            automount = section
                .trim_end_matches(']')
                .trim()
                .eq_ignore_ascii_case("automount");
        } else if let Some((key, value)) = line.split_once('=')
            && automount
            && key.trim().eq_ignore_ascii_case("root")
        {
            root = Some(value.trim().trim_matches('"').to_owned());
        }
    }
    root.filter(|root| !root.is_empty())
        .map_or_else(|| PathBuf::from(WSL_DRIVES), PathBuf::from)
}

impl Host {
    fn detect() -> Self {
        let inside_wsl = cfg!(target_os = "linux")
            && std::fs::read_to_string(KERNEL_RELEASE)
                .is_ok_and(|release| release.to_ascii_lowercase().contains("microsoft"));
        Self {
            home: dirs::home_dir(),
            windows_drives: inside_wsl
                .then(|| automount_root(&std::fs::read_to_string(WSL_CONFIG).unwrap_or_default())),
            wsl_distribution: std::env::var(WSL_DISTRIBUTION)
                .into_iter()
                .find(|name| inside_wsl && !name.is_empty()),
        }
    }
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escaped = (bytes[at] == b'%')
            .then(|| text.get(at + 1..at + 3))
            .flatten()
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        if let Some(byte) = escaped {
            decoded.push(byte);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

const fn is_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn without_scheme(path: &str) -> Result<String, ResourceError> {
    if !has_scheme(path, "file://") {
        return Ok(path.to_owned());
    }
    let url = url::Url::parse(path).map_err(|source| ResourceError::InvalidUrl {
        path: path.to_owned(),
        source,
    })?;
    let local = percent_decoded(url.path());
    let share = url
        .host_str()
        .filter(|host| !host.is_empty() && *host != "localhost");
    Ok(share.map_or_else(
        || {
            local
                .strip_prefix('/')
                .filter(|rest| is_drive(rest))
                .map_or_else(|| local.clone(), str::to_owned)
        },
        |host| format!("//{host}{local}"),
    ))
}

fn wsl_share(path: &str) -> Option<(&str, &str)> {
    let mut parts = path
        .strip_prefix("\\\\")
        .or_else(|| path.strip_prefix("//"))?
        .splitn(3, ['\\', '/']);
    let host = parts.next()?;
    let distribution = parts.next()?;
    let rest = parts.next()?;
    (host.eq_ignore_ascii_case("wsl.localhost") || host.eq_ignore_ascii_case("wsl$"))
        .then_some((distribution, rest))
}

fn inside_wsl(
    path: &str,
    drives: &Path,
    own: Option<&str>,
) -> Result<Option<PathBuf>, ResourceError> {
    let forward = |rest: &str| rest.replace('\\', "/");
    if is_drive(path) {
        let letter = path[..1].to_ascii_lowercase();
        return Ok(Some(drives.join(letter).join(forward(&path[3..]))));
    }
    match wsl_share(path) {
        None => Ok(None),
        Some((named, rest)) if own.is_some_and(|own| own.eq_ignore_ascii_case(named)) => {
            Ok(Some(Path::new("/").join(forward(rest))))
        }
        Some((named, _)) => Err(ResourceError::OtherDistribution {
            distribution: named.to_owned(),
        }),
    }
}

pub(crate) fn host_path(
    written: &Written,
    folder: &Path,
    host: &Host,
) -> Result<PathBuf, ResourceError> {
    let local = without_scheme(&written.path)?;
    let translated = match host.windows_drives.as_deref() {
        Some(drives) => inside_wsl(&local, drives, host.wsl_distribution.as_deref())?,
        None => None,
    };
    let path = translated.unwrap_or_else(|| PathBuf::from(&local));
    if written.from_home {
        return host
            .home
            .as_ref()
            .map(|home| home.join(&path))
            .ok_or(ResourceError::NoHome);
    }
    Ok(if path.is_absolute() {
        path
    } else {
        folder.join(path)
    })
}

struct Access {
    root: Option<PathBuf>,
    rules: Rules,
}

impl Access {
    #[must_use]
    fn load(engine_home: &Path, folder: &Path) -> Self {
        let root = match std::fs::canonicalize(folder) {
            Ok(root) => Some(without_verbatim_prefix(&root)),
            Err(error) => {
                tracing::warn!(folder = %folder.display(), "resolve the thread folder for resources: {error}");
                None
            }
        };
        Self {
            root,
            rules: Rules::load(engine_home, folder),
        }
    }

    #[must_use]
    fn permits(&self, resolved: &Path) -> bool {
        self.root
            .as_deref()
            .is_some_and(|root| resolved.starts_with(root))
            && !self.rules.hides(READ_TOOL, resolved)
    }
}

fn io_failure(path: &Path, error: std::io::Error) -> ResourceError {
    if error.kind() == std::io::ErrorKind::NotFound {
        ResourceError::Missing
    } else {
        ResourceError::Read {
            path: path.to_path_buf(),
            source: error,
        }
    }
}

#[cfg(unix)]
fn open_resource(path: &Path) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};
    rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(std::fs::File::from)
    .map_err(std::io::Error::from)
}

#[cfg(not(unix))]
fn open_resource(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn read_regular(path: &Path) -> Result<Vec<u8>, ResourceError> {
    let too_large = || ResourceError::NotRegular {
        path: path.to_path_buf(),
    };
    let file = open_resource(path).map_err(|error| io_failure(path, error))?;
    let metadata = file.metadata().map_err(|error| io_failure(path, error))?;
    if !metadata.is_file() || metadata.len() > MAX_RESOURCE_BYTES {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    file.take(MAX_RESOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_failure(path, error))?;
    if bytes.len() as u64 > MAX_RESOURCE_BYTES {
        Err(too_large())
    } else {
        Ok(bytes)
    }
}

fn resolved(live: &Path) -> Result<PathBuf, ResourceError> {
    std::fs::canonicalize(live)
        .map(|path| without_verbatim_prefix(&path))
        .map_err(|error| io_failure(live, error))
}

pub(crate) fn without_verbatim_prefix(path: &Path) -> PathBuf {
    match path.to_str() {
        Some(text) => match (text.strip_prefix(r"\\?\UNC\"), text.strip_prefix(r"\\?\")) {
            (Some(share), _) => PathBuf::from(format!(r"\\{share}")),
            (None, Some(rest)) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
            _ => path.to_path_buf(),
        },
        None => path.to_path_buf(),
    }
}

fn keep(target: &Path, bytes: &[u8]) {
    if let Err(error) = forge_store::keep_resource_snapshot(target, bytes) {
        tracing::warn!(path = %target.display(), "keep resource snapshot: {}", diagnostics::chain(&error));
    }
}

fn snapshot(live: &Path, access: &Access, target: &Path) -> Result<(), ResourceError> {
    let resolved = resolved(live)?;
    if !access.permits(&resolved) {
        return Err(ResourceError::Outside { path: resolved });
    }
    let bytes = read_regular(&resolved)?;
    keep(target, &bytes);
    Ok(())
}

impl Hub {
    fn resource_access(&self, folder: &Path) -> Access {
        Access::load(&self.engines().home(EngineId::Claude), folder)
    }

    pub async fn read_resource(
        &self,
        thread: &Thread,
        item: Option<&ItemId>,
        written: &Written,
    ) -> Result<Vec<u8>, ResourceError> {
        let snapshot = item.and_then(|item| {
            self.store().resource_snapshot_target(
                &thread.id,
                item,
                &written.path,
                written.from_home,
            )
        });
        let folder = self
            .thread_folder(thread)
            .await
            .map_err(ResourceError::Folder)?;
        let live = host_path(written, &folder, &HOST);
        let hub = self.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(kept) = snapshot.as_deref().filter(|kept| kept.is_file()) {
                return read_regular(kept);
            }
            let resolved = resolved(&live?)?;
            let bytes = read_regular(&resolved)?;
            if let Some(target) = &snapshot
                && hub.resource_access(&folder).permits(&resolved)
            {
                keep(target, &bytes);
            }
            Ok(bytes)
        })
        .await
        .map_err(|source| ResourceError::Task {
            thread: thread.id.clone(),
            source,
        })?
    }

    pub(crate) async fn snapshot_resources(
        &self,
        thread_id: &ThreadId,
        item_id: &ItemId,
        shown: Vec<Written>,
    ) {
        if shown.is_empty() {
            return;
        }
        let thread = match self.store().thread(thread_id).await {
            Ok(Some(thread)) => thread,
            Ok(None) => {
                tracing::warn!(thread = %thread_id, item = %item_id, "resources not kept: the thread is gone");
                return;
            }
            Err(error) => {
                tracing::warn!(thread = %thread_id, item = %item_id, "resources not kept: {}", diagnostics::chain(&error));
                return;
            }
        };
        let folder = match self.thread_folder(&thread).await {
            Ok(folder) => folder,
            Err(error) => {
                tracing::warn!(thread = %thread_id, item = %item_id, "resources not kept: {}", diagnostics::chain(&error));
                return;
            }
        };
        let targets: Vec<_> = shown
            .into_iter()
            .take(SNAPSHOTS_PER_ITEM)
            .filter_map(|written| {
                self.store()
                    .resource_snapshot_target(thread_id, item_id, &written.path, written.from_home)
                    .map(|target| (host_path(&written, &folder, &HOST), target))
            })
            .collect();
        let (hub, thread, item) = (self.clone(), thread_id.clone(), item_id.clone());
        let kept = tokio::task::spawn_blocking(move || {
            let access = hub.resource_access(&folder);
            for (live, target) in targets {
                if let Err(error) = live.and_then(|live| snapshot(&live, &access, &target)) {
                    tracing::debug!(%thread, %item, "resource not kept: {}", diagnostics::chain(&error));
                }
            }
        })
        .await;
        if let Err(error) = kept {
            tracing::warn!(thread = %thread_id, item = %item_id, "resource snapshot task: {error}");
        }
    }
}
