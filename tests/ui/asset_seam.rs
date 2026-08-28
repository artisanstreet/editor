//! External coverage for the Phase 7 GPUI asset-presentation seam in
//! `artisan_ui`.
//!
//! These tests exercise only the public `artisan_ui::asset_seam` API and pin
//! what this packet requires without a GPUI runtime: metadata-derived mono
//! versus multicolor routing over representative ids and the whole catalog,
//! adapter round trips with borrowed byte identity and determinism,
//! rejection of unknown, empty, and path-shaped inputs, empty `list`
//! behavior, and why the multicolor route must construct
//! `ImageSource::Resource(Resource::Embedded(..))` explicitly instead of
//! letting upstream string conversion classify catalog keys as URIs.

use std::borrow::Cow;

use artisan_assets::{AssetId, get};
use artisan_ui::asset_seam::{CatalogAssetSource, Presentation, asset_glyph};
use gpui::{AssetSource, ImageSource, Resource};

#[test]
fn routes_representative_ids_by_catalog_metadata() {
    // A Tabler glyph is monochrome (`stroke="currentColor"`): tinted route.
    assert_eq!(
        asset_glyph(AssetId::TABLER_CHECK).presentation(),
        Presentation::Tinted
    );
    // GitLab carries four brand fills that alpha-mask tinting would flatten:
    // full-color route.
    assert_eq!(
        asset_glyph(AssetId::SVGL_GITLAB).presentation(),
        Presentation::FullColor
    );
    // The gradient app icon is multicolor first-party artwork.
    assert_eq!(
        asset_glyph(AssetId::ARTISAN_APP_ICON).presentation(),
        Presentation::FullColor
    );
}

#[test]
fn derives_the_route_for_every_catalog_id_from_monochrome_metadata() {
    for id in AssetId::CONSTANTS {
        let expected = if get(*id).monochrome {
            Presentation::Tinted
        } else {
            Presentation::FullColor
        };
        assert_eq!(
            asset_glyph(*id).presentation(),
            expected,
            "route for `{}` must derive from Asset::monochrome",
            id.as_str()
        );
    }
}

#[test]
fn adapter_round_trips_every_catalog_key_to_identical_embedded_bytes() {
    let source = CatalogAssetSource;

    for id in AssetId::CONSTANTS {
        let first = source
            .load(id.as_str())
            .expect("catalog loads never error")
            .expect("every catalog key must resolve");
        assert!(
            matches!(&first, Cow::Borrowed(_)),
            "loads of `{}` must borrow the embedded catalog bytes",
            id.as_str()
        );
        assert_eq!(first.as_ref(), get(*id).source.as_bytes());

        let second = source
            .load(id.as_str())
            .expect("catalog loads never error")
            .expect("every catalog key must resolve");
        assert_eq!(
            first,
            second,
            "loads of `{}` are deterministic",
            id.as_str()
        );
    }
}

#[test]
fn adapter_rejects_unknown_empty_and_path_shaped_inputs() {
    let source = CatalogAssetSource;

    for rejected in [
        "",
        "nope",
        "tabler",
        "tabler.check.svg",
        "svg/tabler/check.svg",
        "../secrets",
        "/etc/passwd",
        "C:\\evil.png",
    ] {
        let resolved = source
            .load(rejected)
            .expect("unknown input still cannot error");
        assert!(
            resolved.is_none(),
            "`{rejected}` must not resolve through the sealed catalog"
        );
    }
}

#[test]
fn adapter_lists_nothing_for_any_path() {
    let source = CatalogAssetSource;

    for path in ["", "/", "assets", "tabler", "tabler.check"] {
        let entries = source.list(path).expect("catalog listing never errors");
        assert!(entries.is_empty(), "`list(\"{path}\")` must stay empty");
    }
}

#[test]
fn bare_string_keys_misclassify_as_uris_but_explicit_embedding_resolves() {
    // Upstream `From<&str>` parses catalog keys as URIs; pinned verbatim so a
    // future GPUI bump that changes this trap surfaces here first.
    let bare: ImageSource = AssetId::SVGL_GITLAB.as_str().into();
    assert!(
        matches!(bare, ImageSource::Resource(Resource::Uri(_))),
        "upstream string conversion must keep misclassifying catalog keys"
    );

    // The seam's contract construction: explicit Embedded classification.
    let explicit = ImageSource::Resource(Resource::Embedded(AssetId::SVGL_GITLAB.as_str().into()));
    assert!(
        matches!(
            &explicit,
            ImageSource::Resource(Resource::Embedded(key))
                if key.as_ref() == AssetId::SVGL_GITLAB.as_str()
        ),
        "the multicolor route must construct Resource::Embedded explicitly"
    );

    let bytes = CatalogAssetSource
        .load(AssetId::SVGL_GITLAB.as_str())
        .expect("embedded loads never error")
        .expect("the embedded key must resolve through the adapter");
    assert_eq!(bytes.as_ref(), get(AssetId::SVGL_GITLAB).source.as_bytes());
}
