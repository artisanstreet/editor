//! The Marketplace's directories: skills.sh's search, and fetching one of
//! its skills from the GitHub repository that publishes it.

use crate::expansion::{Expansion, ExpansionError, Limits};
use artisan_api::Listing;
use serde::Deserialize;
use std::fs;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

const SKILLS_SH: &str = "skills.sh";
/// skills.sh itself; Forge's tests point the search at a stub.
pub const SKILLS_SH_URL: &str = "https://skills.sh";
/// A repository's tarball; larger ones are refused rather than unpacked.
const MAX_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
/// A file inside it; larger ones are skipped.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const UNPACK_LIMITS: Limits = Limits {
    bytes: 512 * 1024 * 1024,
    files: 50_000,
    folders: 20_000,
    spare_space: 256 * 1024 * 1024,
};

#[derive(Deserialize)]
struct Search {
    skills: Vec<Hit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hit {
    source: String,
    skill_id: String,
    name: String,
    #[serde(default)]
    installs: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchSkillsError {
    #[error("reaching skills.sh at {url}")]
    Reach {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("skills.sh refused the search")]
    Refused(#[source] reqwest::Error),
    #[error("reading skills.sh's answer")]
    Body(#[source] reqwest::Error),
    #[error("reading skills.sh's answer")]
    Decode(#[source] serde_json::Error),
}

pub async fn search_skills(
    http: &reqwest::Client,
    base: &str,
    query: &str,
) -> Result<Vec<Listing>, SearchSkillsError> {
    let url = format!("{base}/api/search");
    let bytes = http
        .get(&url)
        .query(&[("q", query), ("limit", "20")])
        .send()
        .await
        .map_err(|source| SearchSkillsError::Reach {
            url: url.clone(),
            source,
        })?
        .error_for_status()
        .map_err(SearchSkillsError::Refused)?
        .bytes()
        .await
        .map_err(SearchSkillsError::Body)?;
    let search: Search = serde_json::from_slice(&bytes).map_err(SearchSkillsError::Decode)?;
    Ok(search
        .skills
        .into_iter()
        .filter(|hit| repository(&hit.source).is_some() && valid_part(&hit.skill_id))
        .map(|hit| Listing {
            id: format!("{SKILLS_SH}:{}/{}", hit.source, hit.skill_id),
            name: hit.name,
            publisher: hit.source,
            source: SKILLS_SH.to_owned(),
            installs: hit.installs,
        })
        .collect())
}

#[derive(Debug, thiserror::Error)]
pub enum FetchSkillError {
    #[error("That isn't a skills.sh listing.")]
    NotListing { id: String },
    #[error("Couldn't reach {repository} on GitHub.")]
    Reach {
        repository: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("GitHub has no repository {repository}.")]
    NoRepository {
        repository: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{repository} is too large to install from.")]
    TooLarge { repository: String },
    #[error("Couldn't download {repository}.")]
    Download {
        repository: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("unpack {repository}")]
    Unpack {
        repository: String,
        #[source]
        source: UnpackError,
    },
    #[error("wait for the {repository} unpacking task")]
    Task {
        repository: String,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("search {repository} for {skill}")]
    Search {
        repository: String,
        skill: String,
        #[source]
        source: walkdir::Error,
    },
    #[error("{repository} has no skill named {skill}.")]
    NoSkill { repository: String, skill: String },
}

pub async fn fetch_skill(
    http: &reqwest::Client,
    id: &str,
    scratch: &Path,
) -> Result<PathBuf, FetchSkillError> {
    let (owner, repo, skill) = id
        .strip_prefix("skills.sh:")
        .and_then(|rest| {
            let mut parts = rest.splitn(3, '/');
            Some((parts.next()?, parts.next()?, parts.next()?))
        })
        .filter(|(owner, repo, skill)| valid_part(owner) && valid_part(repo) && valid_part(skill))
        .ok_or_else(|| FetchSkillError::NotListing { id: id.to_owned() })?;
    let repository = format!("{owner}/{repo}");
    let url = format!("https://codeload.github.com/{repository}/tar.gz/HEAD");
    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|source| FetchSkillError::Reach {
            repository: repository.clone(),
            source,
        })?
        .error_for_status()
        .map_err(|source| FetchSkillError::NoRepository {
            repository: repository.clone(),
            source,
        })?;
    let bytes = download(response, MAX_ARCHIVE_BYTES, &repository).await?;
    let root = scratch.join("repository");
    let unpack_root = root.clone();
    tokio::task::spawn_blocking(move || unpack(&bytes, &unpack_root, UNPACK_LIMITS))
        .await
        .map_err(|source| FetchSkillError::Task {
            repository: repository.clone(),
            source,
        })?
        .map_err(|source| FetchSkillError::Unpack {
            repository: repository.clone(),
            source,
        })?;
    find_skill(&root, skill)
        .map_err(|source| FetchSkillError::Search {
            repository: repository.clone(),
            skill: skill.to_owned(),
            source,
        })?
        .ok_or_else(|| FetchSkillError::NoSkill {
            repository,
            skill: skill.to_owned(),
        })
}

async fn download(
    mut response: reqwest::Response,
    limit: u64,
    repository: &str,
) -> Result<Vec<u8>, FetchSkillError> {
    let too_large = || FetchSkillError::TooLarge {
        repository: repository.to_owned(),
    };
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| FetchSkillError::Download {
            repository: repository.to_owned(),
            source,
        })?
    {
        if (bytes.len() + chunk.len()) as u64 > limit {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// A `<owner>/<repo>` source, split.
fn repository(source: &str) -> Option<(&str, &str)> {
    let (owner, repo) = source.split_once('/')?;
    (valid_part(owner) && valid_part(repo)).then_some((owner, repo))
}

/// One GitHub owner, repository or skill name: no separators, no dots alone.
fn valid_part(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

#[derive(Debug, thiserror::Error)]
pub enum UnpackError {
    #[error("create {}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("reading the repository")]
    Read(#[source] std::io::Error),
    #[error("the repository is too large to unpack")]
    TooLarge(#[source] ExpansionError),
    #[error("unpacking {}", member.display())]
    Member {
        member: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn unpack(bytes: &[u8], root: &Path, limits: Limits) -> Result<(), UnpackError> {
    let create = |path: &Path| {
        fs::create_dir_all(path).map_err(|source| UnpackError::Create {
            path: path.to_owned(),
            source,
        })
    };
    create(root)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut expansion = Expansion::new(root, limits);
    for entry in archive.entries().map_err(UnpackError::Read)? {
        let mut entry = entry.map_err(UnpackError::Read)?;
        if !matches!(
            entry.header().entry_type(),
            tar::EntryType::Regular | tar::EntryType::Continuous
        ) || entry.size() > MAX_FILE_BYTES
        {
            continue;
        }
        let path = entry.path().map_err(UnpackError::Read)?.into_owned();
        let mut components = path.components();
        components.next();
        let relative: PathBuf = components.collect();
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        {
            continue;
        }
        expansion
            .admit_file(&relative, entry.size())
            .map_err(UnpackError::TooLarge)?;
        let to = root.join(&relative);
        if let Some(parent) = to.parent() {
            create(parent)?;
        }
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|source| UnpackError::Member {
                member: relative,
                source,
            })?;
        fs::write(&to, data).map_err(|source| UnpackError::Write { path: to, source })?;
    }
    Ok(())
}

fn find_skill(root: &Path, skill: &str) -> Result<Option<PathBuf>, walkdir::Error> {
    let mut by_folder = None;
    for entry in walkdir::WalkDir::new(root).max_depth(8).follow_links(false) {
        let entry = entry?;
        if entry.file_name() != "SKILL.md" || !entry.file_type().is_file() {
            continue;
        }
        let Some(folder) = entry.path().parent() else {
            continue;
        };
        let named = match fs::read_to_string(entry.path()) {
            Ok(text) => crate::customise::skill_name(&text),
            Err(error) => {
                tracing::debug!(path = %entry.path().display(), %error, "skip an unreadable SKILL.md");
                None
            }
        };
        if named.as_deref() == Some(skill) {
            return Ok(Some(folder.to_path_buf()));
        }
        if by_folder.is_none() && folder.file_name().is_some_and(|name| name == skill) {
            by_folder = Some(folder.to_path_buf());
        }
    }
    Ok(by_folder)
}

#[cfg(test)]
#[path = "../tests/marketplace/download.rs"]
mod download_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, text) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, text.as_bytes())
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn a_listed_skill_is_found_by_its_name_inside_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = tarball(&[
            ("skills-abc123/README.md", "# Skills"),
            (
                "skills-abc123/skills/frontend/SKILL.md",
                "---\nname: frontend-design\ndescription: Design\n---\n",
            ),
            ("skills-abc123/skills/frontend/notes.md", "notes"),
            (
                "skills-abc123/skills/pdf/SKILL.md",
                "---\nname: pdf\ndescription: PDFs\n---\n",
            ),
        ]);
        unpack(&bytes, dir.path(), UNPACK_LIMITS).unwrap();
        let folder = find_skill(dir.path(), "frontend-design").unwrap().unwrap();
        assert_eq!(folder, dir.path().join("skills/frontend"));
        assert!(folder.join("notes.md").is_file());
        assert!(find_skill(dir.path(), "missing").unwrap().is_none());
    }

    #[test]
    fn a_repository_of_many_small_files_stops_at_the_aggregate_budget() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<(String, String)> = (0..6)
            .map(|index| {
                (
                    format!("repo-abc/skills/s{index}/notes.md"),
                    "x".repeat(400),
                )
            })
            .collect();
        let members: Vec<(&str, &str)> = files
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect();
        let bytes = tarball(&members);
        let limits = Limits {
            bytes: 1000,
            ..UNPACK_LIMITS
        };
        let error = unpack(&bytes, dir.path(), limits).unwrap_err();
        assert!(
            matches!(&error, UnpackError::TooLarge(ExpansionError::Bytes { member, limit: 1000 }) if member == Path::new("skills/s2/notes.md")),
            "{error:?}"
        );
        assert!(!dir.path().join("skills/s2").exists());
        let limits = Limits {
            files: 4,
            ..UNPACK_LIMITS
        };
        let error = unpack(&bytes, &dir.path().join("files"), limits).unwrap_err();
        assert!(
            matches!(
                &error,
                UnpackError::TooLarge(ExpansionError::Files { limit: 4, .. })
            ),
            "{error:?}"
        );
        let limits = Limits {
            folders: 3,
            ..UNPACK_LIMITS
        };
        let error = unpack(&bytes, &dir.path().join("folders"), limits).unwrap_err();
        assert!(
            matches!(
                &error,
                UnpackError::TooLarge(ExpansionError::Folders { limit: 3, .. })
            ),
            "{error:?}"
        );
    }
}
