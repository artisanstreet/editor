//! The full-window image preview (master's `native_image_preview`): a
//! near-opaque backdrop, the image fitted to the window, and a close action
//! in the top-right corner. A press anywhere but on the image closes it, as
//! does Escape. Composer attachments and transcript images open the same
//! one, so the two can't drift apart.
//!
//! One preview is open per app at a time. [`open`] shows an image;
//! the window root mounts [`overlay`], which paints nothing while closed.

use crate::AssetId;
use crate::asset_seam::asset_glyph;
use crate::theme::ArtisanTheme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, ColorExt as _, FocusHandle, Global, Image,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, Pixels, Size,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, deferred, div, img, px,
};
use std::sync::Arc;

/// Above every other deferred surface (menus, the turn navigator): the
/// preview is modal.
const PRIORITY: usize = 100;
/// Largest preview box, before the window bound applies.
const MAX_WIDTH: Pixels = px(960.0);
const MAX_HEIGHT: Pixels = px(720.0);
/// Kept clear between the preview and every window edge (the close action
/// sits in it).
const WINDOW_MARGIN: Pixels = px(56.0);

struct OpenPreview {
    image: Option<Arc<Image>>,
    focus: Option<FocusHandle>,
}

impl Global for OpenPreview {}

/// Shows `image` in the preview, focused so Escape closes it.
pub fn open(image: Arc<Image>, window: &mut Window, cx: &mut App) {
    let focus = cx.focus_handle();
    focus.focus(window, cx);
    cx.set_global(OpenPreview {
        image: Some(image),
        focus: Some(focus),
    });
    cx.refresh_windows();
}

/// A click handler that opens `image` in the preview.
pub fn open_on_click(image: Arc<Image>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| open(image.clone(), window, cx)
}

/// Closes the preview.
pub fn close(cx: &mut App) {
    if cx
        .try_global::<OpenPreview>()
        .is_some_and(|open| open.image.is_some())
    {
        cx.set_global(OpenPreview {
            image: None,
            focus: None,
        });
        cx.refresh_windows();
    }
}

/// Bounds the preview box to the window less its margin on each side.
fn preview_box(viewport: Size<Pixels>) -> (Pixels, Pixels) {
    let fit = |cap: Pixels, extent: Pixels| cap.min(extent - WINDOW_MARGIN * 2.0).max(px(64.0));
    (
        fit(MAX_WIDTH, viewport.width),
        fit(MAX_HEIGHT, viewport.height),
    )
}

/// The open preview over the whole window, or nothing. Mount it in the
/// window root.
pub fn overlay(theme: &ArtisanTheme, window: &Window, cx: &App) -> Option<AnyElement> {
    let open = cx.try_global::<OpenPreview>()?;
    let image = open.image.clone()?;
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
        .cursor_pointer()
        .on_click(|_, _, cx| close(cx))
        .child(asset_glyph(AssetId::TABLER_X).size(px(16.0)));
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
        .when_some(open.focus.as_ref(), gpui::InteractiveElement::track_focus)
        .on_key_down(|event, _, cx| {
            if event.keystroke.key == "escape" {
                cx.stop_propagation();
                close(cx);
            }
        })
        // Anywhere but the picture closes it.
        .on_click(|_, _, cx| close(cx))
        .child(
            img(image)
                .id("image-preview-image")
                .debug_selector(|| "image-preview-image".to_owned())
                .max_w(max_width)
                .max_h(max_height)
                .object_fit(ObjectFit::Contain)
                .on_click(|_, _, cx| cx.stop_propagation()),
        )
        .child(close_button);
    Some(deferred(root).with_priority(PRIORITY).into_any_element())
}
