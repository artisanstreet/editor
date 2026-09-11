//! Native desktop workspace composition.
//!
//! This module owns only the frame: the application supplies the real
//! sidebar, toolbar, route surface, search menu, and composer entities. That
//! keeps the native window geometry independent from route and transport
//! policy while giving every route the same quiet neutral-dark workspace.

#![forbid(unsafe_code)]

use artisan_assets::AssetId;
use artisan_ui::asset_seam::asset_glyph;
use artisan_ui::theme::{ArtisanTheme, DesktopTheme, ThemeMode};
use gpui::prelude::{
    InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
};
use gpui::{AnyElement, Div, FontWeight, Pixels, WindowControlArea, div, px};

use crate::shell::title_bar_caption_button;

/// Root selector for the mounted native workspace.
pub const DESKTOP_ROOT_SELECTOR: &str = "artisan-desktop-workspace";
/// Native titlebar selector.
pub const DESKTOP_TITLEBAR_SELECTOR: &str = "artisan-desktop-titlebar";
/// Sidebar selector.
pub const DESKTOP_SIDEBAR_SELECTOR: &str = "artisan-desktop-sidebar";
/// Main workspace selector.
pub const DESKTOP_MAIN_SELECTOR: &str = "artisan-desktop-main";
/// Route body selector.
pub const DESKTOP_BODY_SELECTOR: &str = "artisan-desktop-body";
/// Home route selector.
pub const DESKTOP_HOME_SELECTOR: &str = "artisan-desktop-home";
/// Actual composer wrapper selector.
pub const DESKTOP_COMPOSER_SELECTOR: &str = "artisan-desktop-composer";
/// Sidebar projects section selector.
pub const DESKTOP_PROJECTS_SELECTOR: &str = "artisan-desktop-projects";
/// Sidebar threads section selector.
pub const DESKTOP_THREADS_SELECTOR: &str = "artisan-desktop-threads";
/// Sidebar empty-state selector.
pub const DESKTOP_EMPTY_SELECTOR: &str = "artisan-desktop-empty";
/// Sidebar offline-state selector.
pub const DESKTOP_OFFLINE_SELECTOR: &str = "artisan-desktop-offline";
/// Sidebar collapse control selector.
pub const DESKTOP_COLLAPSE_SELECTOR: &str = "artisan-desktop-collapse";

/// Native workspace titlebar height.
pub const DESKTOP_TITLEBAR_HEIGHT_PX: f32 = 48.0;
/// Expanded sidebar width.
pub const DESKTOP_SIDEBAR_WIDTH_PX: f32 = 218.0;
/// Compact sidebar width when labels are collapsed.
pub const DESKTOP_SIDEBAR_COLLAPSED_WIDTH_PX: f32 = 58.0;
/// Crosshair arm length.
pub const DESKTOP_CROSSHAIR_SIZE_PX: f32 = 12.0;
/// Native titlebar control width.
pub const DESKTOP_TITLEBAR_CONTROL_WIDTH_PX: f32 = 46.0;

/// Geometry resolved at the shell boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DesktopShellStyle {
    /// Titlebar height.
    pub titlebar_height: Pixels,
    /// Current sidebar width.
    pub sidebar_width: Pixels,
    /// One physical pixel expressed in logical pixels.
    pub one_device_pixel: Pixels,
}

impl DesktopShellStyle {
    /// Resolve the shell geometry for the current display scale.
    #[must_use]
    pub fn resolve(collapsed: bool, scale_factor: f32) -> Self {
        let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };

        Self {
            titlebar_height: px(DESKTOP_TITLEBAR_HEIGHT_PX),
            sidebar_width: px(if collapsed {
                DESKTOP_SIDEBAR_COLLAPSED_WIDTH_PX
            } else {
                DESKTOP_SIDEBAR_WIDTH_PX
            }),
            one_device_pixel: px(1.0 / scale_factor),
        }
    }
}

/// Paints the small crosshair used where the shell's major rules meet.
///
/// This is intentionally a plain layout element: it has no id, focus handle,
/// or pointer listener, so it cannot intercept a click meant for a nearby
/// control. The stroke thickness is supplied by the display-aware shell
/// style so it remains one physical pixel on scaled Windows displays.
#[must_use]
pub fn junction_crosshair(theme: DesktopTheme, stroke: Pixels) -> Div {
    let crosshair_offset = px((DESKTOP_CROSSHAIR_SIZE_PX - f32::from(stroke)) / 2.0);
    div()
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(DESKTOP_CROSSHAIR_SIZE_PX))
        .h(px(DESKTOP_CROSSHAIR_SIZE_PX))
        .child(
            div()
                .absolute()
                .left(px(0.0))
                .top(crosshair_offset)
                .w(px(DESKTOP_CROSSHAIR_SIZE_PX))
                .h(stroke)
                .bg(theme.crosshair),
        )
        .child(
            div()
                .absolute()
                .left(crosshair_offset)
                .top(px(0.0))
                .w(stroke)
                .h(px(DESKTOP_CROSSHAIR_SIZE_PX))
                .bg(theme.crosshair),
        )
}

/// Compose the complete native desktop frame around application-owned
/// surfaces.
///
/// The window root paints one continuous true-black shell face (the explicit
/// black-shell request overriding the Electron dark `surface-900 → surface-925`
/// card gradient for the frame). Titlebar, sidebar, and main wrappers stay
/// transparent so the shell never restarts a background per pane; controls,
/// composer, cards, and popovers keep their own glass/material fills for
/// contrast.
#[must_use]
pub fn desktop_shell(
    theme: DesktopTheme,
    collapsed: bool,
    identity: AnyElement,
    title: AnyElement,
    search: AnyElement,
    sidebar: AnyElement,
    body: AnyElement,
    scale_factor: f32,
    maximized: bool,
) -> Div {
    let style = DesktopShellStyle::resolve(collapsed, scale_factor);
    let legacy_theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    // Own paint override: true black for the whole shell frame. Cards,
    // controls, and overlays paint their own surfaces above this root.
    let window_background = crate::thread_screen::shell_black();

    let controls = div()
        .flex()
        .h_full()
        .flex_shrink_0()
        .debug_selector(|| "artisan-desktop-titlebar-controls".to_string())
        .child(title_bar_caption_button(
            legacy_theme,
            WindowControlArea::Min,
            "artisan-desktop-titlebar-minimize",
        ))
        .child(maximize_button(theme, maximized, style.one_device_pixel))
        .child(title_bar_caption_button(
            legacy_theme,
            WindowControlArea::Close,
            "artisan-desktop-titlebar-close",
        ));

    let drag = div()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .flex()
                .items_center()
                .px(px(14.0))
                .overflow_hidden()
                .child(identity)
                .child(div().flex_1().h_full().window_control_area(WindowControlArea::Drag)),
        )
        .child(
            div()
                .w(px(360.0))
                .max_w(gpui::relative(0.4))
                .flex_shrink_0()
                .min_w(px(0.0))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                // The reserved centre slot carries the open conversation's
                // title as the titlebar header; the unanchored command menu
                // renders nothing in flow at rest and overlays its dialog when
                // open, so the header is never displaced by it.
                .child(title)
                .child(search),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .flex()
                .justify_end()
                .child(div().flex_1().h_full().window_control_area(WindowControlArea::Drag))
                .child(controls),
        );

    let titlebar = div()
        .relative()
        .w_full()
        .h(style.titlebar_height)
        .flex_shrink_0()
        .flex()
        .items_center()
        .border_b_1()
        .border_color(theme.line)
        .debug_selector(|| DESKTOP_TITLEBAR_SELECTOR.to_string())
        .child(drag)
        .child(
            // One-physical-pixel continuation of the sidebar's right rule
            // above the junction. The sidebar border paints the rightmost
            // logical pixel inside `sidebar_width` ([sw-1, sw], border-box
            // inset), and the junction crosshair centers its vertical arm on
            // x = sw, so a rule at [sw-1dp, sw] extends exactly that line
            // through the full header height in the same `theme.line` paint.
            // Absolute, so the drag/search/control flex layout is untouched;
            // a plain element with no pointer listener, so like the
            // crosshair it cannot intercept drags or clicks.
            div()
                .absolute()
                .left(style.sidebar_width - style.one_device_pixel)
                .top(px(0.0))
                .w(style.one_device_pixel)
                .h(style.titlebar_height)
                .bg(theme.line)
                .debug_selector(|| "artisan-desktop-titlebar-divider".to_owned()),
        );

    let main = div()
        .relative()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_row()
        .child(
            div()
                .w(style.sidebar_width)
                .h_full()
                .flex_shrink_0()
                .border_r_1()
                .border_color(theme.line)
                .debug_selector(|| DESKTOP_SIDEBAR_SELECTOR.to_string())
                .child(sidebar),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .debug_selector(|| DESKTOP_MAIN_SELECTOR.to_string())
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        .flex()
                        .flex_col()
                        .debug_selector(|| DESKTOP_BODY_SELECTOR.to_string())
                        .child(body),
                ),
        );

    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .bg(window_background)
        .text_color(theme.foreground)
        .font_family(
            ArtisanTheme::for_mode(ThemeMode::Dark)
                .typography
                .sans
                .family,
        )
        .text_size(px(14.0))
        .debug_selector(|| DESKTOP_ROOT_SELECTOR.to_string())
        .child(titlebar)
        .child(main)
        .child(
            junction_crosshair(theme, style.one_device_pixel)
                .left(style.sidebar_width - px(6.0))
                .top(style.titlebar_height - px(6.0)),
        )
}

/// Windows maximize/restore glyph follows the actual window state.
fn maximize_button(theme: DesktopTheme, maximized: bool, stroke: Pixels) -> gpui::Stateful<Div> {
    let mut glyph = div().relative().w(px(10.0)).h(px(10.0));
    if maximized {
        glyph = glyph
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(px(7.0))
                    .h(px(7.0))
                    .border_t(stroke)
                    .border_r(stroke)
                    .border_color(theme.foreground),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w(px(7.0))
                    .h(px(7.0))
                    .border(stroke)
                    .border_color(theme.foreground),
            );
    } else {
        glyph = glyph.border(stroke).border_color(theme.foreground);
    }
    div()
        .id("artisan-desktop-titlebar-maximize")
        .w(px(DESKTOP_TITLEBAR_CONTROL_WIDTH_PX))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .hover(move |style| style.bg(theme.selected))
        .window_control_area(WindowControlArea::Max)
        .on_click(|event, window, _| {
            if event.is_keyboard() {
                window.zoom_window();
            }
        })
        .child(glyph)
}

/// Small square glyph used by desktop-only navigation rows.
#[must_use]
pub fn desktop_nav_glyph(asset: AssetId, theme: DesktopTheme) -> Div {
    div()
        .w(px(16.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .text_color(theme.secondary)
        .child(asset_glyph(asset).size(px(15.0)))
}

/// Shared title treatment for the compact desktop chrome.
#[must_use]
pub fn desktop_label(theme: DesktopTheme, text: impl Into<String>) -> Div {
    div()
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.secondary)
        .child(text.into())
}

/// Resolve a low-emphasis text color against a neutral desktop surface.
#[must_use]
pub fn desktop_muted(theme: DesktopTheme, text: impl Into<String>) -> Div {
    div()
        .text_size(px(13.0))
        .text_color(theme.secondary)
        .child(text.into())
}

/// Make a one-pixel desktop rule without introducing another palette.
#[must_use]
pub fn desktop_rule(theme: DesktopTheme) -> Div {
    div().h(px(1.0)).w_full().bg(theme.line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_shell_keeps_compact_native_geometry() {
        assert_eq!(DESKTOP_TITLEBAR_HEIGHT_PX, 48.0);
        assert_eq!(DESKTOP_SIDEBAR_WIDTH_PX, 218.0);
        assert_eq!(DESKTOP_CROSSHAIR_SIZE_PX, 12.0);

        let expanded = DesktopShellStyle::resolve(false, 1.0);
        let collapsed = DesktopShellStyle::resolve(true, 1.0);
        assert!(collapsed.sidebar_width < expanded.sidebar_width);
        assert_eq!(expanded.one_device_pixel, px(1.0));
    }

    #[test]
    fn desktop_shell_resolves_one_physical_pixel_on_scaled_displays() {
        assert_eq!(
            DesktopShellStyle::resolve(false, 1.25).one_device_pixel,
            px(0.8)
        );
        assert_eq!(
            DesktopShellStyle::resolve(false, 0.0).one_device_pixel,
            px(1.0)
        );
    }
}
