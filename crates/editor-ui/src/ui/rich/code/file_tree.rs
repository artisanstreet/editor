use serde::Deserialize;
use std::sync::Arc;

mod view;
pub(crate) use view::FileTreeState;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Change {
    #[serde(alias = "add", alias = "new", alias = "created", alias = "create")]
    Added,
    #[serde(
        alias = "modify",
        alias = "changed",
        alias = "updated",
        alias = "edited",
        alias = "edit"
    )]
    Modified,
    #[serde(alias = "delete", alias = "removed", alias = "remove")]
    Deleted,
    #[serde(alias = "rename", alias = "moved", alias = "move")]
    Renamed,
    #[serde(other)]
    Other,
}

impl Change {
    const ALL: [Self; 4] = [Self::Added, Self::Modified, Self::Deleted, Self::Renamed];

    const fn word(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Other => "changed",
        }
    }

    const fn letter(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Other => "•",
        }
    }
}

#[derive(Debug, Deserialize)]
struct EntrySpec {
    path: String,
    #[serde(default)]
    change: Option<Change>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Spec {
    entries: Vec<EntrySpec>,
}

#[derive(Debug)]
struct Entry {
    path: String,
    change: Option<Change>,
    note: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum RowKind {
    Dir { key: String, files: usize },
    File { entry: usize },
}

#[derive(Debug)]
pub(crate) struct Row {
    pub(crate) depth: usize,
    pub(crate) name: String,
    pub(crate) kind: RowKind,
}

#[derive(Debug)]
struct Model {
    entries: Vec<Entry>,
    rows: Vec<Row>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "Spec")]
pub(crate) struct FileTree(Arc<Model>);

impl FileTree {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::{retained_optional, retained_vec};
        std::mem::size_of::<Self>()
            + std::mem::size_of::<Model>()
            + retained_vec(&self.0.entries)
            + self
                .0
                .entries
                .iter()
                .map(|entry| entry.path.capacity() + retained_optional(entry.note.as_ref()))
                .sum::<usize>()
            + retained_vec(&self.0.rows)
            + self
                .0
                .rows
                .iter()
                .map(|row| {
                    row.name.capacity()
                        + match &row.kind {
                            RowKind::Dir { key, .. } => key.capacity(),
                            RowKind::File { .. } => 0,
                        }
                })
                .sum::<usize>()
    }
}

fn segments(path: &str) -> Vec<String> {
    path.trim()
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect()
}

#[derive(Default)]
struct Dir {
    name: String,
    dirs: Vec<Self>,
    indices: std::collections::HashMap<String, usize>,
    count: usize,
    files: Vec<(String, usize)>,
}

impl Dir {
    fn insert(&mut self, parts: &[String], entry: usize) {
        self.count += 1;
        match parts {
            [] => {}
            [file] => self.files.push((file.clone(), entry)),
            [dir, rest @ ..] => {
                let index = *self.indices.entry(dir.clone()).or_insert_with(|| {
                    self.dirs.push(Self {
                        name: dir.clone(),
                        ..Self::default()
                    });
                    self.dirs.len() - 1
                });
                self.dirs[index].insert(rest, entry);
            }
        }
    }

    const fn file_count(&self) -> usize {
        self.count
    }

    fn fold(&mut self) {
        while self.files.is_empty() && self.dirs.len() == 1 {
            let child = self.dirs.remove(0);
            self.name = format!("{}/{}", self.name, child.name);
            self.dirs = child.dirs;
            self.files = child.files;
        }
        for dir in &mut self.dirs {
            dir.fold();
        }
    }

    fn sort(&mut self) {
        self.dirs.sort_by_key(|dir| dir.name.to_lowercase());
        self.files.sort_by_key(|(name, _)| name.to_lowercase());
        for dir in &mut self.dirs {
            dir.sort();
        }
    }

    fn flatten(&self, depth: usize, prefix: &str, rows: &mut Vec<Row>) {
        for dir in &self.dirs {
            let key = format!("{prefix}{}/", dir.name);
            rows.push(Row {
                depth,
                name: String::from(format!("{}/", dir.name)),
                kind: RowKind::Dir {
                    key: String::from(key.clone()),
                    files: dir.file_count(),
                },
            });
            dir.flatten(depth + 1, &key, rows);
        }
        for (name, entry) in &self.files {
            rows.push(Row {
                depth,
                name: String::from(name.clone()),
                kind: RowKind::File { entry: *entry },
            });
        }
    }
}

impl TryFrom<Spec> for FileTree {
    type Error = String;

    fn try_from(spec: Spec) -> Result<Self, String> {
        if spec.entries.len() > 4096 {
            return Err("file-tree preview supports at most 4096 entries".to_owned());
        }
        let mut root = Dir::default();
        let mut entries = Vec::new();
        for entry in spec.entries {
            let parts = segments(&entry.path);
            if parts.len() > 32 {
                return Err("file-tree paths exceed the 32-level preview budget".to_owned());
            }
            if parts.is_empty() {
                continue;
            }
            root.insert(&parts, entries.len());
            entries.push(Entry {
                path: String::from(entry.path.trim().to_owned()),
                change: entry.change,
                note: entry
                    .note
                    .map(|note| note.trim().to_owned())
                    .filter(|note| !note.is_empty())
                    .map(String::from),
            });
        }
        if entries.is_empty() {
            return Err("`entries` names no file".to_owned());
        }
        for dir in &mut root.dirs {
            dir.fold();
        }
        root.sort();
        let mut rows = Vec::new();
        root.flatten(0, "", &mut rows);
        Ok(Self(Arc::new(Model { entries, rows })))
    }
}
