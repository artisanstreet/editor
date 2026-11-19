use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "NtCreateFile is the only call that opens a name relative to a directory handle"
)]
mod windows;

#[derive(Debug, thiserror::Error)]
pub enum OpenBeneathError {
    #[error("{} is a symbolic link or junction", at.display())]
    Linked { at: PathBuf },
    #[error("open {}", at.display())]
    Open {
        at: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum OpenRootError {
    #[error(transparent)]
    Walk(#[from] OpenBeneathError),
    #[error("check which folder {} holds", at.display())]
    Identify {
        at: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} was replaced by another folder while it was opened", at.display())]
    Replaced { at: PathBuf },
}

#[derive(Debug)]
pub(crate) struct Directory(std::fs::File);

impl Directory {
    pub(crate) fn open(canonical: &Path) -> Result<Self, OpenRootError> {
        assert!(
            canonical.is_absolute(),
            "invariant: {} is canonical, so it is absolute",
            canonical.display()
        );
        #[cfg(unix)]
        let opened = unix::directory(canonical);
        #[cfg(windows)]
        let opened = windows::directory(canonical);
        let directory = Self(opened?);
        directory.confirm(canonical)?;
        Ok(directory)
    }

    pub(crate) fn confirm(&self, canonical: &Path) -> Result<(), OpenRootError> {
        #[cfg(unix)]
        let held = unix::held_at(&self.0, canonical);
        #[cfg(windows)]
        let held = windows::held_at(&self.0, canonical);
        match held {
            Ok(true) => Ok(()),
            Ok(false) => Err(OpenRootError::Replaced {
                at: canonical.to_path_buf(),
            }),
            Err(source) => Err(OpenRootError::Identify {
                at: canonical.to_path_buf(),
                source,
            }),
        }
    }
}

fn canonical_names(canonical: &Path) -> impl Iterator<Item = &std::ffi::OsStr> {
    canonical
        .components()
        .filter_map(move |component| match component {
            Component::Normal(name) => Some(name),
            Component::Prefix(_) | Component::RootDir => None,
            Component::CurDir | Component::ParentDir => unreachable!(
                "invariant: {} is canonical, so it holds no . or ..",
                canonical.display()
            ),
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Relative {
    parents: Vec<OsString>,
    name: OsString,
}

impl Relative {
    #[must_use]
    pub(crate) fn path(&self) -> PathBuf {
        self.parents.iter().chain([&self.name]).collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Placed {
    Beneath(Relative),
    Root,
    Outside,
}

fn lexical(path: &Path) -> Vec<Component<'_>> {
    let mut kept: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(kept.last(), Some(Component::Normal(_))) {
                    kept.pop();
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                kept.push(component);
            }
        }
    }
    kept
}

#[cfg(windows)]
fn same_path(left: &Path, right: &Path) -> bool {
    let (left, right) = (lexical(left), lexical(right));
    left.len() == right.len()
        && left
            .iter()
            .zip(&right)
            .all(|(left, right)| same(*left, *right))
}

#[cfg(windows)]
fn same(left: Component<'_>, right: Component<'_>) -> bool {
    left.as_os_str().to_string_lossy().to_uppercase()
        == right.as_os_str().to_string_lossy().to_uppercase()
}

#[cfg(not(windows))]
fn same(left: Component<'_>, right: Component<'_>) -> bool {
    left == right
}

#[must_use]
pub(crate) fn place(path: &Path, roots: &[&Path]) -> Placed {
    let path = lexical(path);
    let below = roots.iter().find_map(|root| {
        let root = lexical(root);
        (path.len() >= root.len()
            && root
                .iter()
                .zip(&path)
                .all(|(root, path)| same(*root, *path)))
        .then(|| path[root.len()..].to_vec())
    });
    let Some(below) = below else {
        return Placed::Outside;
    };
    let mut names: Vec<OsString> = below
        .iter()
        .map(|component| match component {
            Component::Normal(name) => name.to_os_string(),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => {
                unreachable!("invariant: below a root only names remain after normalising")
            }
        })
        .collect();
    match names.pop() {
        Some(name) => Placed::Beneath(Relative {
            parents: names,
            name,
        }),
        None => Placed::Root,
    }
}

pub(crate) fn open_file(
    root: &Directory,
    relative: &Relative,
) -> Result<std::fs::File, OpenBeneathError> {
    #[cfg(unix)]
    let opened = unix::open_file(&root.0, relative);
    #[cfg(windows)]
    let opened = windows::open_file(&root.0, relative);
    opened
}

#[cfg(windows)]
pub(crate) use windows::final_path;
