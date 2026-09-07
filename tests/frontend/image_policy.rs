#[path = "../../modules/frontend/src/image_policy.rs"]
mod image_policy;

use image_policy::{
    IMAGE_COMPRESSION_LADDER, ImageCompressionFormat, ImageDimensions, ImageMediaType,
    MAXIMUM_IMAGE_LONG_EDGE_PIXELS, best_image_format, image_rescale_target,
    image_rescale_target_with_long_edge,
};

fn dimensions(width: f64, height: f64) -> ImageDimensions {
    ImageDimensions { height, width }
}

#[test]
fn compression_ladder_is_ordered_worst_to_best() {
    assert_eq!(
        IMAGE_COMPRESSION_LADDER,
        [
            ImageCompressionFormat::Png,
            ImageCompressionFormat::Jpeg,
            ImageCompressionFormat::Webp,
            ImageCompressionFormat::Avif,
        ]
    );
    assert_eq!(
        IMAGE_COMPRESSION_LADDER.map(ImageCompressionFormat::as_mime_type),
        ["image/png", "image/jpeg", "image/webp", "image/avif"]
    );
}

#[test]
fn known_engines_choose_webp() {
    assert_eq!(best_image_format(Some("claude")), ImageMediaType::Webp);
    assert_eq!(best_image_format(Some("codex")), ImageMediaType::Webp);
}

#[test]
fn unknown_and_absent_engines_choose_png() {
    assert_eq!(best_image_format(None), ImageMediaType::Png);
    assert_eq!(best_image_format(Some("unknown")), ImageMediaType::Png);
    assert_eq!(best_image_format(Some("Claude")), ImageMediaType::Png);
}

#[test]
fn media_types_have_protocol_mime_names() {
    assert_eq!(ImageMediaType::Gif.as_mime_type(), "image/gif");
    assert_eq!(ImageMediaType::Jpeg.as_mime_type(), "image/jpeg");
    assert_eq!(ImageMediaType::Png.as_mime_type(), "image/png");
    assert_eq!(ImageMediaType::Webp.as_mime_type(), "image/webp");
    assert_eq!(ImageCompressionFormat::Avif.as_image_media_type(), None);
}

#[test]
fn landscape_portrait_and_square_images_fit_the_default_cap() {
    assert_eq!(
        image_rescale_target(dimensions(4000.0, 2000.0)),
        Some(dimensions(2576.0, 1288.0))
    );
    assert_eq!(
        image_rescale_target(dimensions(2000.0, 4000.0)),
        Some(dimensions(1288.0, 2576.0))
    );
    assert_eq!(
        image_rescale_target(dimensions(4000.0, 4000.0)),
        Some(dimensions(2576.0, 2576.0))
    );
}

#[test]
fn exact_cap_and_smaller_images_are_not_enlarged() {
    assert_eq!(
        image_rescale_target(dimensions(MAXIMUM_IMAGE_LONG_EDGE_PIXELS, 1000.0)),
        None
    );
    assert_eq!(
        image_rescale_target(dimensions(1000.0, MAXIMUM_IMAGE_LONG_EDGE_PIXELS)),
        None
    );
    assert_eq!(image_rescale_target(dimensions(640.0, 480.0)), None);
}

#[test]
fn custom_caps_scale_without_enlarging() {
    assert_eq!(
        image_rescale_target_with_long_edge(dimensions(4000.0, 2000.0), 1000.0),
        Some(dimensions(1000.0, 500.0))
    );
    assert_eq!(
        image_rescale_target_with_long_edge(dimensions(1000.0, 500.0), 1000.0),
        None
    );
    assert_eq!(
        image_rescale_target_with_long_edge(dimensions(1000.0, 500.0), 2000.0),
        None
    );
}

#[test]
fn rounding_matches_positive_javascript_math_round_boundaries() {
    assert_eq!(
        image_rescale_target_with_long_edge(dimensions(10.0, 5.0), 3.0),
        Some(dimensions(3.0, 2.0))
    );
    assert_eq!(
        image_rescale_target_with_long_edge(dimensions(10.0, 4.999), 3.0),
        Some(dimensions(3.0, 1.0))
    );
    assert_eq!(
        image_rescale_target_with_long_edge(dimensions(10.0, 5.001), 3.0),
        Some(dimensions(3.0, 2.0))
    );
}

#[test]
fn scaled_dimensions_have_a_one_pixel_minimum() {
    assert_eq!(
        image_rescale_target_with_long_edge(dimensions(10.0, 1.0), 3.0),
        Some(dimensions(3.0, 1.0))
    );
}

#[test]
fn invalid_component_dimensions_return_no_target() {
    let invalid_sources = [
        dimensions(0.0, 10.0),
        dimensions(10.0, 0.0),
        dimensions(-1.0, 10.0),
        dimensions(10.0, -1.0),
        dimensions(f64::NAN, 10.0),
        dimensions(10.0, f64::NAN),
        dimensions(f64::INFINITY, 10.0),
        dimensions(10.0, f64::NEG_INFINITY),
    ];

    for source in invalid_sources {
        assert_eq!(image_rescale_target(source), None);
        assert_eq!(image_rescale_target_with_long_edge(source, 1.0), None);
    }
}

#[test]
fn invalid_caps_return_no_target() {
    let source = dimensions(4000.0, 2000.0);

    for cap in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(image_rescale_target_with_long_edge(source, cap), None);
    }
}
