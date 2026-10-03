//! Compact project identity mark beside a recent thread's project label.

use artisan_domain::{ProjectId, RecentProjectIcon, RecentThreadListing};
use gpui::{Image, ImageFormat, ObjectFit, StyledImage as _, img};
use std::sync::Arc;

use super::*;
use crate::repository_mark::RepositoryHost;

#[derive(Default)]
pub(super) struct ProjectIconCache {
    images: HashMap<ProjectId, (Arc<[u8]>, Arc<Image>)>,
}

impl ProjectIconCache {
    pub(super) fn retain(&mut self, listing: &RecentThreadListing) {
        self.images.retain(|id, _| {
            listing
                .threads()
                .iter()
                .any(|row| &row.thread.project_id == id)
        });
    }

    pub(super) fn render(
        &mut self,
        id: &ProjectId,
        project: &RecentProjectIcon,
        theme: &ArtisanTheme,
    ) -> AnyElement {
        let Some(host) = &project.host else {
            return div()
                .size(px(14.0))
                .flex_shrink_0()
                .child(icon(IconStyle::resolve(
                    *theme,
                    AssetId::TABLER_FOLDER,
                    IconSize::Compact,
                    IconTint::Muted,
                )))
                .into_any_element();
        };
        if !project.png.is_empty() {
            let cached = self.images.entry(id.clone()).or_insert_with(|| {
                (
                    project.png.clone(),
                    Arc::new(Image::from_bytes(ImageFormat::Png, project.png.to_vec())),
                )
            });
            if cached.0 != project.png {
                *cached = (
                    project.png.clone(),
                    Arc::new(Image::from_bytes(ImageFormat::Png, project.png.to_vec())),
                );
            }
            let image = Arc::clone(&cached.1);
            return div()
                .size(px(14.0))
                .flex_shrink_0()
                .child(
                    img(image)
                        .size(px(14.0))
                        .rounded(px(3.0))
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element();
        }
        let host = host
            .as_str()
            .parse::<RepositoryHost>()
            .unwrap_or(RepositoryHost::Unknown);
        let mark = repository_mark_for(Some(host));
        let mut glyph = asset_glyph(repository_logo_asset(mark.logo)).size(px(14.0));
        if mark.monochrome {
            glyph = glyph.text_color(theme.colors.muted_foreground.to_paint());
        }
        div()
            .size(px(14.0))
            .flex_shrink_0()
            .child(glyph)
            .into_any_element()
    }
}
