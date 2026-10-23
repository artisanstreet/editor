//! Shared Artisan presentation and interaction primitives for `GPUI` consumers.

pub mod action_row;
pub mod asset_seam;
pub mod attachment_tile;
pub mod badge;
pub mod button;
pub mod card;
pub mod code_card;
pub mod copy_feedback;
pub mod dialog;
pub mod empty_state;
pub mod fade_arc;
mod fence_tokens;
pub mod focus_navigation;
pub mod fonts;
pub mod glass;
pub mod gradient;
pub mod hover_card;
pub mod icon;
pub mod image_preview;
pub mod inline_code_text;
pub mod keycap;
#[allow(unsafe_code, reason = "calls the Win32 locale formatters")]
pub mod local_time;
pub mod markdown;
pub mod markdown_renderer;
pub mod marquee;
pub mod measure;
pub mod media_frame;
pub mod motion;
pub mod overlay;
pub mod page;
pub mod picker_motion;
pub mod popover;
pub mod progress;
pub mod project_mark;
pub mod scroll_edge;
pub mod segmented_control;
pub mod select;
pub mod selectable_text;
pub mod shimmer_text;
pub mod slider;
pub mod status_chip;
pub mod switch;
pub mod tab_card;
pub mod terminal_text;
pub mod text_editor;
mod text_navigation;
pub mod text_runs;
mod textarea;
pub mod theme;
pub mod tree_row;
pub mod wheel_glide;
pub mod window_controls;
pub mod wordmark_loader;

pub use editor_assets::AssetId;

/// A harness's logo, wherever the app names a harness. OpenCode's
/// two-tone mark has a variant per theme mode; the rest tint or keep their
/// brand colour.
#[must_use]
pub const fn engine_icon(theme: &theme::ArtisanTheme, engine: artisan_api::EngineId) -> AssetId {
    match engine {
        artisan_api::EngineId::Claude => AssetId::SVGL_CLAUDE_AI,
        artisan_api::EngineId::Codex => AssetId::SVGL_OPENAI,
        artisan_api::EngineId::OpenCode if theme.colors.mode.is_dark() => AssetId::BRANDS_OPENCODE,
        artisan_api::EngineId::OpenCode => AssetId::BRANDS_OPENCODE_LIGHT,
    }
}

/// A model lab's logo, by the name the catalog gives it (`Model::lab`).
/// None for a lab without vendored artwork.
#[must_use]
pub fn lab_icon(lab: &str) -> Option<AssetId> {
    Some(match lab {
        "Alibaba" => AssetId::SVGL_QWEN,
        "Anthropic" => AssetId::SVGL_CLAUDE_AI,
        "DeepSeek" => AssetId::SVGL_DEEPSEEK,
        "Google" => AssetId::LOBE_GOOGLE,
        "Meituan" => AssetId::SIMPLE_ICONS_MEITUAN,
        "Meta" => AssetId::SVGL_META,
        "MiniMax" => AssetId::LOBE_MINIMAX,
        "Mistral" => AssetId::LOBE_MISTRAL,
        "Moonshot AI" => AssetId::BRANDS_KIMI,
        "NVIDIA" => AssetId::LOBE_NVIDIA,
        "OpenAI" => AssetId::SVGL_OPENAI,
        "Reflection AI" => AssetId::BRANDS_REFLECTION,
        "StepFun" => AssetId::LOBE_STEPFUN,
        "Tencent" => AssetId::LOBE_TENCENT,
        "xAI" => AssetId::LOBE_XAI,
        "Xiaomi" => AssetId::SIMPLE_ICONS_XIAOMI,
        "Z.ai" => AssetId::BRANDS_ZAI,
        _ => return None,
    })
}

/// A harness's brand colour.
///
/// It marks usage meters, and the edge of a message its agent sent to
/// another thread. Brands don't follow the theme: Claude is
/// its clay in both modes, OpenAI's Codex its black-or-white mark (the
/// foreground), OpenCode the neutral solid grey.
#[must_use]
pub const fn engine_accent(
    theme: &theme::ArtisanTheme,
    engine: artisan_api::EngineId,
) -> theme::Oklch {
    let colors = &theme.colors;
    match engine {
        artisan_api::EngineId::Claude => colors.claude,
        artisan_api::EngineId::Codex => colors.text,
        artisan_api::EngineId::OpenCode => colors.gray.step(9),
    }
}

#[must_use]
/// A context window as the model menu names it: `400K`, `1M`.
pub fn context_label(tokens: u64) -> String {
    if tokens >= 1_000_000 && tokens.is_multiple_of(1_000_000) {
        format!("{}M", tokens / 1_000_000)
    } else {
        format!("{}K", tokens / 1_000)
    }
}

#[must_use]
/// An effort as the model menu names it: `medium` reads `Medium`.
pub fn effort_label(word: &str) -> String {
    if word == "xhigh" {
        return "Extra High".to_owned();
    }
    let mut out = String::with_capacity(word.len());
    for (index, part) in word.split(['_', '-']).enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            if index == 0 {
                out.extend(first.to_uppercase());
            } else {
                out.push(first);
            }
            out.push_str(chars.as_str());
        }
    }
    out
}

/// A count or index as a layout float.
#[expect(
    clippy::cast_precision_loss,
    reason = "counts a view lays out stay far below 2^24"
)]
pub(crate) const fn f32_of_count(count: usize) -> f32 {
    count as f32
}

#[cfg(test)]
mod component_tests;

pub mod chart_navigation;
pub mod chat_bubble;
