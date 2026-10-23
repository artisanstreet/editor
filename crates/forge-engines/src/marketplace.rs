//! The Marketplace's directories: skills.sh's search, and fetching one of
//! its skills from the GitHub repository that publishes it.

use anyhow::{Context as _, Result, ensure};
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

/// skills.sh's best matches for `query`, most relevant first.
///
/// # Errors
///
/// When skills.sh can't be reached or answers something else.
pub async fn search_skills(
    http: &reqwest::Client,
    base: &str,
    query: &str,
) -> Result<Vec<Listing>> {
    let bytes = http
        .get(format!("{base}/api/search"))
        .query(&[("q", query), ("limit", "20")])
        .send()
        .await
        .context("reaching skills.sh")?
        .error_for_status()
        .context("skills.sh refused the search")?
        .bytes()
        .await
        .context("reading skills.sh's answer")?;
    let search: Search = serde_json::from_slice(&bytes).context("reading skills.sh's answer")?;
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

/// Downloads the repository behind a skills.sh listing into `scratch` and
/// returns the folder of the skill it names: the one whose `SKILL.md` has
/// that name, else the folder of that name.
///
/// # Errors
///
/// Phrased for the user: an id that isn't skills.sh's, an unreachable or
/// oversized repository, or a repository without that skill.
pub async fn fetch_skill(http: &reqwest::Client, id: &str, scratch: &Path) -> Result<PathBuf> {
    let (owner, repo, skill) = id
        .strip_prefix("skills.sh:")
        .and_then(|rest| {
            let mut parts = rest.splitn(3, '/');
            Some((parts.next()?, parts.next()?, parts.next()?))
        })
        .filter(|(owner, repo, skill)| valid_part(owner) && valid_part(repo) && valid_part(skill))
        .context("That isn't a skills.sh listing.")?;
    let url = format!("https://codeload.github.com/{owner}/{repo}/tar.gz/HEAD");
    let response = http
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Couldn't reach {owner}/{repo} on GitHub."))?
        .error_for_status()
        .with_context(|| format!("GitHub has no repository {owner}/{repo}."))?;
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= MAX_ARCHIVE_BYTES),
        "{owner}/{repo} is too large to install from."
    );
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("Couldn't download {owner}/{repo}."))?;
    ensure!(
        bytes.len() as u64 <= MAX_ARCHIVE_BYTES,
        "{owner}/{repo} is too large to install from."
    );
    let root = scratch.join("repository");
    let unpack_root = root.clone();
    tokio::task::spawn_blocking(move || unpack(&bytes, &unpack_root))
        .await
        .context("unpacking task")??;
    find_skill(&root, skill)?.with_context(|| format!("{owner}/{repo} has no skill named {skill}."))
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

/// Unpacks the regular files of a GitHub tarball under `root`, without its
/// `<repo>-<commit>/` top folder. Links and paths that leave `root` are
/// skipped.
fn unpack(bytes: &[u8], root: &Path) -> Result<()> {
    fs::create_dir_all(root)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for entry in archive.entries().context("reading the repository")? {
        let mut entry = entry.context("reading the repository")?;
        if !matches!(
            entry.header().entry_type(),
            tar::EntryType::Regular | tar::EntryType::Continuous
        ) || entry.size() > MAX_FILE_BYTES
        {
            continue;
        }
        let path = entry.path()?.into_owned();
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
        let to = root.join(&relative);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .with_context(|| format!("unpacking {}", relative.display()))?;
        fs::write(&to, data)?;
    }
    Ok(())
}

/// The folder under `root` whose `SKILL.md` names `skill`, else one named
/// `skill` with a `SKILL.md`.
fn find_skill(root: &Path, skill: &str) -> Result<Option<PathBuf>> {
    let mut by_folder = None;
    for entry in walkdir::WalkDir::new(root).max_depth(8).follow_links(false) {
        let entry = entry?;
        if entry.file_name() != "SKILL.md" || !entry.file_type().is_file() {
            continue;
        }
        let Some(folder) = entry.path().parent() else {
            continue;
        };
        let text = fs::read_to_string(entry.path()).unwrap_or_default();
        if crate::customise::skill_name(&text).as_deref() == Some(skill) {
            return Ok(Some(folder.to_path_buf()));
        }
        if by_folder.is_none() && folder.file_name().is_some_and(|name| name == skill) {
            by_folder = Some(folder.to_path_buf());
        }
    }
    Ok(by_folder)
}

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
        unpack(&bytes, dir.path()).unwrap();
        let folder = find_skill(dir.path(), "frontend-design").unwrap().unwrap();
        assert_eq!(folder, dir.path().join("skills/frontend"));
        assert!(folder.join("notes.md").is_file());
        assert!(find_skill(dir.path(), "missing").unwrap().is_none());
    }
}
