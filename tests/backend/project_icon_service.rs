use super::*;
use serde_json::json;

#[test]
fn repository_artwork_precedes_owner_and_empty_artwork_is_skipped() {
    let metadata = json!({"avatar_url":"https://gitlab.com/project.png", "namespace":{"avatar_url":"/owner.png"}});
    assert_eq!(
        candidates(&metadata, "gitlab.com"),
        vec!["https://gitlab.com/project.png", "/owner.png"]
    );
    let metadata = json!({"avatar_url":null, "namespace":{"avatar_url":"/owner.png"}});
    assert_eq!(candidates(&metadata, "gitlab.com"), vec!["/owner.png"]);
    assert_eq!(candidates(&json!({}), "gitlab.com"), Vec::<&str>::new());
}

#[test]
fn disk_and_unrecognized_host_fallbacks_are_distinct() {
    assert_eq!(fallback(None), RecentProjectIcon::default());
    assert_eq!(
        fallback(Some("https://github.com/owner/repo"))
            .host
            .unwrap()
            .as_str(),
        "github"
    );
    assert_eq!(
        fallback(Some("https://example.com/owner/repo"))
            .host
            .unwrap()
            .as_str(),
        "other"
    );
}

#[test]
fn icon_decoding_bounds_input_and_normalizes_to_small_png() {
    let source = image::DynamicImage::new_rgba8(256, 128);
    let mut bytes = Cursor::new(Vec::new());
    source
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let png = normalize(bytes.get_ref()).unwrap();
    assert!(png.len() <= RECENT_PROJECT_ICON_MAX_BYTES);
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (48, 24));
    assert!(normalize(b"not an image").is_none());
    assert!(normalize(&vec![0; INPUT_LIMIT + 1]).is_none());
}

#[test]
fn gitlab_project_path_is_one_encoded_api_segment() {
    assert_eq!(
        metadata_url(
            &Url::parse("https://gitlab.com/group/subgroup/repo").unwrap(),
            "group/subgroup/repo"
        )
        .unwrap()
        .as_str(),
        "https://gitlab.com/api/v4/projects/group%2Fsubgroup%2Frepo"
    );
}

#[test]
fn explicit_project_icon_wins_and_invalid_files_fall_through() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "artisan-project-icon-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(directory.join(".artisan")).unwrap();
    let write_png = |path: &Path, color| {
        let image = image::RgbaImage::from_pixel(16, 16, image::Rgba(color));
        image.save(path).unwrap();
    };
    write_png(&directory.join(".artisan/icon.png"), [255, 0, 0, 255]);
    write_png(&directory.join("favicon.png"), [0, 255, 0, 255]);
    let png = local_icon(&directory).unwrap();
    assert_eq!(
        image::load_from_memory(&png)
            .unwrap()
            .to_rgba8()
            .get_pixel(0, 0)
            .0,
        [255, 0, 0, 255]
    );
    std::fs::write(directory.join(".artisan/icon.png"), b"invalid").unwrap();
    let png = local_icon(&directory).unwrap();
    assert_eq!(
        image::load_from_memory(&png)
            .unwrap()
            .to_rgba8()
            .get_pixel(0, 0)
            .0,
        [0, 255, 0, 255]
    );
    std::fs::remove_dir_all(directory).unwrap();
}
