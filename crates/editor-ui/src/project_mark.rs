//! A project's mark: its icon once read, else its repository host's logo,
//! else a folder. Shared by the sidebar's thread rows and the composer's
//! project menu.

use crate::AssetId;
use crate::asset_seam::asset_glyph;
use crate::theme::{RadiusStep, RadiusTokens};
use gpui::{
    AnyElement, Hsla, IntoElement as _, ObjectFit, ParentElement as _, RenderImage, Styled as _,
    StyledImage as _, div, img, px,
};
use std::sync::Arc;

/// The `size` px mark; one-colour logos and the folder take `tint`.
/// `host` is `Some` once the project's repository is known, holding its
/// host's name when it has one.
#[must_use]
pub fn project_mark(
    image: Option<Arc<RenderImage>>,
    host: Option<Option<String>>,
    size: f32,
    tint: Hsla,
) -> AnyElement {
    let slot = div().size(px(size)).flex_shrink_0();
    if let Some(image) = image {
        slot.child(
            img(image)
                .size(px(size))
                .rounded(RadiusTokens::value(RadiusStep::X2sMid))
                .object_fit(ObjectFit::Contain),
        )
        .into_any_element()
    } else if let Some(host) = host {
        let (asset, monochrome) = host_mark(host.as_deref());
        let mut glyph = asset_glyph(asset).size(px(size));
        if monochrome {
            glyph = glyph.text_color(tint);
        }
        slot.child(glyph).into_any_element()
    } else {
        slot.child(
            asset_glyph(AssetId::TABLER_FOLDER)
                .size(px(size))
                .text_color(tint),
        )
        .into_any_element()
    }
}

/// A repository host's mark, and whether it is one colour.
fn host_mark(host: Option<&str>) -> (AssetId, bool) {
    match host {
        Some("github") => (AssetId::SVGL_GITHUB, true),
        Some("gitlab") => (AssetId::SVGL_GITLAB, false),
        Some("azure") => (AssetId::SVGL_MICROSOFT_AZURE, false),
        Some("bitbucket") => (AssetId::SIMPLE_ICONS_BITBUCKET, false),
        Some("codeberg") => (AssetId::SIMPLE_ICONS_CODEBERG, false),
        Some("gitea") => (AssetId::SIMPLE_ICONS_GITEA, false),
        Some("sourcehut") => (AssetId::SIMPLE_ICONS_SOURCEHUT, false),
        _ => (AssetId::SVGL_GIT, false),
    }
}
