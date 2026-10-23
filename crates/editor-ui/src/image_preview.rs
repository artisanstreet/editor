//! Full-window image preview shared by attachments and transcript images.
//! The image fits the window. Escape, the close button, or a click outside
//! the image dismisses the preview and restores focus.

use crate::AssetId;
use crate::attachment_tile::{ACTION_INSET, REMOVE_SIZE};
use crate::button::{Button, ButtonVariant};
use crate::media_frame::MediaFrame;
use crate::overlay as overlays;
use crate::theme::{ArtisanTheme, RadiusTokens};
use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Pixels, RenderImage, Size, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, Window, deferred, div, img, px,
};

/// Largest preview box, before the window bound applies.
const MAX_WIDTH: Pixels = px(960.0);
const MAX_HEIGHT: Pixels = px(720.0);
/// Kept clear between the preview and every window edge.
const WINDOW_MARGIN: Pixels = px(56.0);

/// Shows an image in this window, preserving the opener for dismissal.
pub fn open(image: std::sync::Arc<RenderImage>, window: &mut Window, cx: &mut App) {
    overlays::show_image(image, window, cx);
}

/// A click handler that opens `image` in the preview.
pub fn open_on_click(
    image: std::sync::Arc<RenderImage>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| open(std::sync::Arc::clone(&image), window, cx)
}

/// Closes the preview.
pub fn close(window: &mut Window, cx: &mut App) {
    overlays::dismiss("image-preview", window, cx);
}

/// The picture's size: never upscaled, scaled down to fit the preview box
/// (the window less its margin on each side). Explicit so the close card's
/// container is exactly the picture, never its unscaled width.
#[expect(clippy::cast_precision_loss, reason = "image sides are far below 2^24")]
fn picture_size(image: &RenderImage, viewport: Size<Pixels>) -> Size<Pixels> {
    let fit = |cap: Pixels, extent: Pixels| cap.min(extent - WINDOW_MARGIN * 2.0).max(px(1.0));
    let native = image.size(0);
    let (width, height) = (native.width.0.max(1) as f32, native.height.0.max(1) as f32);
    let scale = (f32::from(fit(MAX_WIDTH, viewport.width)) / width)
        .min(f32::from(fit(MAX_HEIGHT, viewport.height)) / height)
        .min(1.0);
    gpui::size(px(width * scale), px(height * scale))
}

/// The open preview over the whole window, or nothing. Mount it in the
/// window root.
pub fn overlay(theme: &ArtisanTheme, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let (image, focus) = overlays::preview(window, cx)?;
    overlays::retain("image-preview", window, cx);
    let picture = picture_size(&image, window.viewport_size());
    // The attachment tile's remove card, at its inset and nested radius.
    let radius = MediaFrame::attachment_content_radius();
    let inset = px(ACTION_INSET);
    let close_button = div().absolute().top(inset).right(inset).child(
        Button::icon(
            "image-preview-close",
            *theme,
            AssetId::TABLER_X,
            "Close image preview",
        )
        .debug_selector("image-preview-close")
        .variant(ButtonVariant::Ghost)
        .hover_fill()
        .muted()
        .glass()
        .compact(px(REMOVE_SIZE))
        .nested_radius(RadiusTokens::nested(radius, inset))
        .on_activate(|_, window, cx| {
            cx.stop_propagation();
            close(window, cx);
        }),
    );
    let keys = focus.clone();
    let root = div()
        .id("image-preview")
        .debug_selector(|| "image-preview".to_owned())
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme
            .colors
            .background
            .at(crate::theme::Opacity::BackdropDense)
            .to_paint())
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
            div()
                .relative()
                .flex_none()
                .w(picture.width)
                .h(picture.height)
                .child(
                    img(image)
                        .id("image-preview-image")
                        .debug_selector(|| "image-preview-image".to_owned())
                        .size_full()
                        .rounded(radius)
                        .object_fit(ObjectFit::Contain)
                        .on_click(|_, _, cx| cx.stop_propagation()),
                )
                .child(close_button),
        );
    Some(
        deferred(root)
            .with_priority(overlays::priority("image-preview", window, cx))
            .into_any_element(),
    )
}
