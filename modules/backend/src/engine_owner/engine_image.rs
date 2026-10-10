//! Engine-shown images onto disk.
//!
//! Codex hands a generated image over either as a file it already wrote
//! (`savedPath`) or inline as base64 (`result`). Observations
//! never carry pixels, so inline images are written once, content-addressed,
//! to `<forge state>/engine-images/<sha256>.<ext>`, and the observation
//! records that absolute path.
//!
//! The store root is the Forge's state directory (the database's parent),
//! set once at startup. Without it, inline images are dropped with a log
//! line rather than written somewhere unowned.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Directory under the Forge state root that holds engine images.
pub(crate) const ENGINE_IMAGE_DIRECTORY: &str = "engine-images";

/// Largest decoded inline image kept. Engine frames are capped at 8 MiB of
/// JSON, so a base64 payload inside one decodes to well under this.
pub(crate) const ENGINE_IMAGE_MAX_BYTES: usize = 8 * 1024 * 1024;

static STORE_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Sets where inline engine images are written: `state_root/engine-images`.
/// Only the first call takes effect.
pub(crate) fn set_state_root(state_root: &Path) {
    let _ = STORE_ROOT.set(state_root.join(ENGINE_IMAGE_DIRECTORY));
}

/// A complete 1x1 PNG, for tests.
#[cfg(test)]
pub(crate) const PIXEL_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

/// Points the store at a per-process temporary state root for tests and
/// returns the image directory inside it.
#[cfg(test)]
pub(crate) fn use_test_state_root() -> &'static Path {
    let root = std::env::temp_dir().join(format!("artisan-engine-images-{}", std::process::id()));
    set_state_root(&root);
    STORE_ROOT
        .get()
        .map_or_else(|| Path::new(""), PathBuf::as_path)
}

/// Why one inline engine image could not be stored. Each variant names the
/// step that failed.
#[derive(Debug, Error)]
pub(crate) enum EngineImageError {
    #[error("engine image store root is not configured")]
    Unconfigured,
    #[error("decoding the base64 image payload failed")]
    Decode {
        #[source]
        source: base64::DecodeError,
    },
    #[error("the image payload is empty")]
    Empty,
    #[error("the image payload is {length} bytes, over the {maximum}-byte limit")]
    TooLarge { length: usize, maximum: usize },
    #[error("the image payload is not a PNG, JPEG, GIF, or WebP image")]
    UnsupportedFormat,
    #[error("creating the engine image directory {} failed", path.display())]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing the engine image {} failed", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Writes one inline image under the configured store root and returns its
/// absolute path as text. Failures are logged with `engine` and `item_id`
/// and return [`None`]: a lost image never disturbs the turn.
pub(crate) fn persist_inline_image(engine: &str, item_id: &str, payload: &str) -> Option<String> {
    let stored = STORE_ROOT
        .get()
        .ok_or(EngineImageError::Unconfigured)
        .and_then(|root| store_inline_image(root, payload));
    match stored {
        Ok(path) => path.to_str().map(str::to_owned),
        Err(error) => {
            eprintln!(
                "engine image dropped: engine={engine} item={item_id}: {}",
                artisan_domain::error_chain::ErrorChain(&error)
            );
            None
        }
    }
}

/// Decodes one base64 image (bare or as a `data:` URL) and writes it to
/// `root/<sha256>.<ext>`, reusing an identical file already there.
///
/// The extension follows the decoded bytes, never a declared media type.
///
/// # Errors
///
/// Returns [`EngineImageError`] naming the failed step: decode, bounds,
/// format sniffing, directory creation, or the write.
pub(crate) fn store_inline_image(root: &Path, payload: &str) -> Result<PathBuf, EngineImageError> {
    let bytes = decode_payload(payload)?;
    if bytes.is_empty() {
        return Err(EngineImageError::Empty);
    }
    if bytes.len() > ENGINE_IMAGE_MAX_BYTES {
        return Err(EngineImageError::TooLarge {
            length: bytes.len(),
            maximum: ENGINE_IMAGE_MAX_BYTES,
        });
    }
    let extension = match image::guess_format(&bytes) {
        Ok(image::ImageFormat::Png) => "png",
        Ok(image::ImageFormat::Jpeg) => "jpg",
        Ok(image::ImageFormat::Gif) => "gif",
        Ok(image::ImageFormat::WebP) => "webp",
        _ => return Err(EngineImageError::UnsupportedFormat),
    };
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    let path = root.join(format!("{digest}.{extension}"));
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(root).map_err(|source| EngineImageError::CreateDirectory {
        path: root.to_path_buf(),
        source,
    })?;
    // Staged beside the target and renamed, so a reader never sees a
    // partial file under the final name.
    // Unique per write, so two writers of the same image never share a
    // staging file; whichever rename lands last leaves identical bytes.
    static STAGED: AtomicU64 = AtomicU64::new(0);
    let staged = root.join(format!(
        ".{digest}.{}.{}.tmp",
        std::process::id(),
        STAGED.fetch_add(1, Ordering::Relaxed)
    ));
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&staged)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&staged, &path)
    };
    if let Err(source) = write() {
        let _ = std::fs::remove_file(&staged);
        return Err(EngineImageError::Write { path, source });
    }
    Ok(path)
}

/// Bounds one provider caption to the observation limit, cut on a character
/// boundary. Blank text is no caption.
pub(crate) fn bounded_caption(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut end = text
        .len()
        .min(artisan_domain::OBSERVATION_DESCRIPTION_MAX_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(text[..end].to_owned())
}

fn decode_payload(payload: &str) -> Result<Vec<u8>, EngineImageError> {
    let payload = payload.trim();
    let data = match payload.strip_prefix("data:") {
        Some(url) => url.split_once(',').map_or("", |(_, data)| data),
        None => payload,
    };
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|source| EngineImageError::Decode { source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_image_lands_content_addressed_and_deduplicates() {
        let root = tempfile::tempdir().expect("temp dir");
        let first = store_inline_image(root.path(), PIXEL_PNG_BASE64).expect("stored");
        assert_eq!(first.parent(), Some(root.path()));
        assert_eq!(
            first.extension().and_then(|value| value.to_str()),
            Some("png")
        );
        assert_eq!(
            first
                .file_stem()
                .and_then(|value| value.to_str())
                .map(str::len),
            Some(64)
        );
        let data_url = format!("data:image/png;base64,{PIXEL_PNG_BASE64}");
        let second = store_inline_image(root.path(), &data_url).expect("stored again");
        assert_eq!(first, second);
        assert_eq!(std::fs::read_dir(root.path()).expect("listing").count(), 1);
    }

    #[test]
    fn inline_image_rejects_garbage_and_non_images() {
        let root = tempfile::tempdir().expect("temp dir");
        assert!(matches!(
            store_inline_image(root.path(), "not base64!"),
            Err(EngineImageError::Decode { .. })
        ));
        assert!(matches!(
            store_inline_image(root.path(), ""),
            Err(EngineImageError::Empty)
        ));
        let text = base64::engine::general_purpose::STANDARD.encode(b"plain text");
        assert!(matches!(
            store_inline_image(root.path(), &text),
            Err(EngineImageError::UnsupportedFormat)
        ));
        assert_eq!(std::fs::read_dir(root.path()).expect("listing").count(), 0);
    }
}
