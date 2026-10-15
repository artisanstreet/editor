//! Explicit import from retired content-addressed skill storage.
use super::*;

/// Best-effort import of skills from the old content-addressed store
/// (`<old data>/extensions/store/<digest>/`), named by their `SKILL.md`.
/// Skills that fail to copy are logged and skipped.
///
/// Individual invalid legacy skills are reported and skipped.
pub fn import_legacy_extensions(old_data_dir: &Path, skills_dir: &Path) -> Vec<String> {
    let store = old_data_dir.join("extensions/store");
    let Ok(entries) = fs::read_dir(&store) else {
        return Vec::new();
    };
    let mut imported = Vec::new();
    for entry in entries.flatten() {
        let Ok(text) = read_skill_header(&entry.path().join("SKILL.md")) else {
            continue;
        };
        let Some(name) = frontmatter_name(&text) else {
            tracing::info!(folder = %entry.path().display(), "old skill without a usable name; skipped");
            continue;
        };
        match import_folder(&entry.path(), skills_dir, &name) {
            Ok(true) => imported.push(name),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(skill = %name, error = %format!("{error:#}"), "couldn't import an old skill")
            }
        }
    }
    imported
}
