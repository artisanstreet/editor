//! Content-addressed WebP files under `blobs/ab/<sha256>`.

use anyhow::{Context, Result, anyhow};
use artisan_api::{BlobHash, BlobRef};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const MAX_BLOB_BYTES: usize = 32 * 1024 * 1024;

/// Files younger than this survive collection: a client uploads a blob just
/// before the message that references it.
const GC_MIN_AGE: Duration = Duration::from_hours(1);

/// Why [`crate::Store::put_blob`] refused the bytes (the client's fault, not
/// Forge's). Find it with `error.downcast_ref::<BlobRejected>()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobRejected {
    NotWebp,
    TooLarge { bytes: usize },
}

impl fmt::Display for BlobRejected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotWebp => formatter.write_str("blob is not a WebP image"),
            Self::TooLarge { bytes } => write!(
                formatter,
                "blob is {bytes} bytes; the limit is {MAX_BLOB_BYTES}"
            ),
        }
    }
}

impl std::error::Error for BlobRejected {}

const WEBP_QUALITY: f32 = 85.0;

pub(crate) fn path(dir: &Path, hash: &BlobHash) -> Option<PathBuf> {
    hash.is_valid()
        .then(|| dir.join(&hash.as_str()[..2]).join(hash.as_str()))
}

/// Canonical encoded image admission, also used to validate historical pending inputs.
pub(crate) fn input_images(dir: &Path, hashes: &[BlobHash]) -> Result<(Vec<PathBuf>, u64)> {
    if hashes.len() > 128 {
        return Err(
            crate::SendRejected::Payload("message exceeds 128 image references".into()).into(),
        );
    }
    let mut paths = Vec::with_capacity(hashes.len());
    let mut bytes = 0u64;
    for hash in hashes {
        let file = path(dir, hash).ok_or_else(|| crate::SendRejected::MissingBlob(hash.clone()))?;
        let metadata = fs::metadata(&file).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow::Error::from(crate::SendRejected::MissingBlob(hash.clone()))
            } else {
                anyhow::Error::from(error)
                    .context(format!("read image metadata {}", file.display()))
            }
        })?;
        if !metadata.is_file() {
            return Err(crate::SendRejected::MissingBlob(hash.clone()).into());
        }
        if metadata.len() > MAX_BLOB_BYTES as u64 {
            return Err(crate::SendRejected::Payload("image blob exceeds 32 MiB".into()).into());
        }
        bytes = bytes.saturating_add(metadata.len());
        if bytes > 128 * 1024 * 1024 {
            return Err(
                crate::SendRejected::Payload("message images exceed 128 MiB".into()).into(),
            );
        }
        paths.push(file);
    }
    Ok((paths, bytes))
}

pub(crate) fn put(dir: &Path, bytes: &[u8]) -> Result<BlobRef> {
    if bytes.len() > MAX_BLOB_BYTES {
        return Err(BlobRejected::TooLarge { bytes: bytes.len() }.into());
    }
    if !is_webp(bytes) {
        return Err(BlobRejected::NotWebp.into());
    }
    use image::ImageDecoder as _;
    let mut decoder = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|_| BlobRejected::NotWebp)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    decoder
        .set_limits(limits)
        .map_err(|_| BlobRejected::NotWebp)?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > 16_777_216 {
        return Err(BlobRejected::NotWebp.into());
    }
    let hash = BlobHash(hex::encode(Sha256::digest(bytes)));
    let blob_ref = BlobRef {
        hash,
        bytes: bytes.len() as u64,
    };
    let target = path(dir, &blob_ref.hash).context("hash a blob")?;
    if target.exists() {
        // Reusing a previously unreferenced blob starts its upload grace period again.
        fs::File::open(&target)?.set_modified(SystemTime::now())?;
        return Ok(blob_ref);
    }
    let shard = target.parent().context("blob shard directory")?;
    fs::create_dir_all(shard).with_context(|| format!("create {}", shard.display()))?;

    let mut temporary = tempfile::NamedTempFile::new_in(shard)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(&target) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| format!("write blob {}", target.display()));
        }
    }
    Ok(blob_ref)
}

/// Deletes blob files (and stale temp files) that nothing references.
pub(crate) fn gc(dir: &Path, referenced: &HashSet<String>) -> Result<usize> {
    let cutoff = SystemTime::now() - GC_MIN_AGE;
    let mut removed = 0;
    for shard in fs::read_dir(dir).with_context(|| format!("list {}", dir.display()))? {
        let shard = shard?.path();
        if !shard.is_dir() {
            continue;
        }
        for file in fs::read_dir(&shard).with_context(|| format!("list {}", shard.display()))? {
            let file = file?;
            let name = file.file_name();
            let name = name.to_string_lossy();
            let keep = referenced.contains(name.as_ref()) || file.metadata()?.modified()? > cutoff;
            if keep {
                continue;
            }
            match fs::remove_file(file.path()) {
                Ok(()) => removed += 1,
                Err(error) => {
                    tracing::warn!(path = %file.path().display(), %error, "remove unreferenced blob");
                }
            }
        }
    }
    Ok(removed)
}

pub(crate) fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// WebP stays as it is (re-encoding only adds loss); anything else is
/// decoded and encoded lossy at quality 85 with sharp YUV.
pub(crate) fn to_webp(bytes: Vec<u8>) -> Result<Vec<u8>> {
    if bytes.len() > MAX_BLOB_BYTES {
        return Err(BlobRejected::TooLarge { bytes: bytes.len() }.into());
    }
    use image::ImageDecoder as _;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoder = reader.into_decoder().context("read image dimensions")?;
    let (width, height) = decoder.dimensions();
    anyhow::ensure!(
        u64::from(width) * u64::from(height) <= 16_777_216,
        "image exceeds 16 megapixels"
    );
    if is_webp(&bytes) {
        drop(decoder);
        return Ok(bytes);
    }
    let image = image::DynamicImage::from_decoder(decoder).context("decode image")?;
    let (width, height) = (image.width(), image.height());
    let mut config = webp::WebPConfig::new().map_err(|()| anyhow!("init the WebP encoder"))?;
    config.quality = WEBP_QUALITY;
    config.use_sharp_yuv = 1;
    let encoded = if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        webp::Encoder::from_rgba(&rgba, width, height)
            .encode_advanced(&config)
            .map(|memory| memory.to_vec())
    } else {
        let rgb = image.to_rgb8();
        webp::Encoder::from_rgb(&rgb, width, height)
            .encode_advanced(&config)
            .map(|memory| memory.to_vec())
    };
    encoded.map_err(|error| anyhow!("encode {width}x{height} WebP: {error:?}"))
}
