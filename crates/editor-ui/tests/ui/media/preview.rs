use super::*;
use editor_core::{EditorFiles, ForgeClient, HostConfig, HostKind};

fn fixture() -> (
    tempfile::TempDir,
    Core,
    HostConfig,
    aqua::images::ImageAssets,
    Preview,
) {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("preview".into()),
        name: "preview".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
    core.host_mut(&config.id).unwrap().test_stop_connection();
    let instance = core.host(&config.id).unwrap().instance();
    let assets = aqua::images::ImageAssets::default();
    let image = assets
        .insert(aqua_media::decode::DecodedImage::from_rgba(2, 1, vec![255; 8]).unwrap())
        .unwrap();
    let fetcher = MediaFetcher::new("preview", |_| {
        Box::pin(async { Err(super::super::source::MediaFailure::Missing) })
    })
    .owned_by(&[42], 1);
    let sources = vec![
        "first.png".to_owned().into(),
        "second.png".to_owned().into(),
    ];
    let preview = Preview::new(
        7,
        config.id.clone(),
        instance,
        fetcher,
        Images::new(),
        sources,
        0,
        image,
        Instant::now(),
    )
    .unwrap();
    (directory, core, config, assets, preview)
}

#[test]
fn navigation_and_swipes_obey_boundaries_and_dismissal_rules() {
    let (_directory, core, _config, _assets, mut preview) = fixture();
    let now = Instant::now();
    assert!(!preview.dispatch(6, Action::Navigate(true), &core, now, false));
    assert!(!preview.dispatch(7, Action::Navigate(false), &core, now, false));
    let viewport = Size { w: 1000.0, h: 800.0 };
    let point = |x: f32, y: f32| aqua_core::Point { x, y };
    let act = |preview: &mut Preview, action| preview.dispatch(7, action, &core, now, false);
    assert!(!act(
        &mut preview,
        Action::Press {
            at: point(100.0, 20.0),
            picture: true
        }
    ));
    assert!(act(
        &mut preview,
        Action::Drag {
            at: point(40.0, 22.0),
            viewport
        }
    ));
    assert!(act(
        &mut preview,
        Action::Release {
            at: point(40.0, 22.0)
        }
    ));
    assert_eq!(preview.gallery_index(), 1);
    assert!(!act(
        &mut preview,
        Action::Press {
            at: point(10.0, 10.0),
            picture: false
        }
    ));
    assert!(!act(
        &mut preview,
        Action::Release {
            at: point(60.0, 10.0)
        }
    ));
    assert!(preview.closing.is_none());
    assert!(!preview.dispatch(7, Action::Navigate(true), &core, now, false));
    assert!(preview.dispatch(7, Action::Close, &core, now, false));
    assert!(!preview.finished(now, false));
    assert!(preview.finished(now + Duration::from_millis(250), false));
}

#[test]
fn a_click_on_the_backdrop_closes_and_a_click_on_the_picture_does_not() {
    let (_directory, core, _config, _assets, mut preview) = fixture();
    let now = Instant::now();
    let point = |x: f32, y: f32| aqua_core::Point { x, y };
    for picture in [true, false] {
        assert!(!preview.dispatch(
            7,
            Action::Press {
                at: point(10.0, 10.0),
                picture
            },
            &core,
            now,
            false
        ));
        assert_eq!(
            preview.dispatch(
                7,
                Action::Release {
                    at: point(12.0, 11.0)
                },
                &core,
                now,
                false
            ),
            !picture
        );
        assert_eq!(preview.closing.is_some(), !picture);
    }
}

#[test]
fn the_wheel_zooms_about_the_cursor_and_drags_pan_only_where_the_picture_overflows() {
    let (_directory, core, _config, _assets, mut preview) = fixture();
    let now = Instant::now();
    let viewport = Size { w: 4.0, h: 4.0 };
    let point = |x: f32, y: f32| aqua_core::Point { x, y };
    let act = |preview: &mut Preview, action| preview.dispatch(7, action, &core, now, false);
    let wheel = |delta: f32| Action::Zoom {
        at: point(2.0, 2.0),
        delta,
        viewport,
    };

    assert!(!act(&mut preview, wheel(16.0)));
    assert!(act(&mut preview, wheel(-16.0)));
    assert!((preview.zoom - 1.2).abs() < 1e-4);
    for _ in 0..40 {
        act(&mut preview, wheel(-16.0));
    }
    assert_eq!(preview.zoom, 8.0);
    assert_eq!(preview.pan, point(0.0, 0.0));

    act(
        &mut preview,
        Action::Press {
            at: point(2.0, 2.0),
            picture: true,
        },
    );
    assert!(act(
        &mut preview,
        Action::Drag {
            at: point(50.0, 50.0),
            viewport
        }
    ));
    assert_eq!(preview.pan, point(2.0, 0.0));
    assert!(!act(
        &mut preview,
        Action::Release {
            at: point(50.0, 50.0)
        }
    ));
    assert_eq!(preview.gallery_index(), 0);

    for _ in 0..40 {
        act(&mut preview, wheel(16.0));
    }
    assert_eq!((preview.zoom, preview.pan), (1.0, point(0.0, 0.0)));
}

#[test]
fn replacement_host_cannot_drive_a_preview_from_the_previous_instance() {
    let (_directory, mut core, config, _assets, mut preview) = fixture();
    let old = core.host(&config.id).unwrap().instance();
    core.remove_host(&config.id).unwrap();
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
    core.host_mut(&config.id).unwrap().test_stop_connection();
    assert_ne!(core.host(&config.id).unwrap().instance(), old);
    assert!(!preview.is_live(&core));
    assert!(!preview.dispatch(7, Action::Navigate(true), &core, Instant::now(), false));
    assert!(!preview.refresh(&core));
    assert_eq!(preview.gallery_index(), 0);
}

#[test]
fn open_request_retains_the_opener_and_rejects_invalid_gallery_index() {
    let (_directory, core, config, _assets, preview) = fixture();
    let instance = core.host(&config.id).unwrap().instance();
    let Content::Gallery {
        fetcher, sources, ..
    } = &preview.content
    else {
        panic!("gallery")
    };
    let request = OpenPreview {
        fetcher: fetcher.clone(),
        sources: sources.clone(),
        index: 1,
        image: preview.image.as_ref().unwrap().clone(),
        opener_key: "row-42-image-1".into(),
    };
    let opened = Preview::from_request(
        8,
        config.id.clone(),
        instance,
        Images::new(),
        request.clone(),
        Instant::now(),
    )
    .unwrap();
    assert_eq!(opened.opener_key(), Some("row-42-image-1"));
    assert_eq!(opened.gallery_index(), 1);
    let invalid = OpenPreview {
        index: 2,
        ..request
    };
    assert!(
        Preview::from_request(
            9,
            config.id,
            instance,
            Images::new(),
            invalid,
            Instant::now()
        )
        .is_none()
    );
}

#[test]
fn mounted_preview_emits_source_frame_with_supported_entrance_transform() {
    use aqua::{UiApp, prelude::DisplayItem};
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::Box as UiBox;

    let (_directory, _core, _config, _assets, preview) = fixture();
    let when = preview.started + Duration::from_millis(125);
    let theme = ArtisanTheme::resolved();
    let viewport = Size { w: 800.0, h: 600.0 };
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| {
        UiBox::new()
            .width(viewport.w)
            .height(viewport.h)
            .child(preview.view(theme, viewport, when, false, Rc::new(|_, _| {})))
            .build()
    })
    .unwrap();
    app.set_viewport(viewport.w, viewport.h, 1.0);
    app.build_frame().unwrap();
    let scene = app.tree_mut().emit_scene();
    assert!(scene.iter().any(|(_, item)| matches!(item, DisplayItem::GpuiBackground { background: aqua_core::GpuiBackground::Solid(color), .. } if *color == theme.colors.background.at(Opacity::BackdropDense).to_reference_paint())));
    let transforms: Vec<_> = scene
        .iter()
        .filter_map(|(_, item)| match item {
            DisplayItem::PushTransform(transform) => Some(transform),
            _ => None,
        })
        .collect();
    assert!(transforms.iter().any(|transform| transform.a > 0.96
        && transform.a < 1.0
        && transform.a == transform.d
        && transform.b == 0.0
        && transform.c == 0.0
        && transform.tx.is_finite()
        && transform.ty.is_finite()));
    assert_eq!(
        transforms.len(),
        scene
            .iter()
            .filter(|(_, item)| matches!(item, DisplayItem::PopTransform))
            .count()
    );
}

#[test]
fn ready_only_preview_retains_image_and_ignores_gallery_navigation() {
    let (_directory, core, config, assets, gallery) = fixture();
    let instance = core.host(&config.id).unwrap().instance();
    let image = gallery.image.as_ref().unwrap().clone();
    let weak = image.downgrade();
    let now = Instant::now();
    let mut preview = Preview::from_ready(
        11,
        config.id.clone(),
        instance,
        image,
        "attachment-1".into(),
        now,
    );
    drop(gallery);
    assert!(weak.upgrade().is_some());
    assert_eq!(preview.opener_key(), Some("attachment-1"));
    assert!(!preview.dispatch(11, Action::Navigate(true), &core, now, false));
    assert!(!preview.dispatch(
        11,
        Action::Press {
            at: aqua_core::Point { x: 60.0, y: 2.0 },
            picture: true
        },
        &core,
        now,
        false
    ));
    assert!(!preview.refresh(&core));
    assert!(matches!(preview.content, Content::ReadyOnly));
    assert!(preview.dispatch(11, Action::Close, &core, now, true));
    assert!(preview.finished(now, true));
    drop(preview);
    assert!(weak.upgrade().is_none());
    assert_eq!(assets.stats().images, 0);
}

#[test]
fn ready_only_preview_rejects_replaced_host_and_stale_actions() {
    let (_directory, mut core, config, _assets, gallery) = fixture();
    let instance = core.host(&config.id).unwrap().instance();
    let mut preview = Preview::from_ready(
        11,
        config.id.clone(),
        instance,
        gallery.image.as_ref().unwrap().clone(),
        "attachment-1".into(),
        Instant::now(),
    );
    assert!(!preview.dispatch(10, Action::Close, &core, Instant::now(), false));
    core.remove_host(&config.id).unwrap();
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
    core.host_mut(&config.id).unwrap().test_stop_connection();
    assert!(!preview.is_live(&core));
    assert!(!preview.dispatch(11, Action::Close, &core, Instant::now(), false));
}
