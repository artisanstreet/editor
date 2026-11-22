use super::*;
use crate::engines::Feed;

const ARTIFACT: Artifact = Artifact {
    feed: Feed::Npm {
        package: "engine",
        dist_tag: "latest",
        suffix: "",
    },
    archive: Archive::TarGz,
    strip: "package",
    entry: "bin/engine",
    path_dirs: &[],
};

const ROOMY: Limits = Limits {
    bytes: 1024 * 1024,
    files: 100,
    folders: 100,
    spare_space: 0,
};

fn archive(folder: &Path, members: &[(&str, usize)]) -> PathBuf {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, size) in members {
        let body = vec![0_u8; *size];
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("package/{path}"), body.as_slice())
            .unwrap();
    }
    let path = folder.join("download.tgz");
    fs::write(&path, builder.into_inner().unwrap().finish().unwrap()).unwrap();
    path
}

fn unpacked(dest: &Path) -> Vec<String> {
    let mut found: Vec<String> = walkdir::WalkDir::new(dest)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            entry
                .path()
                .strip_prefix(dest)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    found.sort();
    found
}

#[test]
fn an_archive_that_expands_past_the_byte_budget_stops_before_writing_the_excess() {
    let folder = tempfile::tempdir().unwrap();
    let download = archive(
        folder.path(),
        &[
            ("bin/engine", 600),
            ("data/a.bin", 600),
            ("data/b.bin", 600),
        ],
    );
    let dest = folder.path().join("staging");
    let limits = Limits {
        bytes: 1000,
        ..ROOMY
    };
    let error = unpack(ARTIFACT, &download, &dest, limits).unwrap_err();
    assert!(
        matches!(
            &error,
            UnpackError::TooLarge(ExpansionError::Bytes { member, limit: 1000 })
                if member == Path::new("data/a.bin")
        ),
        "{error:?}"
    );
    assert_eq!(unpacked(&dest), ["bin/engine"]);
}

#[test]
fn an_archive_with_too_many_members_is_refused() {
    let folder = tempfile::tempdir().unwrap();
    let download = archive(
        folder.path(),
        &[("bin/engine", 8), ("a", 1), ("b", 1), ("c", 1)],
    );
    let dest = folder.path().join("staging");
    let limits = Limits { files: 3, ..ROOMY };
    let error = unpack(ARTIFACT, &download, &dest, limits).unwrap_err();
    assert!(
        matches!(&error, UnpackError::TooLarge(ExpansionError::Files { member, limit: 3 }) if member == Path::new("c")),
        "{error:?}"
    );
    let deep = archive(folder.path(), &[("bin/engine", 8), ("a/b/c/d/e", 1)]);
    let error = unpack(
        ARTIFACT,
        &deep,
        &dest,
        Limits {
            folders: 4,
            ..ROOMY
        },
    )
    .unwrap_err();
    assert!(
        matches!(
            &error,
            UnpackError::TooLarge(ExpansionError::Folders { limit: 4, .. })
        ),
        "{error:?}"
    );
}

#[test]
fn an_archive_larger_than_the_free_space_writes_nothing() {
    let folder = tempfile::tempdir().unwrap();
    let download = archive(folder.path(), &[("bin/engine", 8)]);
    let dest = folder.path().join("staging");
    let limits = Limits {
        spare_space: u64::MAX / 2,
        ..ROOMY
    };
    let error = unpack(ARTIFACT, &download, &dest, limits).unwrap_err();
    assert!(
        matches!(&error, UnpackError::TooLarge(ExpansionError::Space { .. })),
        "{error:?}"
    );
    assert!(unpacked(&dest).is_empty());
    unpack(ARTIFACT, &download, &dest, ROOMY).unwrap();
    assert_eq!(unpacked(&dest), ["bin/engine"]);
}
