//! General: this device's theme, notifications and redraw controls, kept
//! in its `settings.json`, and how the selected host's Forge carries
//! threads on by itself.
use crate::ui::{PageBlock, card, header, row, section_shell};
use crate::{Settings, frame_rate, page, threads};
use editor_core::{FONT_SET_KEY, HostData, OUTLINE_TEXT_KEY, SettingsPage};
use editor_ui::{
    select::{Select, SelectEntry},
    theme::{self as theme_tokens, ArtisanTheme, FontSet, ThemeMode},
};
use gpui::{
    AnyElement, Context, Div, Entity, IntoElement, ParentElement as _, Styled as _, div, px,
};
use serde_json::Value;

/// System notifications when a thread finishes, fails or asks; default on.
pub const NOTIFICATIONS_KEY: &str = "notifications";

pub fn render(
    settings: &Settings,
    host: Option<&Entity<HostData>>,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .child(header(
            theme,
            page::label(SettingsPage::General),
            page::description(SettingsPage::General),
        ))
        .child(render_appearance(settings, theme, cx))
        .child(render_notifications(settings, theme, cx))
        .children(host.map(|host| render_behaviour(settings, host, theme, cx)))
        .child(render_performance(settings, theme, cx))
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
                    "Dark mode",
                    "Draw the Editor dark. The sun and moon in the profile menu switch it too.",
                    Some(dark),
                )
                .into_any_element(),
                row(
                    theme,
                    "Fontset",
                    "Spline Sans and Spline Sans Mono, bundled and tuned for the Editor, or this system's UI and monospace fonts. Choose System to match your other apps.",
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
                    "Notify me",
                    "Posts a system notification when a thread finishes, fails, or needs an answer from you. It clears itself after a few seconds; an approval or a question keeps waiting in its thread.",
                    Some(switch),
                )
                .into_any_element(),
            ],
        ),
    )
    .debug_selector("settings-section-notifications")
}

/// Behaviour: when the host's Forge carries a thread's turn on by itself.
/// All are Forge settings; each uses its declared default.
fn render_behaviour(
    settings: &Settings,
    host: &Entity<HostData>,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> PageBlock {
    let switch = |id: &'static str, key: &'static str, cx: &mut Context<Settings>| {
        let enabled = host
            .read(cx)
            .setting(key)
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                artisan_api::setting_decl(key)
                    .and_then(|decl| decl.default_value().as_bool())
                    .unwrap_or(true)
            });
        let host = host.clone();
        settings.switch(
            id,
            enabled,
            Box::new(move |checked, _, cx| {
                threads::save_switch(&host, key, checked, cx);
            }),
        )
    };
    let overloaded = switch(
        "settings-switch-retry-overloaded",
        "retry_when_overloaded",
        cx,
    );
    let usage = switch(
        "settings-switch-continue-usage-limit",
        "continue_after_usage_limit",
        cx,
    );
    let restart = switch(
        "settings-switch-continue-restart",
        "continue_after_restart",
        cx,
    );
    let enhanced = native_claude_switch(settings, host, cx);
    section_shell(
        theme,
        "Behaviour",
        None,
        None,
        card(
            theme,
            vec![
                row(
                    theme,
                    "Keep retrying when the provider is overloaded",
                    "When a turn fails because the model is at capacity or the provider has a server error, the thread waits and tries again: after 15 seconds, then twice as long each time up to every 5 minutes, until it goes through.",
                    Some(overloaded),
                )
                .into_any_element(),
                row(
                    theme,
                    "Continue when my usage limit resets",
                    "When you run out of usage, the thread waits for the limit to reset and then carries on where it stopped. If the reset time is unknown, it tries again every 15 minutes.",
                    Some(usage),
                )
                .into_any_element(),
                row(
                    theme,
                    "Continue after a restart",
                    "When Forge restarts for an update or after a crash, threads that were working carry on where they stopped.",
                    Some(restart),
                )
                .into_any_element(),
                row(
                    theme,
                    "Native Claude Mode",
                    "Runs Claude's turns on claude.ai's backend: the same files, commands, skills, MCP servers and subagents on your machine, plus the provider's own thinking recaps. If you run into issues with Claude, turn this off.",
                    Some(enhanced),
                )
                .into_any_element(),
            ],
        ),
    )
    .debug_selector("settings-section-behaviour")
}

/// The account's Claude transport choice, shared by every Claude thread.
fn native_claude_switch(
    settings: &Settings,
    host: &Entity<HostData>,
    cx: &Context<Settings>,
) -> AnyElement {
    let on = artisan_api::transport_of(host.read(cx).setting(artisan_api::TRANSPORT_KEY))
        == artisan_api::CLAUDE_AI;
    let host = host.clone();
    settings.switch(
        "settings-switch-enhanced-claude",
        on,
        Box::new(move |checked, _, cx| {
            let transport = if checked {
                artisan_api::CLAUDE_AI
            } else {
                artisan_api::CLAUDE_CODE
            };
            threads::save_value(
                &host,
                artisan_api::TRANSPORT_KEY,
                Value::String(transport.to_owned()),
                cx,
            );
        }),
    )
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
