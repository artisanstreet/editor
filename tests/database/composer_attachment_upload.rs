//! Chunked composer attachment uploads and windowed reads against migrated
//! SQLite.

use artisan_database::{
    COMPOSER_ATTACHMENT_UNREFERENCED_GRACE_MS, ComposerDraftRepositoryError, Repository,
    SqliteConfig, connect,
};
use artisan_domain::{
    COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES, COMPOSER_ATTACHMENT_MAX_BYTES, ComposerAttachmentChunk,
    ComposerAttachmentDigest, ComposerImage, ImageMimeType, ReadComposerAttachment, UnixMillis,
};
use artisan_migrations::migrate_to_current;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use sha2::{Digest, Sha256};

async fn database() -> DatabaseConnection {
    let database = connect(
        SqliteConfig::in_memory()
            .min_connections(1)
            .max_connections(1)
            .sqlx_logging(false),
    )
    .await
    .expect("memory database should open");
    migrate_to_current(&database)
        .await
        .expect("memory database should migrate");
    database
}

/// A picked image of `len` bytes whose content varies, so a misplaced
/// chunk changes the digest.
fn picked(len: usize) -> ComposerImage {
    let bytes = (0..len).map(|index| (index % 251) as u8).collect();
    ComposerImage::new("image/png", bytes, "large.png").expect("image")
}

fn digest_of(image: &ComposerImage) -> ComposerAttachmentDigest {
    ComposerAttachmentDigest::new(Sha256::digest(image.bytes()).into())
}

async fn waiting_chunks(database: &DatabaseConnection) -> i64 {
    database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT count(*) FROM composer_attachment_upload_chunks",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index(0)
        .unwrap()
}

#[tokio::test]
async fn a_32_mib_image_uploads_in_chunks_in_any_order_and_reads_back_in_windows() {
    let database = database().await;
    let repository = Repository::new(database.clone());
    let image = picked(COMPOSER_ATTACHMENT_MAX_BYTES);
    let digest = digest_of(&image);
    let mut chunks = ComposerAttachmentChunk::split(&image, digest);
    assert_eq!(chunks.len(), 8);
    // The last chunk arrives first; nothing is stored until every byte is.
    chunks.rotate_right(1);
    let total = u32::try_from(COMPOSER_ATTACHMENT_MAX_BYTES).unwrap();
    let chunk_len = u32::try_from(COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES).unwrap();
    for (index, chunk) in chunks.iter().enumerate() {
        let outcome = repository
            .store_composer_attachment_chunk(chunk, UnixMillis::from_millis(10))
            .await
            .expect("chunk stored");
        assert_eq!(outcome.reference.size_bytes(), total);
        assert_eq!(outcome.reference.name(), "large.png");
        let received = chunk_len * u32::try_from(index + 1).unwrap();
        assert_eq!(outcome.pending_bytes, total - received);
    }
    assert_eq!(waiting_chunks(&database).await, 0);

    let mut read_back = Vec::new();
    while read_back.len() < COMPOSER_ATTACHMENT_MAX_BYTES {
        let window = repository
            .read_composer_attachment_window(&ReadComposerAttachment {
                digest,
                offset: u32::try_from(read_back.len()).unwrap(),
                max_bytes: chunk_len,
            })
            .await
            .unwrap()
            .expect("stored");
        assert_eq!(window.total_bytes, total);
        assert_eq!(window.mime_type, ImageMimeType::Png);
        assert!(window.bytes.len() <= COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES);
        read_back.extend(window.bytes);
    }
    assert_eq!(read_back, image.bytes());
    assert!(matches!(
        repository
            .read_composer_attachment_window(&ReadComposerAttachment {
                digest,
                offset: total,
                max_bytes: chunk_len,
            })
            .await,
        Err(ComposerDraftRepositoryError::AttachmentMismatch { .. })
    ));

    // Uploading the stored image again answers it as stored at once.
    let again = repository
        .store_composer_attachment_chunk(&chunks[3], UnixMillis::from_millis(11))
        .await
        .unwrap();
    assert_eq!(again.pending_bytes, 0);
    assert_eq!(waiting_chunks(&database).await, 0);
}

#[tokio::test]
async fn chunks_that_do_not_match_their_digest_discard_the_upload() {
    let database = database().await;
    let repository = Repository::new(database.clone());
    let image = picked(COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES + 10);
    let wrong = ComposerAttachmentDigest::new([9; 32]);
    let chunks = ComposerAttachmentChunk::split(&image, wrong);
    repository
        .store_composer_attachment_chunk(&chunks[0], UnixMillis::from_millis(1))
        .await
        .unwrap();
    assert!(matches!(
        repository
            .store_composer_attachment_chunk(&chunks[1], UnixMillis::from_millis(2))
            .await,
        Err(ComposerDraftRepositoryError::ChunkRejected { .. })
    ));
    assert_eq!(waiting_chunks(&database).await, 0);
    assert!(
        repository
            .read_composer_attachment(&wrong)
            .await
            .unwrap()
            .is_none()
    );

    // A chunk that disagrees with the upload it joins discards it too.
    let digest = digest_of(&image);
    let chunks = ComposerAttachmentChunk::split(&image, digest);
    repository
        .store_composer_attachment_chunk(&chunks[0], UnixMillis::from_millis(3))
        .await
        .unwrap();
    let contradicting = ComposerAttachmentChunk::new(
        digest,
        ImageMimeType::Webp,
        "large.png",
        chunks[1].total_bytes(),
        chunks[1].offset(),
        chunks[1].bytes().to_vec(),
    )
    .unwrap();
    assert!(matches!(
        repository
            .store_composer_attachment_chunk(&contradicting, UnixMillis::from_millis(4))
            .await,
        Err(ComposerDraftRepositoryError::ChunkRejected { .. })
    ));
    assert_eq!(waiting_chunks(&database).await, 0);
}

#[tokio::test]
async fn chunks_of_an_abandoned_upload_are_pruned_after_the_grace_period() {
    let database = database().await;
    let repository = Repository::new(database.clone());
    let abandoned = picked(COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES + 1);
    let chunks = ComposerAttachmentChunk::split(&abandoned, digest_of(&abandoned));
    repository
        .store_composer_attachment_chunk(&chunks[0], UnixMillis::from_millis(5))
        .await
        .unwrap();
    assert_eq!(waiting_chunks(&database).await, 1);

    let later = picked(COMPOSER_ATTACHMENT_CHUNK_MAX_BYTES + 2);
    let chunks = ComposerAttachmentChunk::split(&later, digest_of(&later));
    repository
        .store_composer_attachment_chunk(
            &chunks[0],
            UnixMillis::from_millis(6 + COMPOSER_ATTACHMENT_UNREFERENCED_GRACE_MS),
        )
        .await
        .unwrap();
    assert_eq!(
        waiting_chunks(&database).await,
        1,
        "only the live upload waits"
    );
}
