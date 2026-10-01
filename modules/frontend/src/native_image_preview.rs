//! The full-window image preview overlay.
//!
//! Persisted message images ([`crate::native_message_images`]) and composer
//! draft attachments ([`crate::native_composer`]) open the same modal: a
//! near-opaque backdrop over the whole window, the image fitted to the
//! viewport, a status line beneath it only while the preview is being
//! prepared or has failed, and a close action in the top-right corner. A
//! press anywhere but on the image itself dismisses the preview. Each owner keeps its own decode, fencing, and focus; this module
//! owns only the chrome, so the two previews cannot drift apart.
//!
//! The overlay paints in GPUI's deferred layer at
//! [`IMAGE_PREVIEW_DEFERRED_PRIORITY`], above every other deferred surface
//! (the turn navigator, menus, link previews), because it is modal: nothing
//! of the conversation may paint over it.

#![forbid(unsafe_code)]

use std::{rc::Rc, sync::Arc};

use artisan_assets::AssetId;
use artisan_ui::{asset_seam::asset_glyph, theme::ArtisanTheme};
use gpui::prelude::{
    InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _,
};
use gpui::{
    App, ColorExt as _, Div, ElementId, ImageSource, ObjectFit, Pixels, RenderImage, SharedString,
    Size, Window, div, img, px,
};

/// Deferred paint priority of the preview overlay.
///
/// Higher than every other deferred surface in the application (the turn
/// navigator paints at 2, select menus at 20), so an open preview covers
/// them all.
pub(crate) const IMAGE_PREVIEW_DEFERRED_PRIORITY: usize = 100;

/// Largest full-preview box, before the window bound applies.
const PREVIEW_MAX_WIDTH: Pixels = px(960.0);
const PREVIEW_MAX_HEIGHT: Pixels = px(720.0);
/// Space kept clear between the preview and every window edge (the close
/// action sits in it).
const PREVIEW_WINDOW_MARGIN: Pixels = px(56.0);
/// Height held for the status line and gap beneath the image.
const PREVIEW_CAPTION_ALLOWANCE: Pixels = px(32.0);

/// Stable selectors of one owner's preview elements.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ImagePreviewSelectors {
    /// The click-to-dismiss backdrop.
    pub(crate) backdrop: &'static str,
    /// The centered column: the image and, when present, its status line.
    pub(crate) content: &'static str,
    /// The image itself.
    pub(crate) image: &'static str,
    /// The close action.
    pub(crate) close: &'static str,
}

/// Dismissal callback shared by the backdrop, the close action, and its keys.
pub(crate) type ImagePreviewClose = Rc<dyn Fn(&mut Window, &mut App)>;

/// One open preview, as its owner currently knows it.
pub(crate) struct ImagePreview {
    /// The owner's stable selectors.
    pub(crate) selectors: ImagePreviewSelectors,
    /// The best pixels available: the full preview, else a thumbnail, else
    /// nothing while the first decode runs.
    pub(crate) image: Option<Arc<RenderImage>>,
    /// The preparing or failure state, beneath the image. A ready preview
    /// has none: the picture stands alone, without its file name.
    pub(crate) status: Option<SharedString>,
    /// Closes the preview.
    pub(crate) on_close: ImagePreviewClose,
}

/// Bounds the preview box to the viewport less its margin on each side.
fn preview_box(viewport: Size<Pixels>) -> (Pixels, Pixels) {
    let fit = |cap: Pixels, extent: Pixels| {
        cap.min(extent - PREVIEW_WINDOW_MARGIN * 2.0)
            .max(PREVIEW_CAPTION_ALLOWANCE * 2.0)
    };
    (
        fit(PREVIEW_MAX_WIDTH, viewport.width),
        fit(PREVIEW_MAX_HEIGHT, viewport.height),
    )
}

/// Builds the overlay: backdrop, centered content, and close action.
///
/// The returned element fills its positioned parent and occludes everything
/// beneath it. The owner adds its identity, focus tracking, and key handling,
/// and mounts it in the deferred layer at
/// [`IMAGE_PREVIEW_DEFERRED_PRIORITY`] under a parent that spans the window.
pub(crate) fn image_preview_overlay(
    preview: ImagePreview,
    theme: ArtisanTheme,
    viewport: Size<Pixels>,
) -> Div {
    let ImagePreview {
        selectors,
        image,
        status,
        on_close,
    } = preview;

    let dismiss_close = Rc::clone(&on_close);
    let dismiss = div()
        .id(ElementId::Name(selectors.backdrop.into()))
        .absolute()
        .left(Pixels::ZERO)
        .top(Pixels::ZERO)
        .right(Pixels::ZERO)
        .bottom(Pixels::ZERO)
        .debug_selector(move || selectors.backdrop.to_owned())
        .on_click(move |_, window, app| dismiss_close(window, app));

    // The dialog fits the window: fixed caps alone overflow any viewport
    // smaller than them and push the image off-center.
    let (max_width, max_height) = preview_box(viewport);
    let content_close = Rc::clone(&on_close);
    let mut content = div()
        .id(ElementId::Name(selectors.content.into()))
        .debug_selector(move || selectors.content.to_owned())
        .relative()
        .max_w(max_width)
        .max_h(max_height)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .on_click(move |_, window, app| content_close(window, app));
    if let Some(image) = image {
        content = content.child(
            img(ImageSource::Render(image))
                .id(ElementId::Name(selectors.image.into()))
                // Only the picture itself holds the preview open: a press
                // anywhere else in the dialog, the space around the image
                // included, dismisses it.
                .on_click(|_, _, app| app.stop_propagation())
                .debug_selector(move || selectors.image.to_owned())
                .max_w(max_width)
                .max_h(max_height - PREVIEW_CAPTION_ALLOWANCE)
                .object_fit(ObjectFit::Contain),
        );
    }
    if let Some(status) = status {
        content = content.child(
            div()
                .text_size(theme.typography.label_text)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(status),
        );
    }

    let click_close = Rc::clone(&on_close);
    let close = div()
        .id(ElementId::Name(selectors.close.into()))
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
        .tab_index(0)
        .role(gpui::Role::Button)
        .aria_label("Close image preview")
        .debug_selector(move || selectors.close.to_owned())
        .on_click(move |_, window, app| click_close(window, app))
        .on_key_down(move |event, window, app| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                on_close(window, app);
            }
        })
        .child(asset_glyph(AssetId::TABLER_X).size(px(16.0)));

    div()
        .absolute()
        .left(Pixels::ZERO)
        .top(Pixels::ZERO)
        .right(Pixels::ZERO)
        .bottom(Pixels::ZERO)
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.colors.background.to_paint().opacity(0.97))
        .occlude()
        .child(dismiss)
        .child(content)
        .child(close)
}
