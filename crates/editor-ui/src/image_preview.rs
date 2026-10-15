//! Full-window image preview shared by attachments and transcript images.
//! The image fits the window. Escape, the close button, or a click outside
//! the image dismisses the preview and restores focus.

use crate::overlay as overlays;
use crate::theme::ArtisanTheme;
use crate::{AssetId, asset_seam::asset_glyph};
use gpui::{
    AnyElement, App, ClickEvent, ColorExt as _, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Pixels, RenderImage, Size, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, deferred, div, img, px,
};

/// Largest preview box, before the window bound applies.
const MAX_WIDTH: Pixels = px(960.0);
const MAX_HEIGHT: Pixels = px(720.0);
/// Kept clear between the preview and every window edge (the close action
/// sits in it).
const WINDOW_MARGIN: Pixels = px(56.0);

/// Shows an image in this window, preserving the opener for dismissal.
pub fn open(image: std::sync::Arc<RenderImage>, window: &mut Window, cx: &mut App) {
    overlays::show_image(image, window, cx);
}

/// A click handler that opens `image` in the preview.
pub fn open_on_click(
    image: std::sync::Arc<RenderImage>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| open(image.clone(), window, cx)
}

/// Closes the preview.
pub fn close(window: &mut Window, cx: &mut App) {
    overlays::dismiss("image-preview", window, cx);
}

/// Bounds the preview box to the window less its margin on each side.
fn preview_box(viewport: Size<Pixels>) -> (Pixels, Pixels) {
    let fit = |cap: Pixels, extent: Pixels| cap.min(extent - WINDOW_MARGIN * 2.0).max(px(1.0));
    (
        fit(MAX_WIDTH, viewport.width),
        fit(MAX_HEIGHT, viewport.height),
    )
}

/// The open preview over the whole window, or nothing. Mount it in the
/// window root.
pub fn overlay(theme: &ArtisanTheme, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let (image, focus) = overlays::preview(window, cx)?;
    overlays::retain("image-preview", window, cx);
    let (max_width, max_height) = preview_box(window.viewport_size());
    let close_button = div()
        .id("image-preview-close")
        .debug_selector(|| "image-preview-close".to_owned())
        .absolute()
        .top(px(12.0))
        .right(px(12.0))
        .size(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(theme.colors.card.to_paint().opacity(0.94))
        .text_color(theme.colors.muted_foreground.to_paint())
        .role(gpui::Role::Button)
        .aria_label("Close image preview")
        .focusable()
        .tab_index(0)
        .cursor_pointer()
        .on_click(|_, window, cx| close(window, cx))
        .child(asset_glyph(AssetId::TABLER_X).size(px(16.0)));
    let keys = focus.clone();
    let root = div()
        .id("image-preview")
        .debug_selector(|| "image-preview".to_owned())
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.colors.background.to_paint().opacity(0.97))
        .occlude()
        .track_focus(&focus)
        .role(gpui::Role::Dialog)
        .aria_label("Image preview")
        .tab_group()
        .tab_stop(false)
        .on_key_down(move |event, window, cx| {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                close(window, cx);
            } else if event.keystroke.key == "tab" {
                overlays::cycle_focus(&keys, event.keystroke.modifiers.shift, window, cx);
                cx.stop_propagation();
            }
        })
        // Anywhere but the picture closes it.
        .on_click(|_, window, cx| close(window, cx))
        .child(
            img(image)
                .id("image-preview-image")
                .debug_selector(|| "image-preview-image".to_owned())
                .max_w(max_width).max_h(max_height)
                .object_fit(ObjectFit::Contain)
                .on_click(|_, _, cx| cx.stop_propagation()),
        )
        .child(close_button);
    Some(
        deferred(root)
            .with_priority(overlays::priority("image-preview", window, cx))
            .into_any_element(),
    )
}
