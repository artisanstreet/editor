use std::sync::Arc;

use aqua_core::DisplayItem;
use aqua_ui::element::{Canvas, Element};
use aqua_ui::style::Style;
use editor_assets::AssetId;

use crate::ui::theme::Oklch;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Icon {
    Search,
    Gear,
    User,
    Info,
    Laptop,
    Messages,
    FileText,
    Plug,
    Tool,
    Close,
    Sun,
    Moon,
    Selector,
    Logout,
    Download,
    ChevronDown,
    ChevronUp,
    Refresh,
    Pencil,
    Trash,
    Key,
    World,
    Terminal,
    FolderCode,
    Unarchive,
    Spinner,
}

impl Icon {
    const fn tabler(self) -> (AssetId, f32) {
        match self {
            Self::Search => (AssetId::TABLER_SEARCH, 1.8),
            Self::Gear => (AssetId::TABLER_SETTINGS, 1.8),
            Self::User => (AssetId::TABLER_USER, 1.8),
            Self::Info => (AssetId::TABLER_INFO_CIRCLE, 1.8),
            Self::Laptop => (AssetId::TABLER_DEVICE_LAPTOP, 1.8),
            Self::Messages => (AssetId::TABLER_MESSAGES, 1.8),
            Self::FileText => (AssetId::TABLER_FILE_TEXT, 1.8),
            Self::Plug => (AssetId::TABLER_PLUG_CONNECTED, 1.8),
            Self::Tool => (AssetId::TABLER_TOOL, 1.8),
            Self::Close => (AssetId::TABLER_X, 1.8),
            Self::Sun => (AssetId::TABLER_SUN, 1.8),
            Self::Moon => (AssetId::TABLER_MOON, 1.8),
            Self::Selector => (AssetId::TABLER_SELECTOR, 2.0),
            Self::Logout => (AssetId::TABLER_LOGOUT, 1.9),
            Self::Download => (AssetId::TABLER_DOWNLOAD, 2.0),
            Self::ChevronDown => (AssetId::TABLER_CHEVRON_DOWN, 2.0),
            Self::ChevronUp => (AssetId::TABLER_CHEVRON_UP, 2.0),
            Self::Refresh => (AssetId::TABLER_REFRESH, 2.0),
            Self::Pencil => (AssetId::TABLER_PENCIL, 1.8),
            Self::Trash => (AssetId::TABLER_TRASH, 1.8),
            Self::Key => (AssetId::TABLER_KEY, 1.8),
            Self::World => (AssetId::TABLER_WORLD, 1.8),
            Self::Terminal => (AssetId::TABLER_TERMINAL_2, 1.8),
            Self::FolderCode => (AssetId::TABLER_FOLDER_CODE, 1.8),
            Self::Unarchive => (AssetId::TABLER_ARROW_BACK_UP, 1.8),
            Self::Spinner => (AssetId::TABLER_LOADER_2, 2.2),
        }
    }

    #[must_use]
    pub(crate) fn source(self) -> String {
        let (id, stroke) = self.tabler();
        editor_assets::get(id).source.replacen(
            r#"stroke-width="2""#,
            &format!(r#"stroke-width="{stroke}""#),
            1,
        )
    }
}

#[must_use]
pub fn glyph(icon: Icon, edge: f32, color: Oklch) -> Element {
    let source: Arc<str> = icon.source().into();
    let tint = color.to_reference_paint();
    Canvas::new()
        .width(edge)
        .height(edge)
        .style(Style::new().flex_shrink(0.0))
        .scroll_stable()
        .on_paint(move |bounds| {
            vec![DisplayItem::GpuiSvg {
                bounds,
                source: Arc::clone(&source),
                tint: Some(tint),
                opacity: 1.0,
            }]
        })
        .build()
        .exclude_from_hit_testing()
}
