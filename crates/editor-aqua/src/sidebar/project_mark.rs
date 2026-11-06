use std::sync::Arc;

use aqua::images::ImageHandle;
use aqua_core::{GpuiHsla, ImageRef, image::ImageFit};
use aqua_ui::element::{Element, Media};
use editor_assets::AssetId;

use crate::ui::{
    asset::source_asset_glyph,
    theme::{RadiusStep, RadiusTokens},
};

pub fn project_mark(
    image: Option<Arc<ImageHandle>>,
    host: Option<Option<&str>>,
    size: f32,
    tint: GpuiHsla,
) -> Element {
    if let Some(image) = image {
        return Media::new(ImageRef::with_lease(image.id(), image))
            .width(size)
            .height(size)
            .radius(RadiusTokens::value(RadiusStep::X2sMid))
            .fit(ImageFit::Contain)
            .build();
    }
    let asset = match host {
        None => AssetId::TABLER_FOLDER,
        Some(Some("github")) => AssetId::SVGL_GITHUB,
        Some(Some("gitlab")) => AssetId::SVGL_GITLAB,
        Some(Some("azure")) => AssetId::SVGL_MICROSOFT_AZURE,
        Some(Some("bitbucket")) => AssetId::SIMPLE_ICONS_BITBUCKET,
        Some(Some("codeberg")) => AssetId::SIMPLE_ICONS_CODEBERG,
        Some(Some("gitea")) => AssetId::SIMPLE_ICONS_GITEA,
        Some(Some("sourcehut")) => AssetId::SIMPLE_ICONS_SOURCEHUT,
        Some(_) => AssetId::SVGL_GIT,
    };
    source_asset_glyph(asset, size, tint)
}
