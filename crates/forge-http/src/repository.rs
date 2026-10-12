//! What git says about a project's folder: the checked-out branch and the
//! default remote's browser page. Read with the `git` binary each time it is
//! asked, so a branch switch shows on the next read.

use artisan_api::ProjectRepository;
use std::path::Path;
use std::process::Command;

/// `None` when the folder is not in a git repository or git is missing.
pub(crate) fn read(path: &Path) -> Option<ProjectRepository> {
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let git_dir = git(&["rev-parse", "--absolute-git-dir"])?;
    let branch = git(&["symbolic-ref", "--quiet", "--short", "HEAD"]).filter(|b| !b.is_empty());
    let remotes = git(&["remote"]).unwrap_or_default();
    let remote = remotes
        .lines()
        .find(|name| *name == "origin")
        .or_else(|| remotes.lines().next())
        .and_then(|name| git(&["remote", "get-url", name]));
    let web_url = remote.as_deref().and_then(web_url);
    let host = web_url.as_deref().map(|url| host_kind(url).to_owned());
    // A linked worktree keeps its own git dir beside the common one.
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    let worktree = common
        .filter(|common| Path::new(common) != Path::new(&git_dir))
        .and_then(|_| git(&["rev-parse", "--show-toplevel"]))
        .and_then(|top| {
            Path::new(&top)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });
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
pub(crate) async fn owner_avatar(repository: &ProjectRepository) -> Option<Vec<u8>> {
    const LIMIT: usize = 512 * 1024;
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
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent(concat!("Artisan-Forge/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let download = async |url: &str| -> Option<Vec<u8>> {
        if !url.starts_with("https://") {
            return None;
        }
        let response = http.get(url).send().await.ok()?.error_for_status().ok()?;
        if response.content_length().is_some_and(|n| n > LIMIT as u64) {
            return None;
        }
        let bytes = response.bytes().await.ok()?;
        (bytes.len() <= LIMIT).then(|| bytes.to_vec())
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
