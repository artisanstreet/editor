use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek},
    path::{Component, Path},
};

use crate::{
    error::{BootstrapError, Result, io},
    manifest::ArchiveFormat,
};

const MAX_ARCHIVE_ENTRIES: usize = 16_384;
const MAX_ENTRY_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub fn extract(
    archive: &Path,
    format: ArchiveFormat,
    destination: &Path,
    declared_entries: &[String],
) -> Result<()> {
    if declared_entries.is_empty() || declared_entries.len() > MAX_ARCHIVE_ENTRIES {
        return Err(BootstrapError::UnsafeArchiveEntry(
            "signed archive entry list is empty or exceeds its bound".to_owned(),
        ));
    }
    std::fs::create_dir_all(destination).map_err(io(destination))?;
    match format {
        ArchiveFormat::Zip => extract_zip(
            File::open(archive).map_err(io(archive))?,
            destination,
            declared_entries,
        ),
        ArchiveFormat::TarZstd => {
            let file = File::open(archive).map_err(io(archive))?;
            let decoder = zstd::Decoder::new(file)
                .map_err(|error| BootstrapError::Archive(error.to_string()))?;
            extract_tar(decoder, destination, declared_entries)
        }
    }
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn extract_zip<R: Read + Seek>(
    reader: R,
    destination: &Path,
    declared_entries: &[String],
) -> Result<()> {
    let mut extracted = HashSet::new();
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|error| BootstrapError::Archive(error.to_string()))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(BootstrapError::UnsafeArchiveEntry(
            "ZIP contains too many entries".to_owned(),
        ));
    }
    let mut expanded = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| BootstrapError::Archive(error.to_string()))?;
		let enclosed = entry
			.enclosed_name()
			.ok_or_else(|| BootstrapError::UnsafeArchiveEntry(entry.name().to_owned()))?
			.clone();
        if !safe_relative(&enclosed) || entry.is_symlink() {
            return Err(BootstrapError::UnsafeArchiveEntry(
                entry.name().to_owned(),
            ));
        }
        if entry.size() > MAX_ENTRY_BYTES {
            return Err(BootstrapError::UnsafeArchiveEntry(
                entry.name().to_owned(),
            ));
        }
        expanded = expanded.saturating_add(entry.size());
        if expanded > MAX_EXPANDED_BYTES {
            return Err(BootstrapError::UnsafeArchiveEntry(
                "ZIP expanded size exceeds its bound".to_owned(),
            ));
        }
        let output = destination.join(&enclosed);
        if !entry.is_dir()
            && !declared_entries
                .iter()
                .any(|path| Path::new(path) == enclosed)
        {
            return Err(BootstrapError::UnsafeArchiveEntry(entry.name().to_owned()));
        }
        if !entry.is_dir() && !extracted.insert(enclosed) {
            return Err(BootstrapError::UnsafeArchiveEntry(entry.name().to_owned()));
        }
        if entry.is_dir() {
            std::fs::create_dir_all(&output).map_err(io(&output))?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(io(parent))?;
            }
            let mut file = File::create(&output).map_err(io(&output))?;
            std::io::copy(&mut entry, &mut file).map_err(io(&output))?;
        }
    }
    ensure_declared_entries(&extracted, declared_entries)?;
    Ok(())
}

fn extract_tar<R: Read>(reader: R, destination: &Path, declared_entries: &[String]) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    let mut extracted = HashSet::new();
    let mut entry_count = 0_usize;
    let mut expanded = 0_u64;
    for entry in archive
        .entries()
        .map_err(|error| BootstrapError::Archive(error.to_string()))?
    {
        let mut entry = entry.map_err(|error| BootstrapError::Archive(error.to_string()))?;
        entry_count += 1;
        if entry_count > MAX_ARCHIVE_ENTRIES {
            return Err(BootstrapError::UnsafeArchiveEntry(
                "tar archive contains too many entries".to_owned(),
            ));
        }
        let size = entry
            .header()
            .size()
            .map_err(|error| BootstrapError::Archive(error.to_string()))?;
        if size > MAX_ENTRY_BYTES {
            return Err(BootstrapError::UnsafeArchiveEntry(
                "tar entry exceeds its bound".to_owned(),
            ));
        }
        expanded = expanded.saturating_add(size);
        if expanded > MAX_EXPANDED_BYTES {
            return Err(BootstrapError::UnsafeArchiveEntry(
                "tar expanded size exceeds its bound".to_owned(),
            ));
        }
        let path = entry
            .path()
            .map_err(|error| BootstrapError::Archive(error.to_string()))?
            .into_owned();
        if !safe_relative(&path)
            || !(entry.header().entry_type().is_file() || entry.header().entry_type().is_dir())
        {
            return Err(BootstrapError::UnsafeArchiveEntry(
                path.display().to_string(),
            ));
        }
        let output = destination.join(&path);
        if entry.header().entry_type().is_file()
            && !declared_entries
                .iter()
                .any(|declared| Path::new(declared) == path)
        {
            return Err(BootstrapError::UnsafeArchiveEntry(
                path.display().to_string(),
            ));
        }
        if entry.header().entry_type().is_file() && !extracted.insert(path.clone()) {
            return Err(BootstrapError::UnsafeArchiveEntry(
                path.display().to_string(),
            ));
        }
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&output).map_err(io(&output))?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(io(parent))?;
            }
            entry.unpack(&output).map_err(io(&output))?;
        }
    }
    ensure_declared_entries(&extracted, declared_entries)?;
    Ok(())
}

fn ensure_declared_entries(
    extracted: &HashSet<std::path::PathBuf>,
    declared: &[String],
) -> Result<()> {
    if declared.len() != extracted.len()
        || declared
            .iter()
            .any(|path| !extracted.contains(Path::new(path)))
    {
        return Err(BootstrapError::UnsafeArchiveEntry(
            "archive entries do not exactly match the signed manifest".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    use super::extract_zip;

    #[test]
    fn rejects_zip_traversal() {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            writer
                .start_file("../outside", SimpleFileOptions::default())
                .expect("entry");
            writer.write_all(b"bad").expect("contents");
            writer.finish().expect("finish");
        }
        let destination = tempdir().expect("temp");
        assert!(
            extract_zip(
                Cursor::new(bytes.into_inner()),
                destination.path(),
                &["../outside".to_owned()]
            )
            .is_err()
        );
    }
}
