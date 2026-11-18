use editor_aqua::frame::avatar::{Avatar, AvatarError};
use std::time::{Duration, Instant};

#[test]
fn a_failed_avatar_keeps_its_cause_and_reloads_after_a_pause() {
    let assets = aqua::images::ImageAssets::default();
    let url = "data:image/png;base64,bm90IGFuIGltYWdl";
    let start = Instant::now();
    let mut avatar = Avatar::default();
    avatar.want(Some(url), start);
    test_support::wait::wait_until("the broken avatar fails", Duration::from_secs(10), || {
        avatar.drain(&assets, start);
        avatar.failure().is_some()
    });
    assert_eq!(avatar.failure(), Some(AvatarError::Decode));
    avatar.want(Some(url), start + Duration::from_secs(5));
    assert_eq!(
        avatar.failure(),
        Some(AvatarError::Decode),
        "a fresh failure is not refetched on every frame"
    );
    avatar.want(Some(url), start + Duration::from_secs(30));
    assert_eq!(avatar.failure(), None, "the same URL loads again");
    assert!(avatar.image().is_none());
}
