use crate::Hub;
use anyhow::Context as _;
use artisan_api::{ItemId, Thread, ThreadId};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub const MAX_RESOURCE_BYTES: u64 = 128 * 1024 * 1024;

const SNAPSHOTS_PER_ITEM: usize = 32;
const KERNEL_RELEASE: &str = "/proc/sys/kernel/osrelease";
const WSL_CONFIG: &str = "/etc/wsl.conf";
const WSL_DRIVES: &str = "/mnt";
const BLOCK_PREFIX: &str = "artisan-";

static HOST: LazyLock<Host> = LazyLock::new(Host::detect);

#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    #[error("resource not found")]
    Missing,
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
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
    pub fn parse(source: &str) -> Option<Self> {
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
pub fn written_sources(markdown: &str) -> Vec<Written> {
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
    pub home: Option<PathBuf>,
    pub windows_drives: Option<PathBuf>,
}

#[must_use]
pub fn automount_root(config: &str) -> PathBuf {
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
    let url = url::Url::parse(path).map_err(|error| ResourceError::Refused(error.to_string()))?;
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

fn wsl_share(path: &str) -> Option<&str> {
    let mut parts = path
        .strip_prefix("\\\\")
        .or_else(|| path.strip_prefix("//"))?
        .splitn(3, ['\\', '/']);
    let host = parts.next()?;
    let _distribution = parts.next()?;
    let rest = parts.next()?;
    (host.eq_ignore_ascii_case("wsl.localhost") || host.eq_ignore_ascii_case("wsl$"))
        .then_some(rest)
}

fn inside_wsl(path: &str, drives: &Path) -> Option<PathBuf> {
    let forward = |rest: &str| rest.replace('\\', "/");
    if is_drive(path) {
        let letter = path[..1].to_ascii_lowercase();
        return Some(drives.join(letter).join(forward(&path[3..])));
    }
    wsl_share(path).map(|rest| Path::new("/").join(forward(rest)))
}

pub fn host_path(written: &Written, folder: &Path, host: &Host) -> Result<PathBuf, ResourceError> {
    let local = without_scheme(&written.path)?;
    let path = host
        .windows_drives
        .as_deref()
        .and_then(|drives| inside_wsl(&local, drives))
        .unwrap_or_else(|| PathBuf::from(&local));
    if written.from_home {
        return host
            .home
            .as_ref()
            .map(|home| home.join(&path))
            .ok_or_else(|| ResourceError::Refused("Forge has no home directory".to_owned()));
    }
    Ok(if path.is_absolute() {
        path
    } else {
        folder.join(path)
    })
}

fn read_regular(path: &Path) -> Result<Vec<u8>, ResourceError> {
    let missing = |error: std::io::Error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ResourceError::Missing
        } else {
            ResourceError::Failed(
                anyhow::Error::from(error).context(format!("read resource {}", path.display())),
            )
        }
    };
    let too_large = || {
        ResourceError::Refused("resource must be a regular file no larger than 128 MiB".to_owned())
    };
    let metadata = std::fs::metadata(path).map_err(missing)?;
    if !metadata.is_file() || metadata.len() > MAX_RESOURCE_BYTES {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(missing)?
        .take(MAX_RESOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(missing)?;
    if bytes.len() as u64 > MAX_RESOURCE_BYTES {
        Err(too_large())
    } else {
        Ok(bytes)
    }
}

impl Hub {
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
        let folder = self.thread_folder(thread).await?;
        let live = host_path(written, &folder, &HOST);
        tokio::task::spawn_blocking(move || {
            if let Some(kept) = snapshot.as_deref().filter(|kept| kept.is_file()) {
                return read_regular(kept);
            }
            let bytes = read_regular(&live?)?;
            if let Some(target) = &snapshot
                && let Err(error) = forge_store::keep_resource_snapshot(target, &bytes)
            {
                tracing::warn!(path = %target.display(), "keep resource snapshot: {error:#}");
            }
            Ok(bytes)
        })
        .await
        .context("resource read task")?
    }

    pub(crate) fn snapshot_resources(
        &self,
        thread_id: &ThreadId,
        item_id: &ItemId,
        shown: Vec<Written>,
    ) {
        if shown.is_empty() {
            return;
        }
        let (hub, thread_id, item_id) = (self.clone(), thread_id.clone(), item_id.clone());
        self.inner.tasks.spawn(async move {
            let Ok(Some(thread)) = hub.store().thread(&thread_id).await else {
                return;
            };
            for written in shown.iter().take(SNAPSHOTS_PER_ITEM) {
                if let Err(error) = hub.read_resource(&thread, Some(&item_id), written).await {
                    tracing::debug!(
                        thread = %thread_id,
                        item = %item_id,
                        path = %written.path,
                        "resource not kept: {error:#}"
                    );
                }
            }
        });
    }
}
