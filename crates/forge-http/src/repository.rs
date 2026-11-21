use artisan_api::{BlobRef, ProjectRepository};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, PoisonError},
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::{Mutex, Semaphore};
const ENTRIES: usize = 64;
const AVATAR_BYTES: usize = 512 * 1024;

#[derive(Debug, thiserror::Error)]
pub(crate) enum AvatarError {
    #[error("took longer than {0:?}")]
    Slow(Duration),
    #[error("fetch {url}")]
    Fetch {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("the response is larger than {limit} bytes")]
    TooLarge { limit: usize },
    #[error("read the {host} repository metadata")]
    Metadata {
        host: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

pub(crate) struct Repositories {
    entries: Mutex<HashMap<PathBuf, Arc<Mutex<Observation>>>>,
    slots: Semaphore,
    http: reqwest::Client,
}
#[derive(Default)]
struct Observation {
    touched: Option<Instant>,
    repository: Option<(Instant, Option<ProjectRepository>)>,
    avatar: Option<(Instant, Option<Vec<u8>>)>,
}
impl Repositories {
    pub(crate) fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            entries: Mutex::new(HashMap::new()),
            slots: Semaphore::new(4),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .redirect(reqwest::redirect::Policy::limited(3))
                .user_agent(concat!("Artisan-Forge/", env!("CARGO_PKG_VERSION")))
                .build()?,
        })
    }
    async fn slot(&self) -> tokio::sync::SemaphorePermit<'_> {
        self.slots
            .acquire()
            .await
            .expect("invariant: the repository slots are never closed")
    }
    async fn entry(&self, path: &Path) -> Arc<Mutex<Observation>> {
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries.get(path) {
            return Arc::clone(entry);
        }
        if entries.len() >= ENTRIES {
            let evict = entries
                .iter()
                .filter(|(_, entry)| Arc::strong_count(entry) == 1)
                .min_by_key(|(_, entry)| entry.try_lock().ok().and_then(|entry| entry.touched))
                .map(|(path, _)| path.clone());
            if let Some(evict) = evict {
                entries.remove(&evict);
            }
        }
        let entry = Arc::new(Mutex::new(Observation::default()));
        entries.insert(path.to_owned(), Arc::clone(&entry));
        entry
    }
    async fn refresh(&self, path: &Path, entry: &mut Observation) {
        entry.touched = Some(Instant::now());
        if entry
            .repository
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        let repository = read(path).await;
        if entry.repository.as_ref().map(|(_, repo)| repo) != Some(&repository) {
            entry.avatar = None;
        }
        entry.repository = Some((Instant::now(), repository));
    }
    pub(crate) async fn read(&self, path: &Path) -> Option<ProjectRepository> {
        let _permit = self.slot().await;
        let entry = self.entry(path).await;
        let mut entry = entry.lock().await;
        self.refresh(path, &mut entry).await;
        entry.repository.as_ref().and_then(|(_, repo)| repo.clone())
    }
    pub(crate) async fn avatar(&self, path: &Path) -> Option<Vec<u8>> {
        let _permit = self.slot().await;
        let entry = self.entry(path).await;
        let mut entry = entry.lock().await;
        self.refresh(path, &mut entry).await;
        if let Some((at, avatar)) = &entry.avatar {
            let ttl = if avatar.is_some() {
                Duration::from_secs(300)
            } else {
                Duration::from_secs(5)
            };
            if at.elapsed() < ttl {
                return avatar.clone();
            }
        }
        let avatar = match entry
            .repository
            .as_ref()
            .and_then(|(_, repo)| repo.as_ref())
        {
            Some(repo) => {
                let limit = Duration::from_secs(8);
                let fetched = tokio::time::timeout(limit, owner_avatar(&self.http, repo))
                    .await
                    .unwrap_or(Err(AvatarError::Slow(limit)));
                fetched.unwrap_or_else(|error| {
                    tracing::debug!(
                        "owner avatar for {}: {}",
                        path.display(),
                        diagnostics::chain(&error)
                    );
                    None
                })
            }
            None => None,
        };
        entry.avatar = Some((Instant::now(), avatar.clone()));
        avatar
    }
}

#[derive(Default)]
pub(crate) struct Icons {
    encoded: std::sync::Mutex<HashMap<IconSource, Icon>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum IconSource {
    File {
        path: PathBuf,
        modified: SystemTime,
        len: u64,
    },
    Avatar([u8; 32]),
}

#[derive(Clone)]
pub(crate) enum Icon {
    Blob(BlobRef),
    Unusable,
}

impl Icons {
    pub(crate) fn get(&self, source: &IconSource, store: &forge_store::Store) -> Option<Icon> {
        let known = self
            .encoded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(source)
            .cloned()?;
        match &known {
            Icon::Blob(blob) => store.blob_path(&blob.hash).map(|_| known),
            Icon::Unusable => Some(known),
        }
    }

    pub(crate) fn insert(&self, source: IconSource, icon: Icon) {
        let mut encoded = self.encoded.lock().unwrap_or_else(PoisonError::into_inner);
        if encoded.len() >= ENTRIES * 4 {
            encoded.clear();
        }
        encoded.insert(source, icon);
    }
}

#[derive(Debug, thiserror::Error)]
enum GitError {
    #[error("run git {args} in {path}")]
    Run {
        args: String,
        path: PathBuf,
        #[source]
        source: Box<forge_engines::process::ProbeError>,
    },
    #[error("git {args} in {path} printed text that is not UTF-8")]
    Output {
        args: String,
        path: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
}

async fn git(path: &Path, args: &[&str]) -> Option<String> {
    match run_git(path, args).await {
        Ok(output) => output,
        Err(error) => {
            tracing::debug!("{}", diagnostics::chain(&error));
            None
        }
    }
}

async fn run_git(path: &Path, args: &[&str]) -> Result<Option<String>, GitError> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    let output = forge_engines::process::probe(command, Duration::from_secs(3), 64 * 1024)
        .await
        .map_err(|source| GitError::Run {
            args: args.join(" "),
            path: path.to_owned(),
            source: Box::new(source),
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8(output.stdout).map_err(|source| GitError::Output {
        args: args.join(" "),
        path: path.to_owned(),
        source,
    })?;
    Ok(Some(text.trim().to_owned()))
}
async fn read(path: &Path) -> Option<ProjectRepository> {
    let observation = git(
        path,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
            "--show-toplevel",
        ],
    )
    .await?;
    let fields: Vec<_> = observation.lines().collect();
    let [git_dir, common, top] = fields.as_slice() else {
        return None;
    };
    let head = Path::new(git_dir).join("HEAD");
    let branch = match head_branch(&head).await {
        Ok(branch) => branch,
        Err(error) => {
            tracing::debug!(path = %head.display(), %error, "read the repository HEAD");
            None
        }
    };
    let remotes = git(path, &["remote"]).await.unwrap_or_default();
    let name = remotes
        .lines()
        .find(|name| *name == "origin")
        .or_else(|| remotes.lines().next());
    let remote = match name {
        Some(name) => git(path, &["remote", "get-url", name]).await,
        None => None,
    };
    let web_url = remote.as_deref().and_then(web_url);
    let host = web_url.as_deref().map(|url| host_kind(url).to_owned());
    let worktree = (Path::new(git_dir) != Path::new(common))
        .then(|| {
            Path::new(top)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .flatten();
    Some(ProjectRepository {
        branch,
        host,
        web_url,
        worktree,
    })
}

async fn head_branch(head: &Path) -> std::io::Result<Option<String>> {
    use tokio::io::AsyncReadExt as _;
    const LIMIT: u64 = 64 * 1024;
    let file = match tokio::fs::File::open(head).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > LIMIT {
        return Ok(None);
    }
    Ok(std::str::from_utf8(&bytes)
        .ok()
        .and_then(|head| head.trim().strip_prefix("ref: refs/heads/"))
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned))
}

fn web_url(remote: &str) -> Option<String> {
    let remote = remote.trim();
    let (host, path) = if let Some(rest) = remote
        .strip_prefix("https://")
        .or_else(|| remote.strip_prefix("http://"))
        .or_else(|| remote.strip_prefix("ssh://"))
        .or_else(|| remote.strip_prefix("git://"))
    {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        let host = if remote.starts_with("ssh://") {
            host.split(':').next()?
        } else {
            host
        };
        (host, path)
    } else if !remote.contains("://") && remote.contains(':') && !remote.starts_with('/') {
        let (authority, path) = remote.split_once(':')?;
        if authority.len() == 1 {
            return None;
        }
        (authority.rsplit('@').next()?, path)
    } else {
        return None;
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    (!host.is_empty() && !path.is_empty()).then(|| format!("https://{host}/{path}"))
}

async fn owner_avatar(
    http: &reqwest::Client,
    repository: &ProjectRepository,
) -> Result<Option<Vec<u8>>, AvatarError> {
    const LIMIT: usize = AVATAR_BYTES;
    let Some((host, domain, path)) = repository
        .web_url
        .as_deref()
        .and_then(|url| url.strip_prefix("https://"))
        .and_then(|rest| rest.split_once('/'))
        .zip(repository.host.as_deref())
        .map(|((domain, path), host)| (host, domain, path))
    else {
        return Ok(None);
    };
    let owner = path.split('/').next().unwrap_or(path);
    let (metadata, fields, host): (String, &[&str], &'static str) = match host {
        "github" if domain == "github.com" => (
            format!("https://api.github.com/users/{owner}"),
            &["/avatar_url"],
            "github",
        ),
        "gitlab" => (
            format!(
                "https://{domain}/api/v4/projects/{}",
                path.replace('/', "%2F")
            ),
            &["/avatar_url", "/namespace/avatar_url", "/owner/avatar_url"],
            "gitlab",
        ),
        "codeberg" | "gitea" => (
            format!("https://{domain}/api/v1/repos/{path}"),
            &["/avatar_url", "/owner/avatar_url"],
            "gitea",
        ),
        _ => return Ok(None),
    };
    let download = async |url: &str| -> Result<Vec<u8>, AvatarError> {
        let fetch = |source| AvatarError::Fetch {
            url: url.to_owned(),
            source,
        };
        let response = http
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(fetch)?;
        bounded_response(response, LIMIT)
            .await
            .map_err(|failure| match failure {
                Bounded::TooLarge => AvatarError::TooLarge { limit: LIMIT },
                Bounded::Read(source) => fetch(source),
            })
    };
    let metadata: serde_json::Value = serde_json::from_slice(&download(&metadata).await?)
        .map_err(|source| AvatarError::Metadata { host, source })?;
    let mut failed = None;
    for url in fields
        .iter()
        .filter_map(|field| metadata.pointer(field).and_then(serde_json::Value::as_str))
        .filter(|url| url.starts_with("https://"))
    {
        match download(url).await {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(error) => {
                failed.get_or_insert(error);
            }
        }
    }
    failed.map_or(Ok(None), Err)
}

#[derive(Debug)]
pub(crate) enum Bounded {
    TooLarge,
    Read(reqwest::Error),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum IconFileError {
    #[error("inspect the icon file {path}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("open the icon file {path}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read the icon file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub(crate) async fn icon_file(path: &Path) -> Result<Option<Vec<u8>>, IconFileError> {
    use tokio::io::AsyncReadExt as _;
    let usable = |metadata: &std::fs::Metadata| {
        metadata.is_file() && metadata.len() <= forge_store::MAX_BLOB_BYTES as u64
    };
    let metadata = match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(IconFileError::Inspect {
                path: path.to_owned(),
                source,
            });
        }
    };
    if !usable(&metadata) {
        return Ok(None);
    }
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(IconFileError::Open {
                path: path.to_owned(),
                source,
            });
        }
    };
    let metadata = file
        .metadata()
        .await
        .map_err(|source| IconFileError::Inspect {
            path: path.to_owned(),
            source,
        })?;
    if !usable(&metadata) {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(forge_store::MAX_BLOB_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|source| IconFileError::Read {
            path: path.to_owned(),
            source,
        })?;
    Ok((bytes.len() <= forge_store::MAX_BLOB_BYTES).then_some(bytes))
}

async fn bounded_response(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, Bounded> {
    use futures::StreamExt as _;
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(Bounded::TooLarge);
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(Bounded::Read)?;
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(Bounded::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn host_kind(web_url: &str) -> &'static str {
    let host = web_url
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let is = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if is("github.com") {
        "github"
    } else if is("gitlab.com") || host.starts_with("gitlab.") {
        "gitlab"
    } else if is("bitbucket.org") {
        "bitbucket"
    } else if is("codeberg.org") {
        "codeberg"
    } else if is("sr.ht") {
        "sourcehut"
    } else if is("dev.azure.com") || is("visualstudio.com") {
        "azure"
    } else if host.contains("gitea") {
        "gitea"
    } else {
        "other"
    }
}

#[cfg(test)]
#[path = "../tests/repository/unit.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/repository/owner.rs"]
mod owner_tests;
