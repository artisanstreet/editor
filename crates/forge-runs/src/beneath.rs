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

#[derive(Debug)]
pub(crate) struct Directory(std::fs::File);

impl Directory {
    pub(crate) fn open(path: &Path) -> std::io::Result<Self> {
        #[cfg(unix)]
        let opened = unix::directory(path);
        #[cfg(windows)]
        let opened = windows::directory(path);
        opened.map(Self)
    }
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

#[cfg(all(unix, test))]
pub(crate) fn walk(
    root: &Directory,
    relative: &Relative,
) -> Result<std::fs::File, OpenBeneathError> {
    unix::walk(&root.0, relative)
}

#[cfg(windows)]
pub(crate) use windows::final_path;
