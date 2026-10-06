//! Artisan's own skills, shipped inside the Forge.
//!
//! Release builds embed every file of each first-party skill, so the skill
//! a Forge places always matches the build. Development builds link engines
//! straight to the repository's `skills/<name>` folder instead, so editing a
//! first-party skill reaches every engine on its next session without a
//! rebuild.

use std::path::PathBuf;

use artisan_native_engine::extensions::{SkillError, SkillFile, SkillPackage, read_skill_dir};

/// One first-party skill: its repository folder name and embedded files.
struct FirstPartySkill {
    folder: &'static str,
    files: &'static [(&'static str, &'static [u8])],
}

const SKILLS: &[FirstPartySkill] = &[FirstPartySkill {
    folder: "artisan-editor",
    files: &[
        (
            "SKILL.md",
            include_bytes!("../../../../skills/artisan-editor/SKILL.md"),
        ),
        (
            "references/blocks.md",
            include_bytes!("../../../../skills/artisan-editor/references/blocks.md"),
        ),
    ],
}];

/// A first-party skill ready to place.
#[derive(Clone, Debug)]
pub(crate) struct FirstPartyPackage {
    pub(crate) package: SkillPackage,
    /// The repository folder engines link to in development builds.
    pub(crate) linked_folder: Option<PathBuf>,
}

/// The repository's first-party skills folder in development builds.
pub(crate) fn repository_skills_root() -> Option<PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("skills");
    root.canonicalize().ok().filter(|root| root.is_dir())
}

/// Every first-party skill. A development build whose repository folder
/// reads as a valid skill links to it; otherwise the embedded files are
/// placed.
///
/// # Errors
///
/// Returns [`SkillError`] when an embedded skill is invalid, which the
/// repository's tests rule out.
pub(crate) fn first_party_packages() -> Result<Vec<FirstPartyPackage>, SkillError> {
    let repository = repository_skills_root();
    SKILLS
        .iter()
        .map(|skill| {
            if let Some(folder) = repository.as_ref().map(|root| root.join(skill.folder))
                && let Ok(package) = read_skill_dir(&folder)
            {
                return Ok(FirstPartyPackage {
                    package,
                    linked_folder: Some(folder),
                });
            }
            let package = SkillPackage::from_files(
                skill
                    .files
                    .iter()
                    .map(|(path, bytes)| SkillFile {
                        path: (*path).to_owned(),
                        bytes: bytes.to_vec(),
                    })
                    .collect(),
            )?;
            Ok(FirstPartyPackage {
                package,
                linked_folder: None,
            })
        })
        .collect()
}

/// The identifier of a first-party skill.
pub(crate) fn first_party_id(name: &str) -> String {
    format!("artisan.{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every file in a first-party skill's repository folder is embedded,
    /// and nothing else is: a release Forge places exactly what the
    /// repository holds.
    #[test]
    fn embedded_files_match_the_repository_folders() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills");
        for skill in SKILLS {
            let on_disk = read_skill_dir(&root.join(skill.folder)).unwrap();
            let embedded = SkillPackage::from_files(
                skill
                    .files
                    .iter()
                    .map(|(path, bytes)| SkillFile {
                        path: (*path).to_owned(),
                        bytes: bytes.to_vec(),
                    })
                    .collect(),
            )
            .unwrap();
            assert_eq!(embedded.digest(), on_disk.digest(), "{}", skill.folder);
            assert_eq!(embedded.name(), skill.folder);
        }
        let folders: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        for folder in folders {
            assert!(
                SKILLS.iter().any(|skill| skill.folder == folder),
                "skills/{folder} is not embedded in the Forge"
            );
        }
    }
}
