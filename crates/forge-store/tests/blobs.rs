
use artisan_api::BlobHash;
use forge_store::Store;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn images_saved_before_extensions_are_found_by_agents_as_webp_files() {
    let dir = tempfile::tempdir().unwrap();
    let image = webp::Encoder::from_rgb(&[7, 7, 7], 1, 1)
        .encode_lossless()
        .to_vec();
    let hash = BlobHash(hex::encode(Sha256::digest(&image)));
    let shard = dir.path().join("blobs").join(&hash.as_str()[..2]);
    std::fs::create_dir_all(&shard).unwrap();
    let old = shard.join(hash.as_str());
    std::fs::write(&old, &image).unwrap();

    let store = Store::open(dir.path()).unwrap();
    let path = store.blob_path(&hash).unwrap();
    assert_eq!(path, old.with_extension("webp"));
    assert_eq!(std::fs::read(&path).unwrap(), image);
    assert!(!old.exists());

    let again = store.put_blob(image).await.unwrap();
    assert_eq!(again.hash, hash);
    assert_eq!(std::fs::read_dir(&shard).unwrap().count(), 1);
}
