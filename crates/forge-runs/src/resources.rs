use crate::Hub;
use crate::beneath::{self, Directory, OpenBeneathError, OpenRootError, Placed, Relative};
use crate::scratch::ThreadFolderError;
use artisan_api::{EngineId, ItemId, Thread, ThreadId};
use forge_engines::claudeai::{ReadRefusal, Rules};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

const MAX_RESOURCE_BYTES: u64 = artisan_api::TUNNEL_BODY_MAX_BYTES as u64;

const SNAPSHOTS_PER_ITEM: usize = 32;
const KERNEL_RELEASE: &str = "/proc/sys/kernel/osrelease";
const WSL_CONFIG: &str = "/etc/wsl.conf";
const WSL_DRIVES: &str = "/mnt";
const BLOCK_PREFIX: &str = "artisan-";
const WSL_DISTRIBUTION: &str = "WSL_DISTRO_NAME";

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
    #[error("{} is not a regular file", path.display())]
    NotRegular { path: PathBuf },
    #[error("{} is {bytes} bytes, more than the {MAX_RESOURCE_BYTES} an Editor can carry", path.display())]
    TooLarge { path: PathBuf, bytes: u64 },
    #[error("{} is outside the thread's folder", path.display())]
    Outside { path: PathBuf },
    #[error("{} isn't an image, and only images are shown from outside the thread's folder", path.display())]
    NotAnImage { path: PathBuf },
    #[error("{} is reached through a symbolic link", path.display())]
    Linked {
        path: PathBuf,
        #[source]
        source: OpenBeneathError,
    },
    #[error("open the thread's folder to read resources beneath it")]
    Root {
        #[source]
        source: Arc<RootError>,
    },
    #[error("open resource {}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: OpenBeneathError,
    },
    #[error("{} is not readable on the agent's behalf", path.display())]
    Refused {
        path: PathBuf,
        #[source]
        source: ReadRefusal,
    },
    #[error("read resource {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("find the folder resources are read from")]
    Folder(#[source] ThreadFolderError),
    #[error("find out whether the reply keeps its files when first viewed")]
    KeptAtView(#[source] forge_store::KeptAtViewError),
    #[error("run the resource read task of thread {thread}")]
    Task {
        thread: ThreadId,
        #[source]
        source: tokio::task::JoinError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RootError {
    #[error("resolve the thread's folder {}", folder.display())]
    Resolve {
        folder: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("open the thread's folder {} without following links", resolved.display())]
    Open {
        resolved: PathBuf,
        #[source]
        source: OpenRootError,
    },
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
                if let Some(body) = block.take() {
                    match serde_json::from_str::<serde_json::Value>(&body) {
                        Ok(value) => block_sources(&value, &mut found),
                        Err(error) => {
                            tracing::debug!(%error, "an artisan block that isn't JSON names no files");
                        }
                    }
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

fn wsl_config() -> String {
    match std::fs::read_to_string(WSL_CONFIG) {
        Ok(config) => config,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            tracing::warn!(path = WSL_CONFIG, %error, "Windows drives are looked for under the default mount root");
            String::new()
        }
    }
}

impl Host {
    fn detect() -> Self {
        let inside_wsl = cfg!(target_os = "linux")
            && std::fs::read_to_string(KERNEL_RELEASE)
                .is_ok_and(|release| release.to_ascii_lowercase().contains("microsoft"));
        Self {
            home: dirs::home_dir(),
            windows_drives: inside_wsl.then(|| automount_root(&wsl_config())),
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
        let (letter, rest) = path.split_at(1);
        let (_, rest) = rest.split_at(2);
        return Ok(Some(
            drives.join(letter.to_ascii_lowercase()).join(forward(rest)),
        ));
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

#[derive(Debug)]
struct Root {
    directory: Directory,
    given: PathBuf,
    resolved: PathBuf,
}

impl Root {
    fn acquire(folder: &Path) -> Result<Self, RootError> {
        let resolved = std::fs::canonicalize(folder)
            .map(|resolved| without_verbatim_prefix(&resolved))
            .map_err(|source| RootError::Resolve {
                folder: folder.to_path_buf(),
                source,
            })?;
        let directory = Directory::open(&resolved).map_err(|source| RootError::Open {
            resolved: resolved.clone(),
            source,
        })?;
        Ok(Self {
            directory,
            given: folder.to_path_buf(),
            resolved,
        })
    }
}

pub(crate) struct Located<'a> {
    root: &'a Root,
    relative: Relative,
    object: PathBuf,
}

pub(crate) struct Access {
    root: Result<Root, Arc<RootError>>,
    rules: Rules,
}

impl Access {
    #[must_use]
    pub(crate) fn load(engine_home: &Path, folder: &Path) -> Self {
        Self {
            root: Root::acquire(folder).map_err(Arc::new),
            rules: Rules::load(engine_home, folder),
        }
    }

    pub(crate) fn locate(&self, live: &Path) -> Result<Located<'_>, ResourceError> {
        let root = self.root.as_ref().map_err(|source| ResourceError::Root {
            source: Arc::clone(source),
        })?;
        let relative = match beneath::place(live, &[&root.given, &root.resolved]) {
            Placed::Beneath(relative) => relative,
            Placed::Root => {
                return Err(ResourceError::NotRegular {
                    path: live.to_path_buf(),
                });
            }
            Placed::Outside => {
                return Err(ResourceError::Outside {
                    path: live.to_path_buf(),
                });
            }
        };
        let object = root.resolved.join(relative.path());
        self.may_read(&object)?;
        Ok(Located {
            root,
            relative,
            object,
        })
    }

    #[cfg_attr(
        not(windows),
        expect(
            clippy::unused_self,
            reason = "only Windows checks the rules again against the opened handle's final path"
        )
    )]
    fn open(&self, located: &Located<'_>) -> Result<std::fs::File, ResourceError> {
        let object = &located.object;
        let file =
            beneath::open_file(&located.root.directory, &located.relative).map_err(|failure| {
                match failure {
                    OpenBeneathError::Linked { .. } => ResourceError::Linked {
                        path: object.clone(),
                        source: failure,
                    },
                    OpenBeneathError::Open { source, .. }
                        if source.kind() == std::io::ErrorKind::NotFound =>
                    {
                        ResourceError::Missing
                    }
                    OpenBeneathError::Open { .. } => ResourceError::Open {
                        path: object.clone(),
                        source: failure,
                    },
                }
            })?;
        #[cfg(windows)]
        {
            let opened = beneath::final_path(&file).map_err(|source| ResourceError::Read {
                path: object.clone(),
                source,
            })?;
            self.may_read(&without_verbatim_prefix(&opened))?;
        }
        Ok(file)
    }

    fn may_read(&self, object: &Path) -> Result<(), ResourceError> {
        self.rules
            .may_read(object)
            .map_err(|source| ResourceError::Refused {
                path: object.to_path_buf(),
                source,
            })
    }

    pub(crate) fn read_located(&self, located: &Located<'_>) -> Result<Vec<u8>, ResourceError> {
        read_regular(self.open(located)?, &located.object)
    }

    pub(crate) fn read(&self, live: &Path) -> Result<Vec<u8>, ResourceError> {
        self.read_located(&self.locate(live)?)
    }

    pub(crate) fn read_shown(&self, live: &Path) -> Result<Vec<u8>, ResourceError> {
        match self.read(live) {
            Err(ResourceError::Outside { .. } | ResourceError::Linked { .. }) => {
                self.read_loose_image(live)
            }
            read => read,
        }
    }

    fn read_loose_image(&self, live: &Path) -> Result<Vec<u8>, ResourceError> {
        let resolved = std::fs::canonicalize(live)
            .map(|resolved| without_verbatim_prefix(&resolved))
            .map_err(|error| io_failure(live, error))?;
        self.may_read(&resolved)?;
        let file = open_kept(&resolved).map_err(|error| io_failure(&resolved, error))?;
        #[cfg(windows)]
        {
            let opened = beneath::final_path(&file).map_err(|source| ResourceError::Read {
                path: resolved.clone(),
                source,
            })?;
            self.may_read(&without_verbatim_prefix(&opened))?;
        }
        let bytes = read_regular(file, &resolved)?;
        if is_image(&bytes) {
            Ok(bytes)
        } else {
            Err(ResourceError::NotAnImage { path: resolved })
        }
    }
}

const ISO_MEDIA_IMAGE_BRANDS: [&[u8; 4]; 10] = [
    b"avif", b"avis", b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1",
];
const BMP_HEADER_SIZES: [u32; 7] = [12, 40, 52, 56, 64, 108, 124];

fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = std::str::from_utf8(head).unwrap_or_else(|error| {
        std::str::from_utf8(&head[..error.valid_up_to()])
            .expect("invariant: the prefix up to valid_up_to is UTF-8")
    });
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with('<') && text.contains("<svg")
}

fn is_image(bytes: &[u8]) -> bool {
    let at = |start: usize, length: usize| bytes.get(start..start + length);
    let le_u16 = |start| at(start, 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let le_u32 = |start| at(start, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.starts_with(b"RIFF") && at(8, 4) == Some(b"WEBP"))
        || (bytes.starts_with(b"BM")
            && le_u32(14).is_some_and(|size| BMP_HEADER_SIZES.contains(&size)))
        || bytes.starts_with(b"II*\0")
        || bytes.starts_with(b"MM\0*")
        || (bytes.starts_with(&[0, 0, 1, 0]) && le_u16(4).is_some_and(|count| count > 0))
        || (at(4, 4) == Some(b"ftyp")
            && at(8, 4).is_some_and(|brand| {
                ISO_MEDIA_IMAGE_BRANDS
                    .iter()
                    .any(|known| known.as_slice() == brand)
            }))
        || is_svg(bytes)
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
fn open_kept(path: &Path) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};
    rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(std::fs::File::from)
    .map_err(std::io::Error::from)
}

#[cfg(windows)]
fn open_kept(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
    std::fs::File::options()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)
}

fn read_kept(path: &Path) -> Result<Vec<u8>, ResourceError> {
    let file = open_kept(path).map_err(|error| io_failure(path, error))?;
    read_regular(file, path)
}

pub(crate) fn read_regular(file: std::fs::File, path: &Path) -> Result<Vec<u8>, ResourceError> {
    let too_large = |bytes| ResourceError::TooLarge {
        path: path.to_path_buf(),
        bytes,
    };
    let metadata = file.metadata().map_err(|error| io_failure(path, error))?;
    if !metadata.is_file() {
        return Err(ResourceError::NotRegular {
            path: path.to_path_buf(),
        });
    }
    if metadata.len() > MAX_RESOURCE_BYTES {
        return Err(too_large(metadata.len()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_RESOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_failure(path, error))?;
    if bytes.len() as u64 > MAX_RESOURCE_BYTES {
        Err(too_large(bytes.len() as u64))
    } else {
        Ok(bytes)
    }
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
    let bytes = access.read_shown(live)?;
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
        let keep_at_view = match item {
            Some(item) => self
                .store()
                .resource_kept_at_view(&thread.id, item)
                .await
                .map_err(ResourceError::KeptAtView)?,
            None => false,
        };
        let folder = self
            .thread_folder(thread)
            .await
            .map_err(ResourceError::Folder)?;
        let live = host_path(written, &folder, &HOST);
        let hub = self.clone();
        tokio::task::spawn_blocking(move || {
            if let Some(kept) = snapshot.as_deref().filter(|kept| kept.is_file()) {
                return read_kept(kept);
            }
            let bytes = hub.resource_access(&folder).read_shown(&live?)?;
            if let Some(target) = snapshot.as_deref().filter(|_| keep_at_view) {
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
