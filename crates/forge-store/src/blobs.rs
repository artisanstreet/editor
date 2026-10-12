//! Content-addressed WebP files under `blobs/ab/<sha256>`.

use anyhow::{Context, Result, anyhow};
use artisan_api::{BlobHash, BlobRef};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt::{self, Write as _};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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

pub(crate) fn put(dir: &Path, bytes: &[u8]) -> Result<BlobRef> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    if bytes.len() > MAX_BLOB_BYTES {
        return Err(BlobRejected::TooLarge { bytes: bytes.len() }.into());
    }
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(BlobRejected::NotWebp.into());
    }
    let hash = BlobHash(Sha256::digest(bytes).iter().fold(
        String::with_capacity(64),
        |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        },
    ));
    let blob_ref = BlobRef {
        hash,
        bytes: bytes.len() as u64,
    };
    let target = path(dir, &blob_ref.hash).context("hash a blob")?;
    if target.exists() {
        return Ok(blob_ref);
    }
    let shard = target.parent().context("blob shard directory")?;
    fs::create_dir_all(shard).with_context(|| format!("create {}", shard.display()))?;

    let temp = shard.join(format!(
        "{}.{}-{}.tmp",
        blob_ref.hash,
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, &target)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("write blob {}", target.display()));
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

/// Every string in `value` that is a blob hash.
pub(crate) fn collect_hashes(value: &serde_json::Value, hashes: &mut HashSet<String>) {
    match value {
        serde_json::Value::String(text) => {
            if text.len() == 64 && BlobHash(text.clone()).is_valid() {
                hashes.insert(text.clone());
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_hashes(value, hashes);
            }
        }
        serde_json::Value::Object(map) => {
            for value in map.values() {
                collect_hashes(value, hashes);
            }
        }
        _ => {}
    }
}

pub(crate) fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// WebP stays as it is (re-encoding only adds loss); anything else is
/// decoded and encoded lossy at quality 85 with sharp YUV.
pub(crate) fn to_webp(bytes: Vec<u8>) -> Result<Vec<u8>> {
    if is_webp(&bytes) {
        return Ok(bytes);
    }
    let image = image::load_from_memory(&bytes).context("decode image")?;
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
