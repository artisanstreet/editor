//! "No project": the built-in scratch project, whose threads each run in a
//! fresh folder of their own under `<data>/scratch`.

use crate::Hub;
use anyhow::{Context, Result};
use artisan_api::{CreateProject, ProjectId, Thread, Timestamp};
use std::path::{Path, PathBuf};

/// Words of the first message kept in a thread's folder name.
const FOLDER_WORDS: usize = 4;
/// Characters of the thread id that make its folder unique.
const FOLDER_KEY: usize = 12;

impl Hub {
    fn scratch_root(&self) -> PathBuf {
        self.data_dir().join("scratch")
    }

    /// Registers the scratch project. Inside a git work tree (a data folder
    /// in a checkout) every scratch folder would inherit that repository's
    /// status, so it is not offered there.
    pub async fn ensure_scratch_project(&self) -> Result<()> {
        let root = self.scratch_root();
        if inside_work_tree(self.data_dir()) {
            tracing::info!(data = %self.data_dir().display(), "no scratch project inside a git work tree");
            return Ok(());
        }
        let created = root.clone();
        tokio::task::spawn_blocking(move || std::fs::create_dir_all(&created))
            .await
            .context("create scratch folder task")?
            .with_context(|| format!("create {}", root.display()))?;
        self.store()
            .create_project(CreateProject {
                id: ProjectId::scratch(),
                path: root.display().to_string(),
                name: Some("No project".to_owned()),
            })
            .await
            .context("create the scratch project")?;
        Ok(())
    }

    /// Where the thread's engine works: its project's folder, or for a
    /// scratch thread its own folder, created on first use and found again
    /// by the thread id it ends with.
    pub async fn thread_folder(&self, thread: &Thread) -> Result<PathBuf> {
        if thread.project_id.is_scratch() {
            let (root, thread) = (self.scratch_root(), thread.clone());
            return tokio::task::spawn_blocking(move || scratch_folder(&root, &thread))
                .await
                .context("scratch folder task")?;
        }
        let project = self
            .store()
            .project(&thread.project_id)
            .await?
            .with_context(|| format!("no project {}", thread.project_id))?;
        Ok(PathBuf::from(project.path))
    }
}

fn inside_work_tree(path: &Path) -> bool {
    path.ancestors().any(|dir| dir.join(".git").exists())
}

fn scratch_folder(root: &Path, thread: &Thread) -> Result<PathBuf> {
    let key: String = thread
        .id
        .as_str()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let key = &key[key.len().saturating_sub(FOLDER_KEY)..];
    let suffix = format!("-{key}");
    std::fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;
    let existing = std::fs::read_dir(root)
        .with_context(|| format!("list {}", root.display()))?
        .filter_map(std::result::Result::ok)
        .find(|entry| {
            entry.file_name().to_string_lossy().ends_with(&suffix)
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
        });
    if let Some(entry) = existing {
        return Ok(entry.path());
    }
    let words = thread
        .title
        .as_deref()
        .map(folder_words)
        .unwrap_or_default();
    let name = [date(thread.created_at), words, key.to_owned()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let folder = root.join(name);
    std::fs::create_dir_all(&folder).with_context(|| format!("create {}", folder.display()))?;
    Ok(folder)
}

/// The first few words of the message, lowercase ASCII, joined by dashes.
fn folder_words(text: &str) -> String {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(FOLDER_WORDS)
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

/// `YYYY-MM-DD` in UTC (Howard Hinnant's `civil_from_days`).
fn date(at: Timestamp) -> String {
    let days = at.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
