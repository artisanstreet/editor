//! Agent Skills folders: bounded reading, frontmatter, and content digests.
//!
//! A skill is a folder holding `SKILL.md` (YAML frontmatter with `name` and
//! `description`, then instructions) and any supporting files. Reading is
//! bounded and refuses links inside the folder, so a skill can never smuggle
//! in a file from elsewhere. The digest covers every relative path and its
//! bytes in sorted order, so equal folders hash equally on every platform.

use std::fs;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

/// The instructions file every skill holds at its root.
pub const SKILL_FILE: &str = "SKILL.md";
/// Maximum files in one skill.
pub const SKILL_MAX_FILES: usize = 256;
/// Maximum total bytes of one skill.
pub const SKILL_MAX_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum folder depth below the skill root.
pub const SKILL_MAX_DEPTH: usize = 8;

/// One file of a skill, by its `/`-separated path relative to the root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillFile {
    /// Relative path with `/` separators.
    pub path: String,
    /// File contents.
    pub bytes: Vec<u8>,
}

/// A complete, validated skill held in memory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillPackage {
    name: String,
    description: String,
    files: Vec<SkillFile>,
    digest: String,
}

impl SkillPackage {
    /// Builds a package from its files, validating paths, bounds, and the
    /// `SKILL.md` frontmatter.
    ///
    /// # Errors
    ///
    /// Returns [`SkillError`] for unsafe or repeated paths, exceeded bounds,
    /// a missing `SKILL.md`, or invalid frontmatter.
    pub fn from_files(mut files: Vec<SkillFile>) -> Result<Self, SkillError> {
        if files.len() > SKILL_MAX_FILES {
            return Err(SkillError::TooManyFiles);
        }
        let mut total: u64 = 0;
        for file in &files {
            if !safe_relative_path(&file.path) {
                return Err(SkillError::UnsafePath);
            }
            total = total.saturating_add(file.bytes.len() as u64);
        }
        if total > SKILL_MAX_BYTES {
            return Err(SkillError::TooLarge);
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        if files.windows(2).any(|pair| pair[0].path == pair[1].path) {
            return Err(SkillError::UnsafePath);
        }
        let instructions = files
            .iter()
            .find(|file| file.path == SKILL_FILE)
            .ok_or(SkillError::MissingSkillFile)?;
        let text = std::str::from_utf8(&instructions.bytes).map_err(|_| SkillError::Frontmatter)?;
        let (name, description) = parse_frontmatter(text)?;
        let digest = digest_files(&files);
        Ok(Self {
            name,
            description,
            files,
            digest,
        })
    }

    /// Returns the skill name from the frontmatter.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the description from the frontmatter.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the files in path order.
    #[must_use]
    pub fn files(&self) -> &[SkillFile] {
        &self.files
    }

    /// Returns the lowercase hexadecimal SHA-256 content digest.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Returns the total bytes of every file.
    #[must_use]
    pub fn byte_count(&self) -> u64 {
        self.files.iter().map(|file| file.bytes.len() as u64).sum()
    }
}

/// Reads a skill folder from disk.
///
/// # Errors
///
/// Returns [`SkillError`] when the folder is missing or unreadable, holds a
/// link or special file, exceeds the bounds, or has no valid `SKILL.md`.
pub fn read_skill_dir(root: &Path) -> Result<SkillPackage, SkillError> {
    let metadata = fs::symlink_metadata(root).map_err(|_| SkillError::Unreadable)?;
    if !metadata.is_dir() {
        return Err(SkillError::Unreadable);
    }
    let mut files = Vec::new();
    let mut total: u64 = 0;
    collect(root, root, 0, &mut files, &mut total)?;
    SkillPackage::from_files(files)
}

fn collect(
    root: &Path,
    directory: &Path,
    depth: usize,
    files: &mut Vec<SkillFile>,
    total: &mut u64,
) -> Result<(), SkillError> {
    if depth > SKILL_MAX_DEPTH {
        return Err(SkillError::TooDeep);
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(directory)
        .map_err(|_| SkillError::Unreadable)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .map_err(|_| SkillError::Unreadable)?;
    entries.sort();
    for path in entries {
        let metadata = fs::symlink_metadata(&path).map_err(|_| SkillError::Unreadable)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(SkillError::Link);
        }
        if file_type.is_dir() {
            collect(root, &path, depth + 1, files, total)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(SkillError::Link);
        }
        if files.len() >= SKILL_MAX_FILES {
            return Err(SkillError::TooManyFiles);
        }
        *total = total.saturating_add(metadata.len());
        if *total > SKILL_MAX_BYTES {
            return Err(SkillError::TooLarge);
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| SkillError::UnsafePath)?
            .components()
            .map(|component| match component {
                Component::Normal(part) => part.to_str().map(str::to_owned),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(SkillError::UnsafePath)?
            .join("/");
        let bytes = fs::read(&path).map_err(|_| SkillError::Unreadable)?;
        files.push(SkillFile {
            path: relative,
            bytes,
        });
    }
    Ok(())
}

/// Hashes sorted `(path, bytes)` pairs with length prefixes, so no two
/// different folders share a byte stream.
fn digest_files(files: &[SkillFile]) -> String {
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update((file.path.len() as u64).to_be_bytes());
        hasher.update(file.path.as_bytes());
        hasher.update((file.bytes.len() as u64).to_be_bytes());
        hasher.update(&file.bytes);
    }
    hex(&hasher.finalize())
}

/// Lowercase hexadecimal of `bytes`.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains(':'))
}

/// Parses `name` and `description` from `SKILL.md` frontmatter.
///
/// Supports the forms skills use in practice: plain and quoted scalars, and
/// folded (`>`) or literal (`|`) blocks. Other keys are ignored.
///
/// # Errors
///
/// Returns [`SkillError::Frontmatter`] when the frontmatter is missing or
/// lacks a valid name, and [`SkillError::Name`] for an invalid name.
pub fn parse_frontmatter(text: &str) -> Result<(String, String), SkillError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Err(SkillError::Frontmatter);
    }
    let block: Vec<&str> = lines.take_while(|line| line.trim_end() != "---").collect();
    let name = frontmatter_value(&block, "name").ok_or(SkillError::Frontmatter)?;
    if !artisan_domain::valid_skill_name(&name) {
        return Err(SkillError::Name);
    }
    let mut description = frontmatter_value(&block, "description").unwrap_or_default();
    if description.len() > artisan_domain::EXTENSION_DESCRIPTION_MAX_BYTES {
        let mut cut = artisan_domain::EXTENSION_DESCRIPTION_MAX_BYTES;
        while !description.is_char_boundary(cut) {
            cut -= 1;
        }
        description.truncate(cut);
    }
    Ok((name, description))
}

fn frontmatter_value(block: &[&str], key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let index = block.iter().position(|line| line.starts_with(&prefix))?;
    let raw = block[index][prefix.len()..].trim();
    if raw.starts_with('>') || raw.starts_with('|') {
        let folded = raw.starts_with('>');
        let continuation: Vec<&str> = block[index + 1..]
            .iter()
            .take_while(|line| line.is_empty() || line.starts_with(' ') || line.starts_with('\t'))
            .map(|line| line.trim())
            .collect();
        let joined = continuation.join(if folded { " " } else { "\n" });
        return Some(joined.trim().to_owned());
    }
    let unquoted = if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        let inner = &raw[1..raw.len() - 1];
        if raw.starts_with('"') {
            inner.replace("\\\"", "\"").replace("\\\\", "\\")
        } else {
            inner.replace("''", "'")
        }
    } else {
        raw.to_owned()
    };
    Some(unquoted.trim().to_owned())
}

/// Why a skill could not be read.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SkillError {
    #[error("the skill folder cannot be read")]
    Unreadable,
    #[error("the skill folder holds a link or special file")]
    Link,
    #[error("the skill folder holds an unsafe or repeated path")]
    UnsafePath,
    #[error("the skill folder holds more than {SKILL_MAX_FILES} files")]
    TooManyFiles,
    #[error("the skill folder is larger than {SKILL_MAX_BYTES} bytes")]
    TooLarge,
    #[error("the skill folder nests deeper than {SKILL_MAX_DEPTH} levels")]
    TooDeep,
    #[error("the skill folder has no SKILL.md")]
    MissingSkillFile,
    #[error("SKILL.md has no valid frontmatter")]
    Frontmatter,
    #[error("the skill name is not a valid Agent Skills name")]
    Name,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str) -> SkillFile {
        SkillFile {
            path: path.to_owned(),
            bytes: text.as_bytes().to_vec(),
        }
    }

    const SKILL: &str =
        "---\nname: artisan-editor\ndescription: Write replies that render richly.\n---\n# Body\n";

    #[test]
    fn frontmatter_supports_plain_quoted_and_block_scalars() {
        assert_eq!(
            parse_frontmatter(SKILL).unwrap(),
            (
                "artisan-editor".to_owned(),
                "Write replies that render richly.".to_owned()
            )
        );
        let quoted = "---\nname: \"pdf\"\ndescription: 'It''s for PDFs'\n---\n";
        assert_eq!(
            parse_frontmatter(quoted).unwrap(),
            ("pdf".to_owned(), "It's for PDFs".to_owned())
        );
        let folded =
            "---\nname: pdf\ndescription: >\n  Reads PDFs\n  and forms.\nlicense: MIT\n---\n";
        assert_eq!(
            parse_frontmatter(folded).unwrap().1,
            "Reads PDFs and forms."
        );
        assert_eq!(
            parse_frontmatter("# no frontmatter"),
            Err(SkillError::Frontmatter)
        );
        assert_eq!(
            parse_frontmatter("---\nname: Bad Name\n---\n"),
            Err(SkillError::Name)
        );
    }

    #[test]
    fn packages_sort_files_and_hash_stably() {
        let left = SkillPackage::from_files(vec![
            file("references/blocks.md", "blocks"),
            file(SKILL_FILE, SKILL),
        ])
        .unwrap();
        let right = SkillPackage::from_files(vec![
            file(SKILL_FILE, SKILL),
            file("references/blocks.md", "blocks"),
        ])
        .unwrap();
        assert_eq!(left.digest(), right.digest());
        assert_eq!(left.files()[0].path, SKILL_FILE);
        let changed = SkillPackage::from_files(vec![
            file(SKILL_FILE, SKILL),
            file("references/blocks.md", "blocks!"),
        ])
        .unwrap();
        assert_ne!(left.digest(), changed.digest());
        assert_eq!(left.digest().len(), 64);
    }

    #[test]
    fn packages_refuse_unsafe_paths_and_missing_instructions() {
        for path in ["../x", "/abs", "a//b", "a\\b", "./a", "c:x"] {
            assert_eq!(
                SkillPackage::from_files(vec![file(SKILL_FILE, SKILL), file(path, "x")]),
                Err(SkillError::UnsafePath),
                "{path}"
            );
        }
        assert_eq!(
            SkillPackage::from_files(vec![file("README.md", "x")]),
            Err(SkillError::MissingSkillFile)
        );
    }

    #[test]
    fn folders_are_read_and_links_refused() {
        let root = tempfile::tempdir().unwrap();
        let skill = root.path().join("artisan-editor");
        fs::create_dir_all(skill.join("references")).unwrap();
        fs::write(skill.join(SKILL_FILE), SKILL).unwrap();
        fs::write(skill.join("references/blocks.md"), "blocks").unwrap();
        let package = read_skill_dir(&skill).unwrap();
        assert_eq!(package.name(), "artisan-editor");
        assert_eq!(package.files().len(), 2);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.path(), skill.join("escape")).unwrap();
            assert_eq!(read_skill_dir(&skill), Err(SkillError::Link));
        }
    }
}
