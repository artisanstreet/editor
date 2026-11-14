use crate::ui::kit::{IconSize, Radius};
use crate::ui::theme::Oklch;
use aqua::images::ImageHandle;
use aqua_core::{GpuiHsla, ImageRef, image::ImageFit};
use aqua_ui::element::{Box as UiBox, Element, Media};
use aqua_ui::style::Style;
use artisan_api::ProjectId;
use editor_assets::AssetId;
use editor_core::HostData;
use std::sync::Arc;

pub fn project_mark(
    image: Option<Arc<ImageHandle>>,
    host: Option<Option<&str>>,
    size: IconSize,
    tint: GpuiHsla,
) -> Element {
    if let Some(image) = image {
        return Media::new(ImageRef::with_lease(image.id(), image))
            .width(size.px())
            .height(size.px())
            .style(Radius::Row.style())
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
    size.glyph(asset, tint)
}

const ICON: IconSize = IconSize::Md;

pub struct Mark {
    icon: Option<Arc<ImageHandle>>,
    forge: Option<Option<String>>,
}

impl Mark {
    #[must_use]
    pub fn of(host: &HostData, project: &ProjectId) -> Self {
        Self {
            icon: host.project_icon(project),
            forge: host
                .repository(project)
                .map(|repository| repository.host.clone()),
        }
    }

    pub fn view(&self, tint: Oklch) -> Element {
        UiBox::new()
            .width(ICON.px())
            .height(ICON.px())
            .style(Style::new().flex_shrink(0.0))
            .child(project_mark(
                self.icon.clone(),
                self.forge.as_ref().map(Option::as_deref),
                ICON,
                tint.to_reference_paint(),
            ))
            .build()
    }
}
