use crate::ui::kit::{IconSize, Radius};
use crate::ui::theme::Oklch;
use aqua::images::ImageHandle;
use aqua_core::{GpuiHsla, ImageRef, image::ImageFit};
use aqua_ui::element::{Box as UiBox, Element, Media};
use aqua_ui::style::Style;
use artisan_api::{ProjectId, ProjectRepository};
use editor_assets::AssetId;
use editor_core::HostData;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Forge {
    #[default]
    Folder,
    Repository {
        host: Option<String>,
    },
}

impl Forge {
    #[must_use]
    pub fn of(repository: Option<&ProjectRepository>) -> Self {
        repository.map_or(Self::Folder, |repository| Self::Repository {
            host: repository.host.clone(),
        })
    }

    fn asset(&self) -> AssetId {
        match self {
            Self::Folder => AssetId::TABLER_FOLDER,
            Self::Repository { host } => match host.as_deref() {
                Some("github") => AssetId::SVGL_GITHUB,
                Some("gitlab") => AssetId::SVGL_GITLAB,
                Some("azure") => AssetId::SVGL_MICROSOFT_AZURE,
                Some("bitbucket") => AssetId::SIMPLE_ICONS_BITBUCKET,
                Some("codeberg") => AssetId::SIMPLE_ICONS_CODEBERG,
                Some("gitea") => AssetId::SIMPLE_ICONS_GITEA,
                Some("sourcehut") => AssetId::SIMPLE_ICONS_SOURCEHUT,
                Some(_) | None => AssetId::SVGL_GIT,
            },
        }
    }
}

pub fn project_mark(
    image: Option<Arc<ImageHandle>>,
    forge: &Forge,
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
    size.glyph(forge.asset(), tint)
}

const ICON: IconSize = IconSize::Md;

#[derive(Debug, Default)]
pub struct Mark {
    icon: Option<Arc<ImageHandle>>,
    forge: Forge,
}

impl Mark {
    #[must_use]
    pub fn of(host: &HostData, project: &ProjectId) -> Self {
        Self {
            icon: host.project_icon(project),
            forge: Forge::of(host.repository(project)),
        }
    }

    pub fn view(&self, tint: Oklch) -> Element {
        UiBox::new()
            .width(ICON.px())
            .height(ICON.px())
            .style(Style::new().flex_shrink(0.0))
            .child(project_mark(
                self.icon.clone(),
                &self.forge,
                ICON,
                tint.to_reference_paint(),
            ))
            .build()
    }
}
