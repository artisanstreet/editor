use crate::ui::theme::{ArtisanTheme, ThemeMode};
use artisan_api::EngineId;
use editor_assets::AssetId;

pub fn engine_icon(theme: &ArtisanTheme, engine: EngineId) -> AssetId {
    match engine {
        EngineId::Claude => AssetId::SVGL_CLAUDE_AI,
        EngineId::Codex => AssetId::SVGL_OPENAI,
        EngineId::OpenCode if theme.colors.mode == ThemeMode::Dark => AssetId::BRANDS_OPENCODE,
        EngineId::OpenCode => AssetId::BRANDS_OPENCODE_LIGHT,
    }
}

pub fn engine_accent(theme: &ArtisanTheme, engine: EngineId) -> aqua_core::GpuiHsla {
    match engine {
        EngineId::Claude => theme.colors.claude,
        EngineId::Codex => theme.colors.text,
        EngineId::OpenCode => theme.colors.gray.step(9),
    }
    .to_reference_paint()
}
