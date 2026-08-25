//! External coverage for the Phase 7 GPUI asset-presentation seam in
//! `artisan_ui`.
//!
//! These tests exercise only the public `artisan_ui::asset_seam` API and pin
//! what this packet requires without a GPUI runtime: catalog presentation
//! policy routing (including the authored single-hue brand marks preserved
//! despite monochrome artwork) over representative ids and the whole
//! catalog, adapter round trips with borrowed byte identity and determinism,
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
fn authored_single_hue_brand_marks_keep_full_color_despite_monochrome_artwork() {
    // Legacy evidence (docs/ui/ASSETS.md §10, engine/presentation.ts): engine
    // `claude` and provider `anthropic` flag `SvglClaudeAILogo` non-inverting,
    // and provider `deepseek` flags `SvglDeepSeekLogo` non-inverting. Both
    // artworks are single-paint (#D97757 / #4D6BFE), so the structural
    // `monochrome` property stays true; presentation policy is independent of
    // it and preserves the authored colors through the FullColor route.
    for id in [AssetId::SVGL_CLAUDE_AI, AssetId::SVGL_DEEPSEEK] {
        let asset = get(id);
        assert!(
            asset.monochrome,
            "{}: artwork-derived monochrome must stay true",
            id.as_str()
        );
        assert_eq!(
            asset.presentation,
            Presentation::FullColor,
            "{}: catalog policy must preserve the authored brand color",
            id.as_str()
        );
        assert_eq!(
            asset_glyph(id).presentation(),
            Presentation::FullColor,
            "{}: seam must route the authored-color mark full-color",
            id.as_str()
        );
    }

    // Control: an ordinary Tabler glyph shares `monochrome == true` with the
    // exceptions yet stays tinted — proving the two properties diverge.
    assert!(get(AssetId::TABLER_CHECK).monochrome);
    assert_eq!(
        asset_glyph(AssetId::TABLER_CHECK).presentation(),
        Presentation::Tinted
    );
}

#[test]
fn derives_the_route_for_every_catalog_id_from_presentation_metadata() {
    // The seam observes `Asset::presentation`; it never re-derives policy
    // from artwork structure. Every one of the 104 ids routes exactly as its
    // catalog record says.
    assert_eq!(AssetId::CONSTANTS.len(), 104);
    for id in AssetId::CONSTANTS {
        assert_eq!(
            asset_glyph(*id).presentation(),
            get(*id).presentation,
            "route for `{}` must come from Asset::presentation",
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
