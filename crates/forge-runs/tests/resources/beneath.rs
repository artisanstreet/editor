use crate::beneath::{self, Directory, OpenBeneathError, OpenRootError, Placed};
use crate::resources::{Access, ResourceError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

const OUTSIDE: &[u8] = b"the user's private key";

fn link_folder(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(target, link).unwrap();
}

struct Planted {
    home: tempfile::TempDir,
    project: tempfile::TempDir,
    outside: tempfile::TempDir,
}

fn planted() -> Planted {
    let planted = Planted {
        home: tempfile::tempdir().unwrap(),
        project: tempfile::tempdir().unwrap(),
        outside: tempfile::tempdir().unwrap(),
    };
    std::fs::create_dir_all(planted.project.path().join("images")).unwrap();
    std::fs::write(
        planted.project.path().join("images/secret.txt"),
        b"an innocent image",
    )
    .unwrap();
    std::fs::write(planted.outside.path().join("secret.txt"), OUTSIDE).unwrap();
    planted
}

#[test]
fn a_folder_swapped_for_a_link_between_the_check_and_the_open_reads_nothing_outside() {
    let planted = planted();
    let project = planted.project.path().to_path_buf();
    let outside = planted.outside.path().to_path_buf();
    let access = Access::load(planted.home.path(), &project);
    let barrier = Arc::new(Barrier::new(2));
    let swapper = std::thread::spawn({
        let barrier = Arc::clone(&barrier);
        let project = project.clone();
        move || {
            barrier.wait();
            std::fs::rename(project.join("images"), project.join("images.old")).unwrap();
            link_folder(&outside, &project.join("images"));
            barrier.wait();
        }
    });

    let located = access
        .locate(&project.join("images/secret.txt"))
        .expect("the file is inside the project and allowed when checked");
    barrier.wait();
    barrier.wait();
    swapper.join().unwrap();
    let read = access.read_located(&located);

    assert!(
        matches!(read, Err(ResourceError::Linked { .. })),
        "read through the swapped folder: {read:?}"
    );
}

#[test]
fn a_link_anywhere_on_the_way_is_refused_by_every_opener() {
    let planted = planted();
    let project = planted.project.path();
    link_folder(planted.outside.path(), &project.join("shots"));
    std::fs::create_dir_all(project.join("deep/er")).unwrap();
    link_folder(planted.outside.path(), &project.join("deep/er/linked"));
    let root = Directory::open(&canonical(project)).unwrap();
    for written in ["shots/secret.txt", "deep/er/linked/secret.txt"] {
        let Placed::Beneath(relative) = beneath::place(&project.join(written), &[project]) else {
            panic!("{written} is beneath the project");
        };
        let opened = beneath::open_file(&root, &relative);
        assert!(
            matches!(opened, Err(OpenBeneathError::Linked { .. })),
            "{written}: {opened:?}"
        );
    }
}

fn canonical(path: &Path) -> PathBuf {
    crate::resources::without_verbatim_prefix(&std::fs::canonicalize(path).unwrap())
}

fn read_secret(root: &Directory) -> Vec<u8> {
    let Placed::Beneath(relative) =
        beneath::place(Path::new("/project/secret.txt"), &[Path::new("/project")])
    else {
        panic!("secret.txt is beneath the project");
    };
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut beneath::open_file(root, &relative).unwrap(),
        &mut bytes,
    )
    .unwrap();
    bytes
}

struct Nested {
    folders: PathBuf,
    project: PathBuf,
    outside: PathBuf,
    _parent: tempfile::TempDir,
    _outside: tempfile::TempDir,
}

fn nested() -> Nested {
    let parent = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let folders = parent.path().join("folders");
    let project = folders.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("secret.txt"), b"the project's own notes").unwrap();
    let elsewhere = outside.path().join("project");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("secret.txt"), OUTSIDE).unwrap();
    Nested {
        folders,
        project,
        outside: outside.path().to_path_buf(),
        _parent: parent,
        _outside: outside,
    }
}

#[test]
fn a_project_swapped_for_a_link_after_it_was_resolved_opens_nothing_outside() {
    let nested = nested();
    let resolved = canonical(&nested.project);
    std::fs::rename(&nested.project, nested.folders.join("project.old")).unwrap();
    link_folder(&nested.outside.join("project"), &nested.project);

    let opened = Directory::open(&resolved);

    assert!(
        matches!(
            opened,
            Err(OpenRootError::Walk(OpenBeneathError::Linked { .. }))
        ),
        "opened the swapped project: {:?}",
        opened.map(|root| String::from_utf8_lossy(&read_secret(&root)).into_owned())
    );
}

#[test]
fn a_projects_parent_swapped_for_a_link_after_it_was_resolved_opens_nothing_outside() {
    let nested = nested();
    let resolved = canonical(&nested.project);
    let parent = nested.folders.parent().unwrap().join("folders.old");
    std::fs::rename(&nested.folders, &parent).unwrap();
    link_folder(&nested.outside, &nested.folders);

    let opened = Directory::open(&resolved);

    assert!(
        matches!(
            opened,
            Err(OpenRootError::Walk(OpenBeneathError::Linked { .. }))
        ),
        "opened through the swapped parent: {:?}",
        opened.map(|root| String::from_utf8_lossy(&read_secret(&root)).into_owned())
    );
}

#[test]
fn a_project_replaced_after_it_was_opened_is_not_trusted_as_the_project() {
    let nested = nested();
    let resolved = canonical(&nested.project);
    let root = Directory::open(&resolved).unwrap();
    assert_eq!(read_secret(&root), b"the project's own notes");

    std::fs::rename(&nested.project, nested.folders.join("project.old")).unwrap();
    std::fs::create_dir(&nested.project).unwrap();

    let confirmed = root.confirm(&resolved);
    assert!(
        matches!(confirmed, Err(OpenRootError::Replaced { .. })),
        "{confirmed:?}"
    );
}

#[test]
fn a_project_reached_through_a_link_the_user_made_still_opens() {
    let nested = nested();
    let linked = nested.folders.join("shortcut");
    link_folder(&nested.project, &linked);
    let home = tempfile::tempdir().unwrap();

    let read = Access::load(home.path(), &linked).read(&linked.join("secret.txt"));

    assert_eq!(read.unwrap(), b"the project's own notes");
}

#[test]
fn dot_dot_cannot_climb_out_of_the_project() {
    let planted = planted();
    let project = planted.project.path();
    let climbing = project
        .join("images/../..")
        .join(planted.outside.path().file_name().unwrap());
    let access = Access::load(planted.home.path(), project);
    let read = access.read(&climbing.join("secret.txt"));
    assert!(
        matches!(read, Err(ResourceError::Outside { .. })),
        "{read:?}"
    );
    assert_eq!(
        access
            .read(&project.join("images/../images/./secret.txt"))
            .unwrap(),
        b"an innocent image"
    );
}
