use crate::beneath::{self, Directory, OpenBeneathError, Placed};
use crate::resources::{Access, ResourceError};
use std::path::Path;
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
    let root = Directory::open(project).unwrap();
    for written in ["shots/secret.txt", "deep/er/linked/secret.txt"] {
        let Placed::Beneath(relative) = beneath::place(&project.join(written), &[project]) else {
            panic!("{written} is beneath the project");
        };
        let opened = beneath::open_file(&root, &relative);
        assert!(
            matches!(opened, Err(OpenBeneathError::Linked { .. })),
            "{written}: {opened:?}"
        );
        #[cfg(unix)]
        {
            let walked = beneath::walk(&root, &relative);
            assert!(
                matches!(walked, Err(OpenBeneathError::Linked { .. })),
                "{written} by walking: {walked:?}"
            );
        }
    }
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
