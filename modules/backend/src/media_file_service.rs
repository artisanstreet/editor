//! Reads the image, audio, and 3D model files a reply names, on the Forge's
//! machine.
//!
//! Agents write paths for the machine they run on, which is this one. The
//! Editor parses the reply's `src` and asks for a clean path plus the folder
//! it is relative to; here it resolves by this operating system's rules:
//! relative to the thread's project, or to this user's home folder, unless
//! absolute. Only image, audio, and model content is served, recognized by
//! its bytes rather than its name, and never more than
//! [`MEDIA_FILE_MAX_BYTES`]. Text model formats (OBJ, ASCII STL, glTF JSON)
//! have no signature, so they also need their extension.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use artisan_domain::{MEDIA_FILE_MAX_BYTES, MediaFilePath, MediaPathAnchor};

/// Why a media file could not be served.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum MediaFileError {
    /// Nothing exists at the resolved path.
    NotFound,
    /// The path names a folder or another non-file.
    NotAFile,
    /// The file exceeds [`MEDIA_FILE_MAX_BYTES`].
    TooLarge,
    /// The bytes are not a known image, audio, or model format.
    Unsupported,
    /// A home-relative path, but this user has no home folder.
    NoHome,
    /// The operating system refused the read.
    Unreadable,
}

impl MediaFileError {
    /// The reader-facing detail; names no path.
    pub(crate) const fn detail(&self) -> &'static str {
        match self {
            Self::NotFound => "the media file does not exist on the Forge's machine",
            Self::NotAFile => "the media path is not a file",
            Self::TooLarge => "the media file is larger than 12 MiB",
            Self::Unsupported => "the media file is not an image, audio, or 3D model file",
            Self::NoHome => "the Forge's user has no home folder",
            Self::Unreadable => "the media file could not be read",
        }
    }
}

/// Resolves `path` on this machine: against `home` for home-anchored paths,
/// else against `project_root` unless absolute.
pub(crate) fn resolve(
    path: &MediaFilePath,
    project_root: &Path,
    home: Option<&Path>,
) -> Result<PathBuf, MediaFileError> {
    let text = Path::new(path.text());
    match path.anchor() {
        MediaPathAnchor::Home => Ok(home.ok_or(MediaFileError::NoHome)?.join(text)),
        MediaPathAnchor::Project if text.is_absolute() => Ok(text.to_path_buf()),
        MediaPathAnchor::Project => Ok(project_root.join(text)),
    }
}

/// Reads one media file, bounded, and only if its bytes are an image,
/// audio, or model format. Blocking: run it off the event loop.
pub(crate) fn read(path: &Path) -> Result<Vec<u8>, MediaFileError> {
    let file = std::fs::File::open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => MediaFileError::NotFound,
        _ => MediaFileError::Unreadable,
    })?;
    let metadata = file.metadata().map_err(|_| MediaFileError::Unreadable)?;
    if !metadata.is_file() {
        return Err(MediaFileError::NotAFile);
    }
    if metadata.len() > MEDIA_FILE_MAX_BYTES as u64 {
        return Err(MediaFileError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MEDIA_FILE_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| MediaFileError::Unreadable)?;
    if bytes.len() > MEDIA_FILE_MAX_BYTES {
        return Err(MediaFileError::TooLarge);
    }
    if !is_media(&bytes) && !is_model(path, &bytes) {
        return Err(MediaFileError::Unsupported);
    }
    Ok(bytes)
}

/// Whether `bytes` begin like an image or audio format the Editor plays.
fn is_media(bytes: &[u8]) -> bool {
    let starts = |prefix: &[u8]| bytes.starts_with(prefix);
    let riff = |kind: &[u8]| bytes.len() >= 12 && starts(b"RIFF") && &bytes[8..12] == kind;
    starts(b"\x89PNG\r\n\x1a\n")
        || starts(b"\xff\xd8\xff")
        || starts(b"GIF87a")
        || starts(b"GIF89a")
        || riff(b"WEBP")
        || starts(b"BM")
        || starts(b"\x00\x00\x01\x00")
        || starts(b"II*\x00")
        || starts(b"MM\x00*")
        || is_svg(bytes)
        // Audio: MP3 with an ID3 tag or a bare frame header, WAV, FLAC, Ogg.
        || starts(b"ID3")
        || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
        || riff(b"WAVE")
        || starts(b"fLaC")
        || starts(b"OggS")
}

/// Whether `bytes` are a 3D model the Editor's `model` block reads. Binary
/// formats carry a signature (binary STL, its length matching its triangle
/// count); text formats must also be named for their format, so an
/// arbitrary text file is never served as a model.
fn is_model(path: &Path, bytes: &[u8]) -> bool {
    let starts = |prefix: &[u8]| bytes.starts_with(prefix);
    if starts(b"glTF") || starts(b"Kaydara FBX Binary  \0") || is_binary_stl(bytes) {
        return true;
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let head = &bytes[..bytes.len().min(64 * 1024)];
    let Ok(text) = std::str::from_utf8(head)
        .or_else(|error| std::str::from_utf8(&head[..error.valid_up_to()]))
    else {
        return false;
    };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    match extension.as_deref() {
        Some("ply") => text.starts_with("ply"),
        Some("stl") => text.starts_with("solid") && text.contains("facet"),
        Some("gltf") => text.starts_with('{') && text.contains("\"asset\""),
        Some("obj") => {
            !head.contains(&0) && text.lines().any(|line| line.trim_start().starts_with("v "))
        }
        _ => false,
    }
}

/// A binary STL: an 80-byte header, a triangle count, 50 bytes per triangle.
fn is_binary_stl(bytes: &[u8]) -> bool {
    let Some(count) = bytes.get(80..84) else {
        return false;
    };
    let count = u64::from(u32::from_le_bytes([count[0], count[1], count[2], count[3]]));
    count > 0 && count * 50 + 84 == bytes.len() as u64
}

/// SVG is text: an `<svg` element within the first kilobyte, after any XML
/// declaration, comment, or byte-order mark.
fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let Ok(text) = std::str::from_utf8(head)
        .or_else(|error| std::str::from_utf8(&head[..error.valid_up_to()]))
    else {
        return false;
    };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with('<') && text.contains("<svg")
}

#[cfg(test)]
mod tests {
    use super::{MediaFileError, read, resolve};
    use artisan_domain::{MediaFilePath, MediaPathAnchor};
    use std::path::Path;

    fn path(text: &str, anchor: MediaPathAnchor) -> MediaFilePath {
        MediaFilePath::new(text.to_owned(), anchor).unwrap()
    }

    #[test]
    fn paths_resolve_by_this_machines_rules() {
        let root = Path::new("/work/app");
        let home = Some(Path::new("/home/me"));
        let resolved = |text, anchor| resolve(&path(text, anchor), root, home);
        assert_eq!(
            resolved("docs/a.png", MediaPathAnchor::Project),
            Ok("/work/app/docs/a.png".into())
        );
        assert_eq!(
            resolved("/mnt/c/a.png", MediaPathAnchor::Project),
            Ok("/mnt/c/a.png".into())
        );
        assert_eq!(
            resolved("Downloads/a.png", MediaPathAnchor::Home),
            Ok("/home/me/Downloads/a.png".into())
        );
        assert_eq!(
            resolve(&path("a.png", MediaPathAnchor::Home), root, None),
            Err(MediaFileError::NoHome)
        );
    }

    #[test]
    fn only_bounded_image_audio_and_model_bytes_are_served() {
        let dir = std::env::temp_dir().join(format!(
            "artisan-media-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let file = dir.join(name);
            std::fs::write(&file, bytes).unwrap();
            file
        };
        let png = write("a.png", b"\x89PNG\r\n\x1a\n rest");
        assert_eq!(read(&png).unwrap(), b"\x89PNG\r\n\x1a\n rest");
        let svg = write(
            "a.svg",
            b"\xef\xbb\xbf<?xml version=\"1.0\"?>\n<svg xmlns=\"x\"/>",
        );
        assert!(read(&svg).is_ok());
        let wav = write("a.wav", b"RIFF\x00\x00\x00\x00WAVEfmt ");
        assert!(read(&wav).is_ok());
        // A name is not a type: text renamed to .png is refused.
        let fake = write("fake.png", b"not an image");
        assert_eq!(read(&fake), Err(MediaFileError::Unsupported));
        // Models: binary formats by signature, text formats by name too.
        assert!(read(&write("a.glb", b"glTF\x02\x00\x00\x00")).is_ok());
        assert!(read(&write("a.fbx", b"Kaydara FBX Binary  \x00\x1a\x00")).is_ok());
        let mut stl = vec![0_u8; 80];
        stl.extend_from_slice(&1_u32.to_le_bytes());
        stl.extend_from_slice(&[0; 50]);
        assert!(read(&write("a.stl", &stl)).is_ok());
        let obj = b"# cube\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
        assert!(read(&write("a.obj", obj)).is_ok());
        assert_eq!(
            read(&write("notes.txt", obj)),
            Err(MediaFileError::Unsupported)
        );
        assert_eq!(
            read(&write("secret.obj", b"api_key = 123")),
            Err(MediaFileError::Unsupported)
        );
        assert_eq!(
            read(&dir.join("missing.png")),
            Err(MediaFileError::NotFound)
        );
        assert_eq!(read(&dir), Err(MediaFileError::NotAFile));
        let large = write(
            "large.png",
            &vec![0x89; artisan_domain::MEDIA_FILE_MAX_BYTES + 1],
        );
        assert_eq!(read(&large), Err(MediaFileError::TooLarge));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
