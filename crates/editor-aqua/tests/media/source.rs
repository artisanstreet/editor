use editor_aqua::ui::media::source::{Parsed, parse};

#[test]
fn authored_sources_keep_host_relative_paths_and_decode_data_urls() {
    assert_eq!(
        parse(" ~/folder/a.png "),
        Parsed::File {
            path: "folder/a.png".into(),
            from_home: true
        }
    );
    assert_eq!(
        parse("file:///tmp/a.png"),
        Parsed::File {
            path: "file:///tmp/a.png".into(),
            from_home: false
        }
    );
    assert_eq!(
        parse("https://example.com/a.png"),
        Parsed::Url("https://example.com/a.png".into())
    );
    assert_eq!(parse("https://"), Parsed::Invalid);
    let Parsed::Data(base64) = parse("data:application/octet-stream;base64,iVBOR/8=") else {
        panic!("expected data URL");
    };
    let Parsed::Data(percent) = parse("data:application/octet-stream,%89%50%4e%47%ff") else {
        panic!("expected data URL");
    };
    assert_eq!(base64, percent);
    assert_eq!(base64.bytes.as_ref(), &[0x89, 0x50, 0x4e, 0x47, 0xff]);
    assert_eq!(parse("data:text/plain;base64,???"), Parsed::Invalid);
}

#[tokio::test]
async fn media_paths_fetch_through_captured_thread_scope() {
    use editor_aqua::ui::media::source::{
        MediaFetcher, MediaRequest, MediaSource, load_image_bytes,
    };
    use std::sync::{Arc, Mutex};
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let fetcher = MediaFetcher::new("thread-a", move |request| {
        captured.lock().unwrap().push(request);
        Box::pin(async { Ok(Arc::<[u8]>::from([1, 2, 3])) })
    })
    .showing("item-7");
    let source = MediaSource::from("~/asset.png".to_owned());
    let bytes = load_image_bytes(
        &source,
        Some(&fetcher),
        &editor_aqua::core::media_http::client(),
    )
    .await
    .unwrap();
    assert_eq!(bytes.bytes.as_ref(), &[1, 2, 3]);
    let through_provider = fetcher.provider().fetch(MediaRequest {
        scope: "thread-a".into(),
        path: "model.glb".into(),
        from_home: false,
        item: None,
    });
    through_provider.await.unwrap();
    assert_eq!(
        *requests.lock().unwrap(),
        vec![
            MediaRequest {
                scope: "thread-a".into(),
                path: "asset.png".into(),
                from_home: true,
                item: Some("item-7".into()),
            },
            MediaRequest {
                scope: "thread-a".into(),
                path: "model.glb".into(),
                from_home: false,
                item: Some("item-7".into()),
            },
        ]
    );
}

#[test]
fn fetcher_clones_share_thread_cache_and_preserve_semantic_identity() {
    use editor_aqua::ui::media::source::{MediaFailure, MediaFetcher, MediaSource, Owned, Parsed};
    use std::sync::Arc;

    let fetcher = MediaFetcher::new("forge/thread", |_| {
        Box::pin(async { Err(MediaFailure::Missing) })
    });
    let first = fetcher.clone().owned_by(&[42], 1);
    let repeated = fetcher.clone().owned_by(&[42], 1);
    let revised = fetcher.clone().owned_by(&[42], 2);
    let other = fetcher.clone().owned_by(&[43], 1);
    let new_thread = MediaFetcher::new("forge/thread", |_| {
        Box::pin(async { Err(MediaFailure::Missing) })
    })
    .owned_by(&[42], 1);
    assert!(Arc::ptr_eq(first.resources(), other.resources()));
    assert!(!Arc::ptr_eq(first.resources(), new_thread.resources()));
    assert_eq!(
        first.asset_owner("left", true),
        other.asset_owner("right", true)
    );
    assert_eq!(
        first.asset_owner("left", false),
        repeated.asset_owner("right", false)
    );
    assert_ne!(
        first.asset_owner("left", false),
        revised.asset_owner("left", false)
    );
    assert_ne!(
        first.asset_owner("left", false),
        other.asset_owner("left", false)
    );
    assert_ne!(
        first.asset_owner("left", true),
        new_thread.asset_owner("left", true)
    );
    assert!(first.provider() == other.provider());
    assert!(first.provider() != new_thread.provider());

    let Parsed::Data(left) = MediaSource::from("data:text/plain;base64,YQ==".to_owned()).parsed
    else {
        panic!()
    };
    let Parsed::Data(right) = MediaSource::from("data:text/plain,a".to_owned()).parsed else {
        panic!()
    };
    assert_eq!(
        Owned::new(first.asset_owner("left", true), left),
        Owned::new(other.asset_owner("right", true), right)
    );
}
