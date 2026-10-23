//! The skills the claude.ai transport offers the model.
//!
//! Claude Code finds skills in `.claude/skills` folders; Forge materialises
//! the enabled ones into the engine home before every run. The transport
//! reads the same folders (the home's, and the project's, as Claude Code
//! does), lists each skill's name and description in the preamble, and loads
//! a skill's `SKILL.md` on demand when the model calls the `Skill` tool —
//! the same on-demand loading Claude Code does.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::escape_framing;

/// The most skills the transport offers.
const MAX_SKILLS: usize = 256;
/// The most of a skill's metadata that is read.
const MAX_HEADER_BYTES: usize = 64 * 1024;
/// The largest `SKILL.md` the Skill tool loads.
const MAX_SKILL_BYTES: u64 = 256 * 1024;
/// The most bundled files the Skill tool lists.
const MAX_FILES: usize = 200;

/// One skill the model may load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub folder: PathBuf,
}

/// Every skill reachable from `home` and `cwd`, by name. The project's
/// skills override the home's when the names collide, as in Claude Code.
#[must_use]
pub fn discover(home: &Path, cwd: &Path) -> Vec<Skill> {
    discover_with(home, cwd, user_home().as_deref())
}

/// [`discover`], with the user's own home given.
fn discover_with(home: &Path, cwd: &Path, user_home: Option<&Path>) -> Vec<Skill> {
    // The project's skills, from the working directory up to the
    // repository root, so the nearest definition wins.
    let mut project: Vec<_> = cwd.ancestors().take_while(|dir| *dir != home).collect();
    if let Some(root) = project.iter().position(|dir| dir.join(".git").exists()) {
        project.truncate(root + 1);
    } else if user_home.is_some_and(|user_home| same_dir(cwd, user_home)) {
        // Outside a repository, the user's home is not a project: its
        // `.claude/skills` are the ones Forge enables and disables, and
        // reading them here would bring back the disabled ones.
        project.clear();
    } else {
        project.truncate(1);
    }
    let mut roots: Vec<PathBuf> = project
        .into_iter()
        .map(|dir| dir.join(".claude").join("skills"))
        .collect();
    roots.push(home.join(".claude").join("skills"));
    // The project's first, so the first definition of a name wins and a
    // home with many skills cannot crowd the project's out.
    let mut found: BTreeMap<String, Skill> = BTreeMap::new();
    for root in roots {
        for skill in in_root(&root) {
            if found.len() >= MAX_SKILLS {
                break;
            }
            if let Entry::Vacant(slot) = found.entry(skill.name.clone()) {
                slot.insert(skill);
            }
        }
    }
    found.into_values().collect()
}

/// The user's own home directory.
fn user_home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

/// Whether two paths are the same directory, links resolved.
fn same_dir(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

/// The skills one `.claude/skills` folder holds.
fn in_root(root: &Path) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut skills = Vec::new();
    for entry in entries.flatten() {
        if skills.len() >= MAX_SKILLS {
            break;
        }
        let folder = entry.path();
        let folder_name = entry.file_name().to_string_lossy().into_owned();
        if folder_name.starts_with('.') || !folder.is_dir() {
            continue;
        }
        let path = folder.join("SKILL.md");
        let header = match read_header(&path) {
            Ok(header) => header,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                tracing::warn!(path = %path.display(), step = "reading a skill", "the skill is skipped: {error}");
                continue;
            }
        };
        let Some((name, description)) = metadata(&header, &folder_name) else {
            tracing::warn!(path = %path.display(), step = "reading a skill", "the skill is skipped: its name is not usable");
            continue;
        };
        skills.push(Skill {
            name,
            description,
            folder,
        });
    }
    skills
}

/// The `Skill` tool's definition.
#[must_use]
pub fn definition() -> Value {
    json!({
        "name": "Skill",
        "description": "Loads a skill's full instructions, when the task matches its description. Skills carry procedures and reference files the user installed.",
        "input_schema": {
            "type": "object",
            "properties": {
                "skill": {"type": "string", "description": "The skill's name, as listed in <available_skills>."},
                "args": {"type": "string", "description": "Optional arguments the skill's instructions take."}
            },
            "required": ["skill"]
        }
    })
}

/// The `<available_skills>` block the preamble carries, or empty when there
/// are no skills.
#[must_use]
pub fn listing(skills: &[Skill]) -> String {
    let mut text = String::new();
    for skill in skills {
        let name = escape_framing(&skill.name);
        let description = escape_framing(skill.description.trim());
        if description.is_empty() {
            writeln!(text, "- {name}").ok();
        } else {
            writeln!(text, "- {name}: {description}").ok();
        }
    }
    text
}

/// Loads the skill `name` names: its `SKILL.md` body and the files it
/// bundles, so the model can read them with Read.
pub fn load(skills: &[Skill], name: &str) -> Result<String, String> {
    let name = name.trim();
    let Some(skill) = skills.iter().find(|skill| skill.name == name).or_else(|| {
        skills
            .iter()
            .find(|skill| skill.name.eq_ignore_ascii_case(name))
    }) else {
        let known = skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(if known.is_empty() {
            "There are no skills on this machine.".to_owned()
        } else {
            format!("There is no skill called {name}. Available: {known}.")
        });
    };
    let path = skill.folder.join("SKILL.md");
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    if metadata.len() > MAX_SKILL_BYTES {
        return Err(format!(
            "{} is larger than {} KiB; read it in parts with Read.",
            path.display(),
            MAX_SKILL_BYTES / 1024
        ));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let body = strip_front_matter(&text);
    let mut out = format!(
        "<skill_content name=\"{}\">\nBase directory for this skill: {}\n",
        escape_framing(&skill.name).replace('"', "&quot;"),
        skill.folder.display()
    );
    let files = bundled(&skill.folder);
    if !files.is_empty() {
        out.push_str("Files bundled with this skill:\n");
        for file in &files {
            writeln!(out, "  {file}").ok();
        }
        out.push_str("Read them with Read, with paths under the base directory.\n");
    }
    out.push_str(&escape_framing(body.trim()));
    out.push_str("\n</skill_content>");
    Ok(out)
}

/// The files a skill bundles, relative to its folder, bounded.
fn bundled(folder: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let walk = walkdir::WalkDir::new(folder)
        .min_depth(1)
        .max_depth(8)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !entry.file_name().to_string_lossy().starts_with('.'));
    for entry in walk.filter_map(Result::ok) {
        if files.len() >= MAX_FILES {
            break;
        }
        if entry.file_type().is_file() && entry.file_name() != "SKILL.md" {
            let relative = entry
                .path()
                .strip_prefix(folder)
                .unwrap_or_else(|_| entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            files.push(relative);
        }
    }
    files.sort();
    files
}

/// A `SKILL.md`'s YAML front matter, read bounded.
fn read_header(path: &Path) -> std::io::Result<String> {
    use std::io::BufRead as _;
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file).take(MAX_HEADER_BYTES as u64);
    let mut header = String::new();
    if reader.read_line(&mut header)? == 0 || without_bom(&header).trim() != "---" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "skill lacks YAML front matter",
        ));
    }
    loop {
        let start = header.len();
        if reader.read_line(&mut header)? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "skill metadata is unclosed",
            ));
        }
        if header[start..].trim() == "---" {
            return Ok(header);
        }
    }
}

/// The `name` and `description` in a `SKILL.md`'s front matter, as
/// Claude Code reads them: a skill without a name is named for its folder,
/// and front matter that is not valid YAML (`description: Use when: X`)
/// is read line by line.
fn metadata(text: &str, folder: &str) -> Option<(String, String)> {
    let mut lines = without_bom(text).lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut yaml = String::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    let data: Option<serde_yaml_ng::Value> = serde_yaml_ng::from_str(&yaml).ok();
    let field = |key: &str| {
        let value = match &data {
            Some(data) => data.get(key)?.as_str()?.to_owned(),
            None => lenient_field(&yaml, key)?,
        };
        Some(value.trim().to_owned()).filter(|value| !value.is_empty())
    };
    let name = field("name").unwrap_or_else(|| folder.to_owned());
    if !artisan_mentions::valid_mention_name(&name) {
        return None;
    }
    let mut description = field("description").unwrap_or_default();
    if description.len() > 1024 {
        let end = description.floor_char_boundary(1024);
        description.truncate(end);
    }
    Some((name, description))
}

/// A top-level `key: value` line's value, for front matter YAML refuses:
/// everything after the first `: `, unquoted.
fn lenient_field(yaml: &str, key: &str) -> Option<String> {
    yaml.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name != key {
            return None;
        }
        let value = value.trim();
        let unquoted = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        Some(unquoted.to_owned())
    })
}

/// Text without the UTF-8 byte-order mark some editors write first.
fn without_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// The `SKILL.md` body, without its front matter.
fn strip_front_matter(text: &str) -> &str {
    let text = without_bom(text);
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return text;
    };
    if first.trim() != "---" {
        return text;
    }
    let mut offset = first.len();
    for line in lines {
        offset += line.len();
        if line.trim() == "---" {
            return text.get(offset..).unwrap_or(text);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(root: &Path, name: &str, description: &str) {
        let folder = root.join(".claude/skills").join(name);
        std::fs::create_dir_all(folder.join("references")).unwrap();
        std::fs::write(
            folder.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nDo the {name} thing.\n"),
        )
        .unwrap();
        std::fs::write(folder.join("references/blocks.md"), "block catalog\n").unwrap();
    }

    #[test]
    fn skills_come_from_the_home_and_the_project_with_the_project_winning() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".git")).unwrap();
        skill(home.path(), "review", "Review code");
        skill(home.path(), "deploy", "Deploy things");
        skill(project.path(), "deploy", "Deploy this project");
        let skills = discover(home.path(), project.path());
        assert_eq!(skills.len(), 2);
        let deploy = skills
            .iter()
            .find(|skill| skill.name == "deploy")
            .expect("the project's deploy");
        assert_eq!(deploy.description, "Deploy this project");
        assert!(deploy.folder.starts_with(project.path()));
    }

    #[test]
    fn a_skill_loads_with_its_body_and_bundled_files() {
        let home = tempfile::tempdir().unwrap();
        skill(home.path(), "review", "Review code");
        let skills = discover(home.path(), home.path());
        let loaded = load(&skills, "review").expect("the skill loads");
        assert!(loaded.contains("Do the review thing."));
        assert!(loaded.contains("references/blocks.md"));
        assert!(
            !loaded.contains("description: Review code"),
            "front matter is not part of the instructions"
        );
        assert!(load(&skills, "missing").unwrap_err().contains("no skill"));
    }

    #[test]
    fn an_invalid_or_hidden_skill_is_not_listed() {
        let home = tempfile::tempdir().unwrap();
        let skills_root = home.path().join(".claude/skills");
        std::fs::create_dir_all(skills_root.join("broken")).unwrap();
        std::fs::write(skills_root.join("broken/SKILL.md"), "no front matter").unwrap();
        std::fs::create_dir_all(skills_root.join(".hidden")).unwrap();
        std::fs::write(skills_root.join(".hidden/SKILL.md"), "---\nname: x\n---\n").unwrap();
        assert!(discover(home.path(), home.path()).is_empty());
    }

    #[test]
    fn a_skill_with_a_bom_no_name_or_loose_yaml_is_still_listed() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".claude/skills");
        for (folder, text) in [
            (
                "bom",
                "\u{feff}---\nname: bom\ndescription: Saved by Notepad\n---\nBody.\n",
            ),
            (
                "nameless",
                "---\ndescription: Named for its folder\n---\nBody.\n",
            ),
            (
                "loose",
                "---\nname: loose\ndescription: Use when: the YAML is loose\n---\nBody.\n",
            ),
        ] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            std::fs::write(root.join(folder).join("SKILL.md"), text).unwrap();
        }
        let skills = discover(home.path(), home.path());
        let listed: Vec<(&str, &str)> = skills
            .iter()
            .map(|skill| (skill.name.as_str(), skill.description.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                ("bom", "Saved by Notepad"),
                ("loose", "Use when: the YAML is loose"),
                ("nameless", "Named for its folder"),
            ]
        );
        let loaded = load(&skills, "bom").unwrap();
        assert!(
            loaded.contains("Body.") && !loaded.contains("Saved by Notepad"),
            "{loaded}"
        );
    }

    #[test]
    fn the_users_home_outside_a_repository_is_not_a_project() {
        let engine_home = tempfile::tempdir().unwrap();
        let user_home = tempfile::tempdir().unwrap();
        // Forge turned this one off: it is in the user's skills, not the
        // engine home's.
        skill(user_home.path(), "disabled", "Turned off in Forge");
        skill(engine_home.path(), "enabled", "Turned on in Forge");
        let names = |skills: Vec<Skill>| {
            skills
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(discover_with(
                engine_home.path(),
                user_home.path(),
                Some(user_home.path())
            )),
            ["enabled"]
        );
        std::fs::create_dir_all(user_home.path().join(".git")).unwrap();
        assert_eq!(
            names(discover_with(
                engine_home.path(),
                user_home.path(),
                Some(user_home.path())
            )),
            ["disabled", "enabled"],
            "a home that is a repository is that repository's project"
        );
    }

    #[test]
    fn a_crowded_home_does_not_crowd_out_the_projects_skills() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".git")).unwrap();
        for index in 0..MAX_SKILLS {
            skill(home.path(), &format!("home-{index:03}"), "From the home");
        }
        skill(project.path(), "zz-project", "From the project");
        let skills = discover(home.path(), project.path());
        assert_eq!(skills.len(), MAX_SKILLS);
        assert!(skills.iter().any(|skill| skill.name == "zz-project"));
    }

    #[test]
    fn an_exact_name_wins_over_one_that_differs_in_case() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".claude/skills");
        for (folder, name) in [("upper", "Review"), ("lower", "review")] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            std::fs::write(
                root.join(folder).join("SKILL.md"),
                format!("---\nname: {name}\n---\nThe {folder} one.\n"),
            )
            .unwrap();
        }
        let skills = discover(home.path(), home.path());
        assert!(load(&skills, "review").unwrap().contains("The lower one."));
        assert!(load(&skills, "Review").unwrap().contains("The upper one."));
    }
}
