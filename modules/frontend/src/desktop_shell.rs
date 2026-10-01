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
use gpui::{AnyElement, Div, FontWeight, Pixels, SharedString, WindowControlArea, div, px};

use crate::shell::title_bar_caption_button;

/// Root selector for the mounted native workspace.
pub const DESKTOP_ROOT_SELECTOR: &str = "artisan-desktop-workspace";
/// Native titlebar selector.
pub const DESKTOP_TITLEBAR_SELECTOR: &str = "artisan-desktop-titlebar";
/// Titlebar sidebar section selector, holding the wordmark above the sidebar.
pub const DESKTOP_TITLEBAR_BRAND_SELECTOR: &str = "artisan-desktop-titlebar-brand";
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
/// Junction crosshair where the right inspector column's left rule meets the
/// titlebar's bottom rule.
pub const DESKTOP_INSPECTOR_JUNCTION_SELECTOR: &str = "artisan-desktop-inspector-junction";

/// Native workspace titlebar height.
pub const DESKTOP_TITLEBAR_HEIGHT_PX: f32 = 48.0;
/// Horizontal inset of the titlebar's content section.
///
/// Matches the wordmark section's padding and the strip's vertical breathing
/// room (the 48 px bar around a 20 px line), so the workspace header is
/// centered in its section instead of flush against the sidebar rule.
pub const DESKTOP_TITLEBAR_CONTENT_INSET_PX: f32 = 14.0;
/// Expanded sidebar width.
///
/// Also the width of the thread screen's right inspector column, at every
/// sidebar state, so the two ruled columns read as a pair.
pub const DESKTOP_SIDEBAR_WIDTH_PX: f32 = 327.0;
/// Padding inside a ruled desktop column (the left sidebar and the thread
/// screen's right inspector column share it).
///
/// Rows and section labels add their own 8 px inset
/// ([`DESKTOP_COLUMN_ROW_INSET_PX`]) on top, so text in both columns sits
/// 18 px from the column's outer edge.
pub const DESKTOP_COLUMN_INSET_PX: f32 = 10.0;
/// Horizontal inset of rows and section labels inside a ruled desktop column.
pub const DESKTOP_COLUMN_ROW_INSET_PX: f32 = 8.0;
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

    /// Resolve the shell geometry for a window width.
    ///
    /// A collapsed rail keeps its compact width. An expanded sidebar keeps
    /// [`DESKTOP_SIDEBAR_WIDTH_PX`] while the window has room, shrinks with
    /// it so the chat keeps its comfortable width, and hides (zero width)
    /// once even the shrunken column does not fit, leaving the chat alone
    /// ([`desktop_sidebar_pixels`](crate::shell_layout::desktop_sidebar_pixels)).
    #[must_use]
    pub fn for_window(collapsed: bool, window_width: Pixels, scale_factor: f32) -> Self {
        let style = Self::resolve(collapsed, scale_factor);
        if collapsed {
            return style;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the resolved width is bounded by the f32 sidebar constant"
        )]
        let sidebar_width = crate::shell_layout::desktop_sidebar_pixels(
            f64::from(f32::from(window_width)),
            crate::shell_layout::ProseWidth::Balanced,
            f64::from(DESKTOP_SIDEBAR_WIDTH_PX),
        )
        .map_or(0.0, |width| width as f32);
        Self {
            sidebar_width: px(sidebar_width),
            ..style
        }
    }

    /// Whether the sidebar column renders at all; a hidden sidebar leaves the
    /// chat alone in the window.
    #[must_use]
    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_width > px(0.0)
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
///
/// `inspector_width` is the width of the thread screen's right inspector
/// column while it is on screen (`None` when the route shows none). The
/// shell owns every junction of its rules, so it paints a second
/// [`junction_crosshair`] where that column's one-device-pixel left rule
/// meets the titlebar's bottom rule, mirroring the sidebar junction. Unlike
/// the sidebar rule, the inspector rule does not continue up through the
/// titlebar: the header and drag surface run unbroken above the column.
///
/// `brand` owns the leading sidebar section: the `Artisan Editor` wordmark,
/// seated above the sidebar at exactly its width. `header` owns the titlebar's
/// content section, which starts at the sidebar's right edge inset by
/// [`DESKTOP_TITLEBAR_CONTENT_INSET_PX`] and runs toward the caption controls,
/// so the workspace header is anchored to the primary card's left edge rather
/// than the wordmark. The drag surface fills the rest
/// of the content section after the header. `search` is the command menu,
/// mounted without reserving space: it paints nothing in flow at rest and
/// overlays its palette dialog when open.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "the shell frame mounts each region as its own element; bundling them into a struct would obscure which region is which"
)]
#[expect(
    clippy::too_many_lines,
    reason = "one GPUI builder composes the whole desktop frame; extraction would split the shared reactive style resolution"
)]
pub fn desktop_shell(
    theme: DesktopTheme,
    style: DesktopShellStyle,
    brand: AnyElement,
    header: AnyElement,
    search: AnyElement,
    sidebar: AnyElement,
    body: AnyElement,
    inspector_width: Option<Pixels>,
    maximized: bool,
) -> Div {
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

    // The sidebar section reserves exactly the sidebar's width, so the
    // content section starts on the primary card's left edge. The wordmark
    // keeps its clicks; the remaining leading strip still drags the window.
    let sidebar_visible = style.sidebar_visible();
    let brand = div()
        .w(style.sidebar_width)
        .h_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .px(px(14.0))
        .overflow_hidden()
        .debug_selector(|| DESKTOP_TITLEBAR_BRAND_SELECTOR.to_owned())
        .child(brand)
        .child(
            div()
                .flex_1()
                .h_full()
                .window_control_area(WindowControlArea::Drag),
        );

    // The content section carries the workspace header at its leading end,
    // inset from the sidebar rule by the same measure as the wordmark section,
    // and the window drag surface up to the caption controls, the native
    // reading of the reference strip's content region. The right inset keeps
    // the elastic thread name from truncating flush against the controls, and
    // the header itself is not a drag area, so its repository link keeps its
    // click.
    let content = div()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .flex()
                .items_center()
                .pl(px(DESKTOP_TITLEBAR_CONTENT_INSET_PX))
                .pr(px(24.0))
                .overflow_hidden()
                .child(header)
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .window_control_area(WindowControlArea::Drag),
                ),
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
        .children(sidebar_visible.then_some(brand))
        .child(content)
        // The command menu is mounted without reserving a slot: it paints
        // nothing in flow at rest and overlays its palette dialog when open,
        // so the header stays flush against the controls' drag region.
        .child(search)
        .child(controls)
        .children(sidebar_visible.then(|| {
            // One-physical-pixel continuation of the sidebar's right rule
            // above the junction. The sidebar border paints the rightmost
            // logical pixel inside `sidebar_width` ([sw-1, sw], border-box
            // inset), and the junction crosshair centers its vertical arm on
            // x = sw, so a rule at [sw-1dp, sw] extends exactly that line
            // through the full header height in the same `theme.line` paint.
            // Absolute, so the drag/command-menu/control flex layout is
            // untouched; a plain element with no pointer listener, so like the
            // crosshair it cannot intercept drags or clicks.
            div()
                .absolute()
                .left(style.sidebar_width - style.one_device_pixel)
                .top(px(0.0))
                .w(style.one_device_pixel)
                .h(style.titlebar_height)
                .bg(theme.line)
                .debug_selector(|| "artisan-desktop-titlebar-divider".to_owned())
        }));

    let main = div()
        .relative()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex()
        .flex_row()
        .children(sidebar_visible.then(|| {
            div()
                .w(style.sidebar_width)
                .h_full()
                .flex_shrink_0()
                .border_r_1()
                .border_color(theme.line)
                .debug_selector(|| DESKTOP_SIDEBAR_SELECTOR.to_string())
                .child(sidebar)
        }))
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

    // Right junction: the inspector column is flush with the window's right
    // edge, so its left rule sits at `window − inspector_width`. The mark is
    // anchored from the right (left inset released) with its center on that
    // edge, the mirror of the sidebar junction centered on `sidebar_width`.
    let inspector_junction = inspector_width.map(|inspector_width| {
        junction_crosshair(theme, style.one_device_pixel)
            .left_auto()
            .right(inspector_width - px(DESKTOP_CROSSHAIR_SIZE_PX / 2.0))
            .top(style.titlebar_height - px(DESKTOP_CROSSHAIR_SIZE_PX / 2.0))
            .debug_selector(|| DESKTOP_INSPECTOR_JUNCTION_SELECTOR.to_owned())
    });

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
        .children(sidebar_visible.then(|| {
            junction_crosshair(theme, style.one_device_pixel)
                .left(style.sidebar_width - px(DESKTOP_CROSSHAIR_SIZE_PX / 2.0))
                .top(style.titlebar_height - px(DESKTOP_CROSSHAIR_SIZE_PX / 2.0))
        }))
        .children(inspector_junction)
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

/// Small muted section label heading a group inside a ruled desktop column.
///
/// One treatment for both columns: the left sidebar's thread-age groups
/// ("Last 24 hours") and the right inspector's sections (Context, Checklist,
/// Terminals). 12 px label type in the muted foreground, inset by
/// [`DESKTOP_COLUMN_ROW_INSET_PX`] so it aligns with the row text below it,
/// with one 4 px spacing step before the first row. Plain text: no id, focus
/// handle, or pointer listener.
#[must_use]
pub fn desktop_section_label(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .px(px(DESKTOP_COLUMN_ROW_INSET_PX))
        .pb(theme.spacing.steps(1.0))
        .text_size(theme.typography.label_text)
        .text_color(theme.colors.muted_foreground.to_paint())
        .truncate()
        .child(text.into())
}

/// Make a one-pixel desktop rule without introducing another palette.
#[must_use]
pub fn desktop_rule(theme: DesktopTheme) -> Div {
    div().h(px(1.0)).w_full().bg(theme.line)
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::float_cmp,
        reason = "test assertions compare the exact pixel arithmetic the UI performs; an epsilon would weaken the regression coverage"
    )]
    use super::*;
    use gpui::IntoElement as _;

    #[test]
    fn desktop_shell_keeps_compact_native_geometry() {
        assert_eq!(DESKTOP_TITLEBAR_HEIGHT_PX, 48.0);
        assert_eq!(DESKTOP_TITLEBAR_CONTENT_INSET_PX, 14.0);
        assert_eq!(DESKTOP_SIDEBAR_WIDTH_PX, 327.0);
        assert_eq!(DESKTOP_CROSSHAIR_SIZE_PX, 12.0);

        let expanded = DesktopShellStyle::resolve(false, 1.0);
        let collapsed = DesktopShellStyle::resolve(true, 1.0);
        assert!(collapsed.sidebar_width < expanded.sidebar_width);
        assert_eq!(expanded.one_device_pixel, px(1.0));
    }

    #[test]
    fn sidebar_shrinks_then_hides_leaving_the_chat_alone() {
        let at = |width: f32| DesktopShellStyle::for_window(false, px(width), 1.0);
        assert_eq!(at(1600.0).sidebar_width, px(DESKTOP_SIDEBAR_WIDTH_PX));
        assert_eq!(at(1104.0).sidebar_width, px(240.0));
        assert!(at(1104.0).sidebar_visible());
        assert!(!at(1103.0).sidebar_visible());
        assert!(!at(480.0).sidebar_visible());
    }

    /// Minimal host mounting the production shell with empty regions, so
    /// the junction marks can be measured in a real layout.
    struct ShellJunctionProbe {
        inspector_width: Option<Pixels>,
        collapsed: bool,
    }

    impl gpui::Render for ShellJunctionProbe {
        fn render(
            &mut self,
            window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let empty = || div().into_any_element();
            desktop_shell(
                DesktopTheme::neutral_dark(),
                DesktopShellStyle::resolve(self.collapsed, window.scale_factor()),
                empty(),
                empty(),
                empty(),
                empty(),
                empty(),
                self.inspector_width,
                false,
            )
        }
    }

    /// A visible inspector column gets its own 12 px crosshair centered where
    /// its left rule (`window − inspector_width`) meets the titlebar's bottom
    /// rule, whatever the sidebar state: the column stays 327 px wide even
    /// with the rail collapsed.
    #[gpui::test]
    fn inspector_junction_marks_the_right_rule_on_the_titlebar_rule(cx: &mut gpui::TestAppContext) {
        for collapsed in [false, true] {
            let (_view, cx) = cx.add_window_view(|_, _| ShellJunctionProbe {
                inspector_width: Some(px(DESKTOP_SIDEBAR_WIDTH_PX)),
                collapsed,
            });
            cx.simulate_resize(gpui::size(px(1800.0), px(900.0)));
            cx.run_until_parked();
            let root = cx
                .debug_bounds(DESKTOP_ROOT_SELECTOR)
                .expect("shell root lays out");
            let junction = cx
                .debug_bounds(DESKTOP_INSPECTOR_JUNCTION_SELECTOR)
                .expect("visible inspector gets a junction mark");
            assert_eq!(
                junction.size,
                gpui::size(px(DESKTOP_CROSSHAIR_SIZE_PX), px(DESKTOP_CROSSHAIR_SIZE_PX))
            );
            let rule_x = root.right() - px(DESKTOP_SIDEBAR_WIDTH_PX);
            assert!(
                (junction.center().x - rule_x).abs() < px(0.01),
                "junction {junction:?} must center on the inspector rule at {rule_x:?} (collapsed={collapsed})"
            );
            assert!(
                (junction.center().y - (root.top() + px(DESKTOP_TITLEBAR_HEIGHT_PX))).abs()
                    < px(0.01),
                "junction {junction:?} must center on the titlebar's bottom rule"
            );
        }
    }

    /// No inspector on screen, no right junction: the mark vanishes with the
    /// column instead of floating over the titlebar.
    #[gpui::test]
    fn inspector_junction_disappears_without_an_inspector(cx: &mut gpui::TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, _| ShellJunctionProbe {
            inspector_width: None,
            collapsed: false,
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds(DESKTOP_ROOT_SELECTOR).is_some());
        assert!(
            cx.debug_bounds(DESKTOP_INSPECTOR_JUNCTION_SELECTOR)
                .is_none(),
            "a hidden inspector must not leave its junction mark behind"
        );
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
