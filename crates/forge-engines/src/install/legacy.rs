//! Explicit legacy engine import; not part of normal installs.
use super::*;

// ---------------------------------------------------------------- legacy import

/// Brings engines installed by the old Forge (`<old data>/toolchain/<engine>/`)
/// into this layout: the active version and the home, hard-linked where the
/// filesystem allows. Engines that already have a `current` are left alone.
pub(crate) async fn import_legacy(layout: &Layout, old_toolchain: &Path) -> Result<Vec<EngineId>> {
    let mut imported = Vec::new();
    for engine in EngineId::ALL {
        let Some(artifact) = host_spec(engine).artifact else {
            continue;
        };
        let old = old_toolchain.join(host_spec(engine).legacy_dir);
        let state_path = old.join("state.json");
        let state = tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
            use std::io::Read as _;
            let metadata = match fs::metadata(&state_path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => {
                    return Err(error).with_context(|| format!("inspect {}", state_path.display()));
                }
            };
            anyhow::ensure!(
                metadata.is_file(),
                "legacy engine state is not a regular file"
            );
            anyhow::ensure!(
                metadata.len() <= crate::process::MAX_LINE_BYTES as u64,
                "legacy engine state exceeds 32 MiB"
            );
            let file = match fs::File::open(&state_path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => {
                    return Err(error).with_context(|| format!("read {}", state_path.display()));
                }
            };
            anyhow::ensure!(
                file.metadata()?.len() <= crate::process::MAX_LINE_BYTES as u64,
                "legacy engine state exceeds 32 MiB"
            );
            let mut bytes = Vec::new();
            file.take(crate::process::MAX_LINE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() <= crate::process::MAX_LINE_BYTES,
                "legacy engine state exceeds 32 MiB"
            );
            Ok(Some(bytes))
        })
        .await
        .context("read legacy engine state task")??;
        let Some(state) = state else {
            continue;
        };
        let _lock = lock(layout, engine).await?;
        if layout.current(engine).is_some() {
            continue;
        }
        let state: serde_json::Value = serde_json::from_slice(&state)
            .with_context(|| format!("parsing {}", old.join("state.json").display()))?;
        let field = |name: &str| {
            state
                .pointer(&format!("/active/{name}"))
                .and_then(serde_json::Value::as_str)
        };
        let (Some(directory), Some(version)) = (field("directory"), field("version")) else {
            continue;
        };
        check_version(version)?;
        let mut parts = Path::new(directory).components();
        anyhow::ensure!(
            matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none(),
            "legacy engine directory must be one child directory"
        );
        let source = old.join("versions").join(directory);
        if !source.join(artifact.entry).is_file() {
            tracing::warn!(%engine, source = %source.display(), "old install has no {}", artifact.entry);
            continue;
        }
        let (layout, version, old) = (layout.clone(), version.to_owned(), old.clone());
        tokio::task::spawn_blocking(move || -> Result<()> {
            let target = layout.version_dir(engine, &version);
            if !target.exists() {
                let staging = layout.versions(engine).join(format!(".staging-{version}"));
                let _ = fs::remove_dir_all(&staging);
                copy_tree(&source, &staging, true)
                    .with_context(|| format!("copying {}", source.display()))?;
                fs::rename(&staging, &target)?;
            }
            let home = layout.home(engine);
            if old.join("home").is_dir() && !home.exists() {
                copy_tree(&old.join("home"), &home, false)
                    .with_context(|| format!("copying {}", old.join("home").display()))?;
            }
            switch(&layout, engine, &version)
        })
        .await
        .context("import task")??;
        tracing::info!(%engine, "imported the old install");
        imported.push(engine);
    }
    Ok(imported)
}

/// Engine homes are copied in full. Generic cache directories can contain
/// mutable indexes and databases; only installed version artifacts are linked.
/// One maintained, link-preserving walk owns directory traversal for both
/// copied mutable homes and hard-linked immutable tool/cache trees.
fn copy_tree(from: &Path, to: &Path, hard_link: bool) -> io::Result<()> {
    for entry in walkdir::WalkDir::new(from)
        .follow_links(false)
        .follow_root_links(false)
        .max_open(8)
    {
        let entry = entry.map_err(io::Error::other)?;
        let relative = entry.path().strip_prefix(from).map_err(io::Error::other)?;
        let target = to.join(relative);
        let kind = entry.file_type();
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
        } else if kind.is_symlink() {
            let link = fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link, &target)?;
            #[cfg(windows)]
            if entry.path().is_dir() {
                std::os::windows::fs::symlink_dir(&link, &target)?;
            } else {
                std::os::windows::fs::symlink_file(&link, &target)?;
            }
        } else if kind.is_file() {
            if !hard_link || fs::hard_link(entry.path(), &target).is_err() {
                fs::copy(entry.path(), &target)?;
            }
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "legacy tree contains unsupported file type",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod traversal_tests {
    use super::*;
    #[test]
    fn new_engine_session_and_cache_writes_leave_the_old_home_unchanged() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("source");
        let target = folder.path().join("target");
        fs::create_dir_all(source.join(".cache/nested")).unwrap();
        fs::create_dir_all(source.join("sessions/nested")).unwrap();
        fs::write(source.join(".cache/nested/tool"), b"tool").unwrap();
        fs::write(source.join("sessions/nested/history"), b"original").unwrap();
        copy_tree(&source, &target, false).unwrap();
        fs::write(target.join("sessions/nested/history"), b"new history").unwrap();
        fs::write(target.join(".cache/nested/tool"), b"new cache state").unwrap();
        assert_eq!(
            fs::read(source.join("sessions/nested/history")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(source.join(".cache/nested/tool")).unwrap(),
            b"tool"
        );
        assert_eq!(
            fs::read(target.join(".cache/nested/tool")).unwrap(),
            b"new cache state"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_ne!(
                fs::metadata(source.join(".cache/nested/tool"))
                    .unwrap()
                    .ino(),
                fs::metadata(target.join(".cache/nested/tool"))
                    .unwrap()
                    .ino()
            );
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn corrupted_legacy_directory_cannot_adopt_an_unrelated_source_tree() {
        let old = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let home = old.path().join("grok");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("state.json"),
            r#"{"active":{"directory":"../outside","version":"1.0.50"}}"#,
        )
        .unwrap();
        let engines = crate::Engines::new(target.path());
        let error = engines.import_legacy(old.path()).await.unwrap_err();
        assert!(error.to_string().contains("one child directory"));
        assert!(engines.binary(EngineId::Grok).is_none());
    }
    #[cfg(unix)]
    #[test]
    fn native_legacy_links_are_preserved_without_recursive_cycles() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("source");
        let target = folder.path().join("target");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("file"), b"value").unwrap();
        std::os::unix::fs::symlink(".", source.join("cycle")).unwrap();
        copy_tree(&source, &target, false).unwrap();
        assert_eq!(fs::read_link(target.join("cycle")).unwrap(), Path::new("."));
        assert_eq!(fs::read(target.join("file")).unwrap(), b"value");
    }
}
