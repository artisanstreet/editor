//! Appearance (`appearance.svelte`), as the previous Editor painted it:
//! the app icon, typography, performance (live: the FPS limit and the
//! overlay, kept in this device's `settings.json`), formatting, glass and
//! reading. Everything but performance is not yet configurable.

use crate::Settings;
use crate::chrome::{card, footnote, header, row, section_shell};
use crate::frame_rate::{self, FrameRateLimit};
use crate::page;
use editor_core::SettingsPage;
use editor_ui::button::ButtonVariant;
use editor_ui::select::{Select, SelectEntry, SelectState};
use editor_ui::theme::ArtisanTheme;
use editor_ui::toggle_group::{ToggleGroup, ToggleGroupVariant};
use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, ParentElement as _, ScrollHandle,
    SharedString, Styled as _, Window, div, px,
};
use std::cell::RefCell;
use std::rc::Rc;

const DEFAULT_TEXT_FONT: &str = "Spline Sans";
const DEFAULT_CODE_FONT: &str = "Spline Sans Mono";
const APP_ICON_DEFAULT_LABEL: &str = "Plastic + jaw shading";
const APP_ICON_ALTERNATE_LABEL: &str = "Foreground plastic + gradient symbol";

/// The FPS select's open state, keyboard state and list scroll.
#[derive(Default)]
pub(crate) struct PerformanceControl {
    open: bool,
    interaction: Rc<RefCell<SelectState>>,
    scroll: ScrollHandle,
}

pub(crate) fn render(
    settings: &Settings,
    theme: &ArtisanTheme,
    window: &mut Window,
    cx: &mut Context<Settings>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .child(header(
            theme,
            page::label(SettingsPage::Appearance),
            page::description(SettingsPage::Appearance),
        ))
        .child(render_top(settings, theme))
        .child(render_frame_rate(settings, theme, window, cx))
        .child(render_bottom(settings, theme))
}

/// The app-icon and typography sections.
fn render_top(settings: &Settings, theme: &ArtisanTheme) -> Div {
    let app_icon = card(
        theme,
        vec![
            app_icon_option(theme, APP_ICON_DEFAULT_LABEL, "Default", true).into_any_element(),
            app_icon_option(theme, APP_ICON_ALTERNATE_LABEL, "Alternate", false).into_any_element(),
        ],
    );
    let restore = settings.static_button(
        "settings-typography-restore",
        "Restore defaults",
        ButtonVariant::Ghost,
    );
    let typography = card(
        theme,
        vec![
            typography_preview(theme).into_any_element(),
            row(
                theme,
                "Text",
                "Interface controls, page titles, and reading text.",
                Some(font_trigger(
                    settings,
                    "settings-font-text",
                    DEFAULT_TEXT_FONT,
                )),
            )
            .into_any_element(),
            row(
                theme,
                "Code",
                "Code, terminals, and the editor.",
                Some(font_trigger(
                    settings,
                    "settings-font-code",
                    DEFAULT_CODE_FONT,
                )),
            )
            .into_any_element(),
        ],
    );
    div()
        .flex()
        .flex_col()
        .child(section_shell(
            theme, "app-icon", "App icon", None, None, app_icon,
        ))
        .child(footnote(
            theme,
            "Open Appearance in the Artisan desktop app to switch its runtime icon.",
        ))
        .child(section_shell(
            theme,
            "typography",
            "Typography",
            Some("Font choices are not yet configurable in the native app."),
            Some(restore),
            typography,
        ))
        .child(footnote(
            theme,
            "Local font names are requested only when you open a picker. The list stays on this device; Artisan saves only the two family names you choose.",
        ))
}

/// One app-icon option, painted as a static row.
fn app_icon_option(theme: &ArtisanTheme, label: &str, caption: &str, selected: bool) -> Div {
    let mut option = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .rounded(px(12.0))
        .px(px(12.0))
        .py(px(12.0))
        .border_1()
        .border_color(theme.colors.border.to_paint());
    if selected {
        option = option.bg(theme.colors.muted.to_paint());
    }
    option.child(
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(4.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.foreground.to_paint())
                    .child(label.to_owned()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(caption.to_owned()),
            ),
    )
}

/// The text and code preview panes of the typography card.
fn typography_preview(theme: &ArtisanTheme) -> Div {
    let pane = |label: &'static str, sample: &'static str, code: bool| {
        let sample_text = div()
            .text_color(theme.colors.foreground.to_paint())
            .child(sample);
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(8.0))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(label),
            )
            .child(if code {
                sample_text.text_xs()
            } else {
                sample_text.text_sm()
            })
    };
    div()
        .w_full()
        .flex()
        .flex_row()
        .gap(px(16.0))
        .py(px(16.0))
        .child(pane("TEXT", "Quiet tools, clear decisions.", false))
        .child(pane("CODE", "craft = deliberate", true))
}

/// One font trigger (`font-picker.svelte`): the family with the selector
/// glyph; the picker itself needs local font discovery.
fn font_trigger(settings: &Settings, id: &'static str, family: &str) -> AnyElement {
    settings.static_button(id, format!("{family} \u{25be}"), ButtonVariant::Outline)
}

/// Performance (`native_frame_rate`): the FPS limit and the overlay.
fn render_frame_rate(
    settings: &Settings,
    theme: &ArtisanTheme,
    _window: &mut Window,
    cx: &mut Context<Settings>,
) -> Div {
    let core = settings.core.read(cx);
    let current = frame_rate::current(core);
    let overlay_visible = frame_rate::overlay_visible(core);
    let on_open = cx.listener(|settings, open: &bool, _, cx| {
        settings.performance.open = *open;
        cx.notify();
    });
    let on_change = cx.listener(|settings, limit: &FrameRateLimit, window, cx| {
        frame_rate::apply(&settings.core, *limit, window, cx);
        settings.performance.open = false;
        cx.notify();
    });
    let select = Select::new(
        "settings-frame-rate-limit",
        settings.control_focus.clone(),
        *theme,
        Some(current),
        FrameRateLimit::OPTIONS
            .iter()
            .map(|limit| SelectEntry::item(*limit, limit.label()))
            .collect(),
    )
    .open(settings.performance.open)
    .with_interaction_state(settings.performance.interaction.clone())
    .with_scroll_handle(settings.performance.scroll.clone())
    .debug_selector("settings-frame-rate-limit")
    .on_open_change(move |open, window, cx| on_open(&open, window, cx))
    .on_change(move |limit, _, window, cx| on_change(&limit, window, cx));
    let core = settings.core.clone();
    let overlay = settings.switch(
        "settings-fps-overlay",
        overlay_visible,
        Some(Box::new(move |visible, window, cx| {
            frame_rate::apply_overlay(&core, visible, window, cx);
        })),
    );
    section_shell(
        theme,
        "performance",
        "Performance",
        None,
        None,
        card(
            theme,
            vec![
                row(
                    theme,
                    "FPS limit",
                    "Limit animation and scrolling redraws. Unlimited disables VSync and the frame cap.",
                    Some(
                        div()
                            .w(px(144.0))
                            .flex_shrink_0()
                            .child(select)
                            .into_any_element(),
                    ),
                )
                .into_any_element(),
                row(
                    theme,
                    "Show FPS overlay",
                    "Display FPS and frame timings in the top-right corner.",
                    Some(overlay),
                )
                .into_any_element(),
            ],
        ),
    )
}

/// One disabled segmented group (`toggle-group` wrappers).
fn segmented(
    settings: &Settings,
    theme: &ArtisanTheme,
    id: &'static str,
    options: &[(&'static str, &'static str)],
    selected: &str,
) -> AnyElement {
    let mut group = ToggleGroup::single(id, *theme, Some(SharedString::from(selected.to_owned())))
        .variant(ToggleGroupVariant::Outline)
        .disabled(true)
        .debug_selector(id);
    for (value, label) in options {
        group = group.item(
            SharedString::from(*value),
            *label,
            settings.control_focus.clone(),
        );
    }
    group.into_any_element()
}

/// The formatting, glass and reading sections.
fn render_bottom(settings: &Settings, theme: &ArtisanTheme) -> Div {
    let time_options = [("12-hour", "12-hour"), ("24-hour", "24-hour")];
    let separator_options = [("backslash", "\\"), ("forward-slash", "/")];
    let width_options = [
        ("tight", "Tight"),
        ("balanced", "Balanced"),
        ("loose", "Loose"),
    ];
    let formatting = card(
        theme,
        vec![
            row(
                theme,
                "Time format",
                "How local times are written throughout Artisan.",
                Some(segmented(
                    settings,
                    theme,
                    "settings-time-format",
                    &time_options,
                    "12-hour",
                )),
            )
            .into_any_element(),
            row(
                theme,
                "Path separator",
                "Which separator file and folder paths use when displayed.",
                Some(segmented(
                    settings,
                    theme,
                    "settings-path-separator",
                    &separator_options,
                    "backslash",
                )),
            )
            .into_any_element(),
        ],
    );
    let glass = card(
        theme,
        vec![
            row(
                theme,
                "Shader under glass",
                "Lights glass surfaces with the animated shader they were designed around. Turning it off leaves the glass itself intact \u{2014} the material, highlight, and depth stay \u{2014} and only the moving light stops.",
                Some(settings.switch("settings-switch-shader", true, None)),
            )
            .into_any_element(),
        ],
    );
    let reading = card(
        theme,
        vec![
            row(
                theme,
                "Prose width",
                "How wide the transcript's reading column runs. Balanced is the width Artisan was designed at; Tight shortens the line for focus, Loose spends more of the window on text.",
                Some(segmented(
                    settings,
                    theme,
                    "settings-prose-width",
                    &width_options,
                    "balanced",
                )),
            )
            .into_any_element(),
        ],
    );
    div()
        .flex()
        .flex_col()
        .child(section_shell(
            theme,
            "formatting",
            "Formatting",
            Some("Time and path display choices are not yet configurable in the native app."),
            None,
            formatting,
        ))
        .child(section_shell(
            theme,
            "glass",
            "Glass",
            Some("The shader choice is not yet configurable in the native app."),
            None,
            glass,
        ))
        .child(section_shell(
            theme,
            "reading",
            "Reading",
            Some("The prose-width choice is not yet configurable in the native app."),
            None,
            reading,
        ))
}
