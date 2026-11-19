use crate::error::{StorageError, operation_error};
use crate::writer::InputFailure;
use crate::{Inner, SendRejected, Store};
use artisan_api::{BlobHash, BlobRef};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const MAX_BLOB_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_MESSAGE_IMAGES: usize = 128;
pub const MAX_MESSAGE_IMAGE_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_IMAGE_PIXELS: u64 = 16_777_216;

const GC_MIN_AGE: Duration = Duration::from_hours(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobRejected {
    NotWebp,
    TooLarge { bytes: usize },
}

impl fmt::Display for BlobRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotWebp => f.write_str("blob is not a WebP image"),
            Self::TooLarge { bytes } => {
                write!(f, "blob is {bytes} bytes; the limit is {MAX_BLOB_BYTES}")
            }
        }
    }
}

impl std::error::Error for BlobRejected {}

type Result<T, E = BlobFailure> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum BlobFileError {
    #[error("list {}", path.display())]
    List {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("name blob {}", path.display())]
    Rename {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum BlobFailure {
    #[error(transparent)]
    Rejected(#[from] BlobRejected),
    #[error("the image is {width}x{height}; the limit is 16 megapixels")]
    TooManyPixels { width: u32, height: u32 },
    #[error("read the image")]
    Image(#[source] image::ImageError),
    #[error("start the WebP encoder")]
    WebPEncoder,
    #[error("encode a {width}x{height} WebP: {error:?}")]
    WebPEncode {
        width: u32,
        height: u32,
        error: webp::WebPEncodingError,
    },
    #[error("{step} {}", path.display())]
    Io {
        step: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for BlobFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

fn io(step: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> BlobFailure {
    let path = path.to_path_buf();
    move |source| BlobFailure::Io { step, path, source }
}

operation_error!(PutBlobError(BlobFailure), "store a blob of {bytes} bytes", bytes: usize);
operation_error!(PutImageError(BlobFailure), "store an image of {bytes} bytes", bytes: usize);
operation_error!(InputImagesError(InputFailure), "find the files of {count} input images", count: usize);
operation_error!(CollectBlobsError(BlobFailure), "collect unreferenced blobs");

const WEBP_QUALITY: f32 = 85.0;

const EXTENSION: &str = "webp";

pub(crate) fn path(dir: &Path, hash: &BlobHash) -> Option<PathBuf> {
    hash.is_valid().then(|| {
        dir.join(hash.as_str().split_at(2).0)
            .join(hash.as_str())
            .with_extension(EXTENSION)
    })
}

pub(crate) fn name_files(dir: &Path) -> Result<usize, BlobFileError> {
    let list = |path: &Path| {
        let path = path.to_path_buf();
        move |source| BlobFileError::List { path, source }
    };
    let mut renamed = 0;
    for shard in fs::read_dir(dir).map_err(list(dir))? {
        let shard = shard.map_err(list(dir))?.path();
        if !shard.is_dir() {
            continue;
        }
        for file in fs::read_dir(&shard).map_err(list(&shard))? {
            let file = file.map_err(list(&shard))?.path();
            let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !BlobHash(name.to_owned()).is_valid() {
                continue;
            }
            fs::rename(&file, file.with_extension(EXTENSION)).map_err(|source| {
                BlobFileError::Rename {
                    path: file.clone(),
                    source,
                }
            })?;
            renamed += 1;
        }
    }
    Ok(renamed)
}

pub(crate) fn input_images(
    dir: &Path,
    hashes: &[BlobHash],
) -> Result<(Vec<PathBuf>, u64), InputFailure> {
    if hashes.len() > MAX_MESSAGE_IMAGES {
        return Err(SendRejected::TooManyImages {
            count: hashes.len(),
        }
        .into());
    }
    let mut paths = Vec::with_capacity(hashes.len());
    let mut bytes = 0u64;
    for hash in hashes {
        let file = path(dir, hash).ok_or_else(|| SendRejected::MissingBlob(hash.clone()))?;
        let metadata = fs::metadata(&file).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                InputFailure::Rejected(SendRejected::MissingBlob(hash.clone()))
            } else {
                InputFailure::ImageMetadata {
                    path: file.clone(),
                    source: error,
                }
            }
        })?;
        if !metadata.is_file() {
            return Err(SendRejected::MissingBlob(hash.clone()).into());
        }
        if metadata.len() > MAX_BLOB_BYTES as u64 {
            return Err(SendRejected::ImageTooLarge {
                blob: hash.clone(),
                bytes: metadata.len(),
            }
            .into());
        }
        bytes = bytes.saturating_add(metadata.len());
        if bytes > MAX_MESSAGE_IMAGE_BYTES {
            return Err(SendRejected::ImagesTooLarge { bytes }.into());
        }
        paths.push(file);
    }
    Ok((paths, bytes))
}

pub(crate) fn put(dir: &Path, bytes: &[u8]) -> Result<BlobRef> {
    use image::ImageDecoder as _;
    if bytes.len() > MAX_BLOB_BYTES {
        return Err(BlobRejected::TooLarge { bytes: bytes.len() }.into());
    }
    if !is_webp(bytes) {
        return Err(BlobRejected::NotWebp.into());
    }
    let mut decoder = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|_decode_error| BlobRejected::NotWebp)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    decoder
        .set_limits(limits)
        .map_err(|_decode_error| BlobRejected::NotWebp)?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return Err(BlobRejected::NotWebp.into());
    }
    let hash = BlobHash(hex::encode(Sha256::digest(bytes)));
    let blob_ref = BlobRef {
        hash,
        bytes: bytes.len() as u64,
    };
    let target = path(dir, &blob_ref.hash).expect("invariant: a SHA-256 hex digest is a blob hash");
    if target.exists() {
        fs::File::options()
            .write(true)
            .open(&target)
            .and_then(|file| file.set_modified(SystemTime::now()))
            .map_err(io("refresh blob", &target))?;
        return Ok(blob_ref);
    }
    let shard = target
        .parent()
        .expect("invariant: a blob path sits in its shard folder");
    fs::create_dir_all(shard).map_err(io("create blob shard", shard))?;

    let mut temporary =
        tempfile::NamedTempFile::new_in(shard).map_err(io("create a blob file in", shard))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(io("write blob", &target))?;
    match temporary.persist_noclobber(&target) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io("write blob", &target)(error.error)),
    }
    Ok(blob_ref)
}

#[derive(Debug)]
pub struct BlobUpload {
    store: Store,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

impl BlobUpload {
    pub async fn put(self, bytes: Vec<u8>) -> Result<BlobRef, PutBlobError> {
        let length = bytes.len();
        self.store
            .persist_blob(bytes)
            .await
            .map_err(|cause| PutBlobError {
                bytes: length,
                cause: Box::new(cause),
            })
    }
}

impl Store {
    pub async fn admit_blob(&self) -> BlobUpload {
        let slot = std::sync::Arc::clone(&self.inner.blob_uploads)
            .acquire_owned()
            .await
            .expect("invariant: the blob upload slots are never closed");
        BlobUpload {
            store: self.clone(),
            _slot: slot,
        }
    }

    pub async fn put_blob(&self, bytes: Vec<u8>) -> Result<BlobRef, PutBlobError> {
        self.admit_blob().await.put(bytes).await
    }

    pub async fn put_image(&self, bytes: Vec<u8>) -> Result<BlobRef, PutImageError> {
        let length = bytes.len();
        let failed = |cause| PutImageError {
            bytes: length,
            cause: Box::new(cause),
        };
        let upload = self.admit_blob().await;
        let webp = tokio::task::spawn_blocking(move || to_webp(bytes))
            .await
            .map_err(|task| failed(BlobFailure::Storage(StorageError::Task(task))))?
            .map_err(failed)?;
        upload.store.persist_blob(webp).await.map_err(failed)
    }

    async fn persist_blob(&self, bytes: Vec<u8>) -> Result<BlobRef> {
        let persisting = std::sync::Arc::clone(&self.inner.blob_lock)
            .lock_owned()
            .await;
        let inner = std::sync::Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let _persisting = persisting;
            let _generation = inner.lock_blobs()?;
            put(&inner.blob_dir, &bytes)
        })
        .await
        .map_err(|task| BlobFailure::Storage(StorageError::Task(task)))?
    }

    pub async fn input_image_paths(
        &self,
        images: Vec<BlobHash>,
    ) -> Result<Vec<PathBuf>, InputImagesError> {
        let count = images.len();
        let dir = self.inner.blob_dir.clone();
        let found = tokio::task::spawn_blocking(move || {
            input_images(&dir, &images).map(|(paths, _)| paths)
        })
        .await;
        found
            .map_err(|task| InputFailure::from(StorageError::Task(task)))
            .and_then(|found| found)
            .map_err(|cause| InputImagesError {
                count,
                cause: Box::new(cause),
            })
    }

    #[must_use]
    pub fn blob_dir(&self) -> &Path {
        &self.inner.blob_dir
    }

    #[must_use]
    pub fn blob_path(&self, hash: &BlobHash) -> Option<PathBuf> {
        path(&self.inner.blob_dir, hash).filter(|path| path.is_file())
    }

    pub async fn gc_blobs(&self) -> Result<usize, CollectBlobsError> {
        self.collect_blobs()
            .await
            .map_err(|cause| CollectBlobsError {
                cause: Box::new(cause),
            })
    }

    async fn collect_blobs(&self) -> Result<usize> {
        let inner = std::sync::Arc::clone(&self.inner);
        let (seen, candidates) = tokio::task::spawn_blocking(move || {
            let (seen, referenced) = inner.with_reader(|conn| {
                let snapshot = conn.unchecked_transaction()?;
                let seen = crate::read::max_seq(&snapshot)?;
                Ok::<_, BlobFailure>((seen, referenced_since(&snapshot, 0)?))
            })?;
            Ok::<_, BlobFailure>((seen, unreferenced(&inner.blob_dir, &referenced)?))
        })
        .await
        .map_err(StorageError::Task)??;
        if candidates.is_empty() {
            return Ok(0);
        }
        let inner = std::sync::Arc::clone(&self.inner);
        let persisting = std::sync::Arc::clone(&self.inner.blob_lock)
            .lock_owned()
            .await;
        self.write(move |tx| {
            let _persisting = persisting;
            let referenced = referenced_since(tx, seen)?;
            let _generation = inner.lock_blobs()?;
            Ok(remove_unreferenced(candidates, &referenced))
        })
        .await
    }
}

impl Inner {
    fn lock_blobs(&self) -> Result<fs::File> {
        use fs2::FileExt as _;
        let lock = self.blob_dir.with_extension("lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock)
            .map_err(io("open the blob lock", &lock))?;
        file.lock_exclusive()
            .map_err(io("lock the blobs with", &lock))?;
        Ok(file)
    }
}

fn referenced_since(
    conn: &rusqlite::Connection,
    seq: artisan_api::Seq,
) -> Result<HashSet<String>, StorageError> {
    let mut hashes = HashSet::new();
    let mut statement = conn.prepare_cached(include_str!("sql/blobs/referenced.sql"))?;
    for hash in statement.query_map([seq], |row| row.get::<_, Option<String>>(0))? {
        if let Some(hash) = hash?
            && BlobHash(hash.clone()).is_valid()
        {
            hashes.insert(hash);
        }
    }
    Ok(hashes)
}

struct Candidate {
    hash: String,
    path: PathBuf,
}

fn unreferenced(dir: &Path, referenced: &HashSet<String>) -> Result<Vec<Candidate>> {
    let cutoff = SystemTime::now() - GC_MIN_AGE;
    let mut candidates = Vec::new();
    for shard in fs::read_dir(dir).map_err(io("list", dir))? {
        let shard = shard.map_err(io("list", dir))?.path();
        if !shard.is_dir() {
            continue;
        }
        for file in fs::read_dir(&shard).map_err(io("list", &shard))? {
            let file = file.map_err(io("list", &shard))?;
            let name = file.file_name();
            let name = name.to_string_lossy();
            let hash = name.strip_suffix(".webp").unwrap_or(&name);
            let modified = file
                .metadata()
                .and_then(|metadata| metadata.modified())
                .map_err(io("read the age of", &file.path()))?;
            if !referenced.contains(hash) && modified <= cutoff {
                candidates.push(Candidate {
                    hash: hash.to_owned(),
                    path: file.path(),
                });
            }
        }
    }
    Ok(candidates)
}

fn remove_unreferenced(candidates: Vec<Candidate>, referenced: &HashSet<String>) -> usize {
    let cutoff = SystemTime::now() - GC_MIN_AGE;
    let mut removed = 0;
    for candidate in candidates {
        if referenced.contains(&candidate.hash) {
            continue;
        }
        match fs::metadata(&candidate.path).and_then(|metadata| metadata.modified()) {
            Ok(modified) if modified <= cutoff => {}
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                tracing::warn!(path = %candidate.path.display(), %error, "read the age of an unreferenced blob");
                continue;
            }
        }
        match fs::remove_file(&candidate.path) {
            Ok(()) => removed += 1,
            Err(error) => {
                tracing::warn!(path = %candidate.path.display(), %error, "remove unreferenced blob");
            }
        }
    }
    removed
}

pub(crate) fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

pub(crate) fn to_webp(bytes: Vec<u8>) -> Result<Vec<u8>> {
    use image::ImageDecoder as _;
    if bytes.len() > MAX_BLOB_BYTES {
        return Err(BlobRejected::TooLarge { bytes: bytes.len() }.into());
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|source| BlobFailure::Image(image::ImageError::IoError(source)))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(BlobFailure::Image)?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return Err(BlobFailure::TooManyPixels { width, height });
    }
    if is_webp(&bytes) {
        drop(decoder);
        return Ok(bytes);
    }
    let image = image::DynamicImage::from_decoder(decoder).map_err(BlobFailure::Image)?;
    let (width, height) = (image.width(), image.height());
    let mut config = webp::WebPConfig::new().map_err(|()| BlobFailure::WebPEncoder)?;
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
    encoded.map_err(|error| BlobFailure::WebPEncode {
        width,
        height,
        error,
    })
}

#[cfg(test)]
#[path = "../tests/blobs/unit.rs"]
mod tests;
