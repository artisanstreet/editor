use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::escape_framing;

const MAX_SKILLS: usize = 256;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_SKILL_BYTES: u64 = 256 * 1024;
const MAX_FILES: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Skill {
    pub name: String,
    pub description: String,
    pub folder: PathBuf,
}

#[must_use]
pub(super) fn discover(home: &Path, cwd: &Path) -> Vec<Skill> {
    discover_with(home, cwd, user_home().as_deref())
}

fn discover_with(home: &Path, cwd: &Path, user_home: Option<&Path>) -> Vec<Skill> {
    let mut project: Vec<_> = cwd.ancestors().take_while(|dir| *dir != home).collect();
    if let Some(root) = project.iter().position(|dir| dir.join(".git").exists()) {
        project.truncate(root + 1);
    } else if user_home.is_some_and(|user_home| same_dir(cwd, user_home)) {
        project.clear();
    } else {
        project.truncate(1);
    }
    let mut roots: Vec<PathBuf> = project
        .into_iter()
        .map(|dir| dir.join(".claude").join("skills"))
        .collect();
    roots.push(home.join(".claude").join("skills"));
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

fn user_home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

fn same_dir(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

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

#[must_use]
pub(super) fn definition() -> Value {
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

#[must_use]
pub(super) fn listing(skills: &[Skill]) -> String {
    let mut text = String::new();
    for skill in skills {
        let name = escape_framing(&skill.name);
        let description = escape_framing(skill.description.trim());
        if description.is_empty() {
            writeln!(text, "- {name}").expect("invariant: writing to a String cannot fail");
        } else {
            writeln!(text, "- {name}: {description}")
                .expect("invariant: writing to a String cannot fail");
        }
    }
    text
}

pub(super) fn load(skills: &[Skill], name: &str) -> Result<String, String> {
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
            writeln!(out, "  {file}").expect("invariant: writing to a String cannot fail");
        }
        out.push_str("Read them with Read, with paths under the base directory.\n");
    }
    out.push_str(&escape_framing(body.trim()));
    out.push_str("\n</skill_content>");
    Ok(out)
}

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
        if header.split_at(start).1.trim() == "---" {
            return Ok(header);
        }
    }
}

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

fn without_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

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
#[path = "../../tests/claudeai/skills.rs"]
mod tests;
