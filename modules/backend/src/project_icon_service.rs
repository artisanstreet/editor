//! Background discovery of small repository icons. No I/O runs in a listing.

use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use artisan_domain::{RECENT_PROJECT_ICON_MAX_BYTES, RecentProjectIcon, RootPath};
use reqwest::Url;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::git_remote_url_policy::{
    repository_host_for, repository_path_for, repository_web_url_for,
};
use crate::project_repository_service::RepositoryObservation;

const INPUT_LIMIT: usize = 512 * 1024;
static DOWNLOADS: Semaphore = Semaphore::const_new(4);
static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();

pub(crate) fn identity(observation: Option<&RepositoryObservation>) -> Option<String> {
    let artisan_protocol::ProjectRepository::Repository(repository) = &observation?.repository
    else {
        return None;
    };
    repository_web_url_for(repository.default_remote_projection()?.url())
}

pub(crate) fn fallback(remote: Option<&str>) -> RecentProjectIcon {
    RecentProjectIcon {
        host: remote.and_then(|remote| {
            artisan_domain::DisplayName::parse(repository_host_for(remote).as_str()).ok()
        }),
        png: Default::default(),
    }
}

pub(crate) async fn resolve(root: &RootPath, remote: &str) -> RecentProjectIcon {
    let mut result = fallback(Some(remote));
    let local_root = root.clone();
    if let Ok(Some(png)) =
        tokio::task::spawn_blocking(move || local_icon(Path::new(local_root.as_str()))).await
    {
        result.png = png.into();
        return result;
    }
    // Best-effort metadata is bounded independently of Git observations.
    if let Ok(Some(png)) = tokio::time::timeout(Duration::from_secs(8), remote_icon(remote)).await {
        result.png = png.into();
    }
    result
}

fn local_icon(root: &Path) -> Option<Vec<u8>> {
    for relative in [
        ".artisan/icon.png",
        ".artisan/icon.webp",
        "icon.png",
        "favicon.png",
        "public/favicon.png",
    ] {
        let file = std::fs::File::open(root.join(relative)).ok();
        let Some(file) = file else { continue };
        let mut bytes = Vec::new();
        if file
            .take((INPUT_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .is_ok()
            && let Some(png) = normalize(&bytes)
        {
            return Some(png);
        }
    }
    None
}

fn normalize(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() > INPUT_LIMIT {
        return None;
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?.thumbnail(48, 48);
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, image::ImageFormat::Png).ok()?;
    let png = output.into_inner();
    (png.len() <= RECENT_PROJECT_ICON_MAX_BYTES).then_some(png)
}

fn client() -> Option<&'static reqwest::Client> {
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .redirect(reqwest::redirect::Policy::limited(3))
                .user_agent("Artisan-Editor-project-icons")
                .build()
                .ok()
        })
        .as_ref()
}

async fn download(url: Url) -> Option<Vec<u8>> {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let _permit = DOWNLOADS.acquire().await.ok()?;
    let mut response = client()?
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    if response
        .content_length()
        .is_some_and(|n| n > INPUT_LIMIT as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > INPUT_LIMIT {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
}

async fn remote_icon(remote: &str) -> Option<Vec<u8>> {
    let base = Url::parse(remote).ok()?;
    let path = repository_path_for(remote)?;
    // Never send a private repository name to another host. GitHub's public
    // account endpoint also works when its repository cannot be read.
    let metadata_url = metadata_url(&base, &path)?;
    let metadata: Value = serde_json::from_slice(&download(metadata_url).await?).ok()?;
    for candidate in candidates(&metadata, base.host_str()?) {
        let Ok(url) = base.join(candidate) else {
            continue;
        };
        if let Some(bytes) = download(url).await
            && let Ok(Some(png)) = tokio::task::spawn_blocking(move || normalize(&bytes)).await
        {
            return Some(png);
        }
    }
    None
}

fn metadata_url(base: &Url, path: &str) -> Option<Url> {
    use crate::git_remote_url_policy::RepositoryHost;
    let mut url = base.clone();
    url.set_query(None);
    url.set_fragment(None);
    match repository_host_for(base.as_str()) {
        RepositoryHost::Github if base.host_str() == Some("github.com") => {
            url = Url::parse("https://api.github.com/users/").ok()?;
            url.path_segments_mut()
                .ok()?
                .pop_if_empty()
                .push(path.split('/').next()?);
        }
        RepositoryHost::Gitlab => {
            url.set_path("/api/v4/projects/");
            url.path_segments_mut().ok()?.pop_if_empty().push(path);
        }
        RepositoryHost::Bitbucket if base.host_str() == Some("bitbucket.org") => {
            url = Url::parse("https://api.bitbucket.org/2.0/repositories/").ok()?;
            for segment in path.split('/') {
                url.path_segments_mut().ok()?.pop_if_empty().push(segment);
            }
        }
        RepositoryHost::Codeberg | RepositoryHost::Gitea => {
            url.set_path("/api/v1/repos/");
            for segment in path.split('/') {
                url.path_segments_mut().ok()?.pop_if_empty().push(segment);
            }
        }
        _ => return None,
    }
    Some(url)
}

fn candidates<'a>(metadata: &'a Value, host: &str) -> Vec<&'a str> {
    let fields: &[&str] = match repository_host_for(&format!("https://{host}")) {
        crate::git_remote_url_policy::RepositoryHost::Github => &["/avatar_url"],
        crate::git_remote_url_policy::RepositoryHost::Gitlab => {
            &["/avatar_url", "/namespace/avatar_url", "/owner/avatar_url"]
        }
        crate::git_remote_url_policy::RepositoryHost::Bitbucket => {
            &["/links/avatar/href", "/owner/links/avatar/href"]
        }
        _ => &["/avatar_url", "/owner/avatar_url"],
    };
    fields
        .iter()
        .filter_map(|field| metadata.pointer(field)?.as_str())
        .filter(|url| !url.is_empty())
        .collect()
}

#[cfg(test)]
#[path = "../../../tests/backend/project_icon_service.rs"]
mod tests;
