//! Bounded, coalesced observations of project repositories and owner avatars.
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

pub struct Repositories {
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
    pub(crate) fn new() -> anyhow::Result<Self> {
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
    async fn entry(&self, path: &Path) -> Arc<Mutex<Observation>> {
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries.get(path) {
            return Arc::clone(entry);
        }
        if entries.len() >= ENTRIES {
            // At most four callers own entries; all other slots can be evicted.
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
        let _permit = self.slots.acquire().await.ok()?;
        let entry = self.entry(path).await;
        let mut entry = entry.lock().await;
        self.refresh(path, &mut entry).await;
        entry.repository.as_ref().and_then(|(_, repo)| repo.clone())
    }
    pub(crate) async fn avatar(&self, path: &Path) -> Option<Vec<u8>> {
        let _permit = self.slots.acquire().await.ok()?;
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
                tokio::time::timeout(Duration::from_secs(8), owner_avatar(&self.http, repo))
                    .await
                    .ok()
                    .flatten()
            }
            None => None,
        };
        entry.avatar = Some((Instant::now(), avatar.clone()));
        avatar
    }
}

/// Project icons as stored blobs, by source, so a request decodes and
/// encodes an image only when its source changed.
#[derive(Default)]
pub struct Icons {
    encoded: std::sync::Mutex<HashMap<IconSource, Icon>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum IconSource {
    File {
        path: PathBuf,
        modified: SystemTime,
        len: u64,
    },
    /// An avatar by the SHA-256 of its bytes.
    Avatar([u8; 32]),
}

/// What a source was found to be.
#[derive(Clone)]
pub enum Icon {
    Blob(BlobRef),
    /// Not an image Forge can store.
    Unusable,
}

impl Icons {
    /// `None` when the source is new, or its blob was collected since.
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
        // Changed files leave stale keys behind; starting over bounds them.
        if encoded.len() >= ENTRIES * 4 {
            encoded.clear();
        }
        encoded.insert(source, icon);
    }
}

async fn git(path: &Path, args: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0");
    let output = forge_engines::process::probe(command, Duration::from_secs(3), 64 * 1024)
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|s| s.trim().to_owned())
}
/// Three native commands instead of six: one structural observation, names,
/// then the selected remote URL (git owns URL rewriting and include semantics).
async fn read(path: &Path) -> Option<ProjectRepository> {
    use tokio::io::AsyncReadExt as _;
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
    // HEAD has one documented native ref line; reading it also supports an
    // unborn repository without a fourth branch-probe subprocess.
    let mut head = Vec::new();
    let branch = match tokio::fs::File::open(Path::new(git_dir).join("HEAD")).await {
        Ok(file) => {
            if file
                .take(64 * 1024 + 1)
                .read_to_end(&mut head)
                .await
                .is_ok()
                && head.len() <= 64 * 1024
            {
                std::str::from_utf8(&head)
                    .ok()
                    .and_then(|head| head.trim().strip_prefix("ref: refs/heads/"))
                    .filter(|head| !head.is_empty())
                    .map(str::to_owned)
            } else {
                None
            }
        }
        Err(_) => None,
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

/// The browser page of a remote URL: `git@github.com:o/r.git`,
/// `ssh://git@host/o/r` and `https://host/o/r.git` all become
/// `https://host/o/r`. Local paths and file URLs have none.
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
        // An ssh port is not the web port.
        let host = if remote.starts_with("ssh://") {
            host.split(':').next()?
        } else {
            host
        };
        (host, path)
    } else if !remote.contains("://") && remote.contains(':') && !remote.starts_with('/') {
        // scp-like: `user@host:owner/repo.git`.
        let (authority, path) = remote.split_once(':')?;
        if authority.len() == 1 {
            // A Windows drive letter, not a host.
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

/// The repository owner's avatar from its host's public API, for a project
/// without an icon file of its own. Only the owner's name is sent, never a
/// private repository's; `None` on any failure.
async fn owner_avatar(http: &reqwest::Client, repository: &ProjectRepository) -> Option<Vec<u8>> {
    const LIMIT: usize = AVATAR_BYTES;
    let web_url = repository.web_url.as_deref()?;
    let rest = web_url.strip_prefix("https://")?;
    let (domain, path) = rest.split_once('/')?;
    let owner = path.split('/').next()?;
    let (metadata, fields): (String, &[&str]) = match repository.host.as_deref()? {
        "github" if domain == "github.com" => (
            format!("https://api.github.com/users/{owner}"),
            &["/avatar_url"],
        ),
        "gitlab" => (
            format!(
                "https://{domain}/api/v4/projects/{}",
                path.replace('/', "%2F")
            ),
            &["/avatar_url", "/namespace/avatar_url", "/owner/avatar_url"],
        ),
        "codeberg" | "gitea" => (
            format!("https://{domain}/api/v1/repos/{path}"),
            &["/avatar_url", "/owner/avatar_url"],
        ),
        _ => return None,
    };
    let download = async |url: &str| -> Option<Vec<u8>> {
        if !url.starts_with("https://") {
            return None;
        }
        let response = http.get(url).send().await.ok()?.error_for_status().ok()?;
        bounded_response(response, LIMIT).await
    };
    let metadata: serde_json::Value = serde_json::from_slice(&download(&metadata).await?).ok()?;
    for field in fields {
        if let Some(url) = metadata.pointer(field).and_then(serde_json::Value::as_str)
            && let Some(bytes) = download(url).await
        {
            return Some(bytes);
        }
    }
    None
}

/// Local icons share Forge's encoded blob admission limit before allocation.
pub async fn icon_file(path: &Path) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt as _;
    // Reject pipes/devices before opening: a finite read limit alone does not
    // prevent a pipe's open from occupying a blocking worker indefinitely.
    let metadata = tokio::fs::metadata(path).await.ok()?;
    if !metadata.is_file() || metadata.len() > forge_store::MAX_BLOB_BYTES as u64 {
        return None;
    }
    let file = tokio::fs::File::open(path).await.ok()?;
    let metadata = file.metadata().await.ok()?;
    if !metadata.is_file() || metadata.len() > forge_store::MAX_BLOB_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(forge_store::MAX_BLOB_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    (bytes.len() <= forge_store::MAX_BLOB_BYTES).then_some(bytes)
}

async fn bounded_response(response: reqwest::Response, limit: usize) -> Option<Vec<u8>> {
    use futures::StreamExt as _;
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return None;
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
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
mod tests {
    use super::{host_kind, web_url};

    #[test]
    fn remotes_become_their_browser_pages() {
        let page = |remote| web_url(remote);
        assert_eq!(
            page("git@github.com:artisanstreet/editor.git").as_deref(),
            Some("https://github.com/artisanstreet/editor")
        );
        assert_eq!(
            page("https://github.com/sandersonstabo/shooter").as_deref(),
            Some("https://github.com/sandersonstabo/shooter")
        );
        assert_eq!(
            page("ssh://git@gitlab.com:2222/group/sub/repo.git").as_deref(),
            Some("https://gitlab.com/group/sub/repo")
        );
        assert_eq!(page("/srv/git/repo.git"), None);
        assert_eq!(page("C:\\repos\\thing"), None);
        assert_eq!(host_kind("https://github.com/o/r"), "github");
        assert_eq!(host_kind("https://git.example.com/o/r"), "other");
    }
}

#[cfg(test)]
mod owner_tests {
    use super::*;
    async fn command(path: &Path, args: &[&str]) {
        let mut command = tokio::process::Command::new("git");
        command.arg("-C").arg(path).args(args);
        let output = forge_engines::process::probe(command, Duration::from_secs(3), 64 * 1024)
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[tokio::test]
    async fn native_repository_handles_unborn_detached_and_linked_worktree() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path();
        command(path, &["init", "-b", "main"]).await;
        command(
            path,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:artisanstreet/editor.git",
            ],
        )
        .await;
        let unborn = read(path).await.unwrap();
        assert_eq!(unborn.branch.as_deref(), Some("main"));
        assert_eq!(
            unborn.web_url.as_deref(),
            Some("https://github.com/artisanstreet/editor")
        );
        command(
            path,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        )
        .await;
        let linked = folder.path().join("linked");
        command(
            path,
            &["worktree", "add", "-b", "side", linked.to_str().unwrap()],
        )
        .await;
        let worktree = read(&linked).await.unwrap();
        assert_eq!(worktree.branch.as_deref(), Some("side"));
        assert_eq!(worktree.worktree.as_deref(), Some("linked"));
        command(path, &["checkout", "--detach"]).await;
        assert_eq!(read(path).await.unwrap().branch, None);
    }
    #[tokio::test]
    async fn concurrent_observations_share_one_cached_snapshot_and_cache_is_finite() {
        let folder = tempfile::tempdir().unwrap();
        let owner = Arc::new(Repositories::new().unwrap());
        command(folder.path(), &["init", "-b", "main"]).await;
        let mut jobs = Vec::new();
        for _ in 0..16 {
            let owner = Arc::clone(&owner);
            let path = folder.path().to_owned();
            jobs.push(tokio::spawn(
                async move { owner.read(&path).await.unwrap() },
            ));
        }
        for job in jobs {
            assert_eq!(job.await.unwrap().branch.as_deref(), Some("main"));
        }
        let entry = Arc::clone(&owner.entries.lock().await[folder.path()]);
        let observed = entry.lock().await.repository.as_ref().unwrap().0;
        owner.read(folder.path()).await;
        assert_eq!(entry.lock().await.repository.as_ref().unwrap().0, observed);
        drop(entry);
        for i in 0..ENTRIES + 8 {
            owner
                .read(&folder.path().join(format!("missing-{i}")))
                .await;
        }
        assert_eq!(owner.entries.lock().await.len(), ENTRIES);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn a_fifo_icon_is_rejected_without_waiting_for_a_writer() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("icon.png");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(
            tokio::time::timeout(Duration::from_secs(1), icon_file(&path))
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn oversized_sparse_local_icon_is_rejected_before_reading() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("icon.png");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(forge_store::MAX_BLOB_BYTES as u64 + 1)
            .unwrap();
        assert!(icon_file(&path).await.is_none());
        std::fs::write(&path, b"small native file").unwrap();
        assert_eq!(icon_file(&path).await.unwrap(), b"small native file");
    }
    #[tokio::test]
    async fn unknown_length_chunked_avatar_is_rejected_before_buffering_the_tail() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 8192];
            let _ = socket.read(&mut request).await.unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            let chunk = vec![b'x'; 300 * 1024];
            for _ in 0..3 {
                if socket
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
                if socket.write_all(&chunk).await.is_err() {
                    break;
                }
                if socket.write_all(b"\r\n").await.is_err() {
                    break;
                }
            }
            socket.write_all(b"0\r\n\r\n").await.ok();
        });
        let response = reqwest::Client::new()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.content_length(), None);
        assert!(bounded_response(response, AVATAR_BYTES).await.is_none());
        server.await.unwrap();
    }
}
