use super::*;

fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, text) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, path, text.as_bytes())
            .unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

#[test]
fn a_listed_skill_is_found_by_its_name_inside_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = tarball(&[
        ("skills-abc123/README.md", "# Skills"),
        (
            "skills-abc123/skills/frontend/SKILL.md",
            "---\nname: frontend-design\ndescription: Design\n---\n",
        ),
        ("skills-abc123/skills/frontend/notes.md", "notes"),
        (
            "skills-abc123/skills/pdf/SKILL.md",
            "---\nname: pdf\ndescription: PDFs\n---\n",
        ),
    ]);
    unpack(&bytes, dir.path(), UNPACK_LIMITS).unwrap();
    let folder = find_skill(dir.path(), "frontend-design").unwrap().unwrap();
    assert_eq!(folder, dir.path().join("skills/frontend"));
    assert!(folder.join("notes.md").is_file());
    assert!(find_skill(dir.path(), "missing").unwrap().is_none());
}

#[test]
fn a_repository_of_many_small_files_stops_at_the_aggregate_budget() {
    let dir = tempfile::tempdir().unwrap();
    let files: Vec<(String, String)> = (0..6)
        .map(|index| {
            (
                format!("repo-abc/skills/s{index}/notes.md"),
                "x".repeat(400),
            )
        })
        .collect();
    let members: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let bytes = tarball(&members);
    let limits = Limits {
        bytes: 1000,
        ..UNPACK_LIMITS
    };
    let error = unpack(&bytes, dir.path(), limits).unwrap_err();
    assert!(
        matches!(&error, UnpackError::TooLarge(ExpansionError::Bytes { member, limit: 1000 }) if member == Path::new("skills/s2/notes.md")),
        "{error:?}"
    );
    assert!(!dir.path().join("skills/s2").exists());
    let limits = Limits {
        files: 4,
        ..UNPACK_LIMITS
    };
    let error = unpack(&bytes, &dir.path().join("files"), limits).unwrap_err();
    assert!(
        matches!(
            &error,
            UnpackError::TooLarge(ExpansionError::Files { limit: 4, .. })
        ),
        "{error:?}"
    );
    let limits = Limits {
        folders: 3,
        ..UNPACK_LIMITS
    };
    let error = unpack(&bytes, &dir.path().join("folders"), limits).unwrap_err();
    assert!(
        matches!(
            &error,
            UnpackError::TooLarge(ExpansionError::Folders { limit: 3, .. })
        ),
        "{error:?}"
    );
}
