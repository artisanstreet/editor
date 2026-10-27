//! Device preferences: appearance, streaming, notifications and performance.
use crate::ui::{PageBlock, card, header, row, section_shell};
use crate::{Settings, frame_rate, page};
use editor_core::{FONT_SET_KEY, OUTLINE_TEXT_KEY, STREAMING_MODE_KEY, SettingsSection};
use editor_ui::{
    select::{Select, SelectEntry},
    streaming::StreamingMode,
    theme::{self as theme_tokens, ArtisanTheme, FontSet, ThemeMode},
};
use gpui::{AnyElement, Context, Div, IntoElement, ParentElement as _, Styled as _, div, px};
use serde_json::Value;

/// System notifications when a thread finishes, fails or asks; default on.
pub const NOTIFICATIONS_KEY: &str = "notifications";

pub fn render(settings: &Settings, theme: &ArtisanTheme, cx: &Context<Settings>) -> Div {
    let page = settings.page(cx);
    let mut content =
        div()
            .flex()
            .flex_col()
            .child(header(theme, page.label(), page::description(page)));
    if page.shows(SettingsSection::Appearance) {
        content = content.child(render_appearance(settings, theme, cx));
    }
    if page.shows(SettingsSection::Streaming) {
        content = content.child(section_shell(
            theme,
            "Text Streaming",
            None,
            None,
            card(theme, vec![streaming_row(settings, theme, cx)]),
        ));
    }
    if page.shows(SettingsSection::Notifications) {
        content = content.child(render_notifications(settings, theme, cx));
    }
    if page.shows(SettingsSection::Performance) {
        content = content.child(render_performance(settings, theme, cx));
    }
    content
}

/// Appearance: light or dark, the font set and how text is rasterized.
/// Switching modes plays the diagonal mode transition.
fn render_appearance(
    settings: &Settings,
    theme: &ArtisanTheme,
    cx: &Context<Settings>,
) -> PageBlock {
    let fonts = settings.core.read(cx).font_set();
    let fonts_core = settings.core.clone();
    let fonts_select = Select::new(
        "settings-font-set",
        *theme,
        Some(fonts),
        FontSet::ALL
            .iter()
            .map(|fonts| SelectEntry::item(*fonts, fonts.label()))
            .collect(),
    )
    .label("Fontset")
    .debug_selector("settings-font-set")
    .on_change(move |fonts: FontSet, _, _, cx| {
        fonts_core.update(cx, |core, cx| {
            core.set_setting(FONT_SET_KEY, Value::from(fonts.key()), cx);
        });
    });
    let outline_core = settings.core.clone();
    let outline = settings.switch(
        "settings-switch-outline-text",
        settings.core.read(cx).outline_text(),
        Box::new(move |checked, _, cx| {
            outline_core.update(cx, |core, cx| {
                core.set_setting(OUTLINE_TEXT_KEY, Value::Bool(checked), cx);
            });
        }),
    );
    let core = settings.core.clone();
    let dark = settings.switch(
        "settings-switch-dark-mode",
        theme.colors.mode.is_dark(),
        Box::new(move |checked, window, cx| {
            let mode = if checked {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            };
            theme_tokens::begin_mode_transition(mode, window, cx);
            core.update(cx, |core, cx| {
                core.set_setting("dark_mode", Value::Bool(checked), cx);
            });
        }),
    );
    section_shell(
        theme,
        "Appearance",
        None,
        None,
        card(
            theme,
            vec![
                row(
                    theme,
                    "Dark Mode",
                    "Whether to draw Artisan Editor in a dark theme.",
                    Some(dark),
                )
                .into_any_element(),
                row(
                    theme,
                    "Fontset",
                    "Choose what set of fonts to Artisan Editor will render in.",
                    Some(
                        div()
                            .w(px(144.0))
                            .flex_shrink_0()
                            .child(fonts_select)
                            .into_any_element(),
                    ),
                )
                .into_any_element(),
                row(
                    theme,
                    "Outline text rendering",
                    "Unhinted grayscale glyphs with no gamma or contrast correction, so letters keep their designed shapes. Turn off if text looks too soft or thin.",
                    Some(outline),
                )
                .into_any_element(),
            ],
        ),
    )
    .debug_selector("settings-section-appearance")
}

fn render_notifications(
    settings: &Settings,
    theme: &ArtisanTheme,
    cx: &Context<Settings>,
) -> PageBlock {
    let enabled = settings
        .core
        .read(cx)
        .settings
        .get(NOTIFICATIONS_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let core = settings.core.clone();
    let switch = settings.switch(
        "settings-switch-notify",
        enabled,
        Box::new(move |checked, _, cx| {
            core.update(cx, |core, cx| {
                core.set_setting(NOTIFICATIONS_KEY, Value::Bool(checked), cx);
            });
        }),
    );
    section_shell(
        theme,
        "Notifications",
        None,
        None,
        card(
            theme,
            vec![
                row(
                    theme,
                    "Notify Me",
                    "Uses your operating systems notification scheduler to alert you when threads finish, is stuck or out right failed.",
                    Some(switch),
                )
                .into_any_element(),
            ],
        ),
    )
    .debug_selector("settings-section-notifications")
}

/// This device's streaming mode, as a dropdown.
fn streaming_row(settings: &Settings, theme: &ArtisanTheme, cx: &Context<Settings>) -> AnyElement {
    let mode = settings.core.read(cx).streaming_mode();
    let core = settings.core.clone();
    let select = Select::new(
        "settings-streaming-mode",
        *theme,
        Some(mode),
        StreamingMode::ALL
            .iter()
            .map(|mode| SelectEntry::item(*mode, mode.label()))
            .collect(),
    )
    .label("Text Streaming")
    .debug_selector("settings-streaming-mode")
    .on_change(move |mode: StreamingMode, _, _, cx| {
        core.update(cx, |core, cx| {
            core.set_setting(STREAMING_MODE_KEY, Value::from(mode.key()), cx);
        });
    });
    row(
        theme,
        "Text Streaming",
        "How conversations in Artisan Editor streams. We recommend you leave it at smooth, but for models with higher TPS we recommend you should try the others.",
        Some(
            div()
                .w(px(144.0))
                .flex_shrink_0()
                .child(select)
                .into_any_element(),
        ),
    )
    .into_any_element()
}

/// Performance (`native_frame_rate`): the lower refresh rate.
fn render_performance(
    settings: &Settings,
    theme: &ArtisanTheme,
    cx: &Context<Settings>,
) -> PageBlock {
    let core = settings.core.clone();
    let lower = settings.switch(
        "settings-lower-refresh-rate",
        frame_rate::lowered(core.read(cx)),
        Box::new(move |lowered, window, cx| {
            frame_rate::apply(&core, lowered, window, cx);
        }),
    );
    section_shell(
        theme,
        "Performance",
        None,
        None,
        card(
            theme,
            vec![
                row(
                    theme,
                    "Lower refresh rate",
                    "Caps animation and scrolling at 60 frames per second to save power. Off, the Editor matches your monitor's refresh rate.",
                    Some(lower),
                )
                .into_any_element(),
            ],
        ),
    )
    .debug_selector("settings-section-performance")
}
