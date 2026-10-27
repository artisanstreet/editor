//! Full-window image preview shared by attachments and transcript images.
//! The image fits the window. Escape, the close button, or a click outside
//! the image dismisses the preview and restores focus.

use crate::AssetId;
use crate::button::{Button, ButtonVariant};
use crate::media_frame::MediaFrame;
use crate::motion::{MotionDuration, smooth_out};
use crate::overlay as overlays;
use crate::theme::ArtisanTheme;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, Entity, InteractiveElement as _,
    IntoElement, MouseButton, MouseUpEvent, ObjectFit, ParentElement as _, Pixels, Point,
    RenderImage, Size, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window,
    deferred, div, img, px,
};

/// Largest preview box, before the window bound applies.
const MAX_WIDTH: Pixels = px(960.0);
const MAX_HEIGHT: Pixels = px(720.0);
/// Kept clear between the preview and every window edge.
const WINDOW_MARGIN: Pixels = px(56.0);

#[derive(Default)]
struct Swipe {
    start: Option<Point<Pixels>>,
    offset: Pixels,
    suppress_click: bool,
}

fn release_swipe(state: Entity<Swipe>) -> impl Fn(&MouseUpEvent, &mut Window, &mut App) {
    move |event, window, cx| {
        let direction = state.update(cx, |state, cx| {
            let start = state.start.take()?;
            let delta = event.position - start;
            state.offset = px(0.0);
            state.suppress_click = delta.x.abs() > px(8.0) || delta.y.abs() > px(8.0);
            cx.notify();
            (delta.x.abs() >= px(48.0) && delta.x.abs() > delta.y.abs() * 1.25)
                .then_some(delta.x < px(0.0))
        });
        if let Some(forward) = direction {
            cx.stop_propagation();
            overlays::navigate_image(forward, window, cx);
        }
    }
}

fn navigation_button(
    theme: &ArtisanTheme,
    forward: bool,
    disabled: bool,
) -> impl IntoElement + use<> {
    let (id, icon, label) = if forward {
        (
            "image-preview-next",
            AssetId::TABLER_CHEVRON_RIGHT,
            "Next image",
        )
    } else {
        (
            "image-preview-previous",
            AssetId::TABLER_CHEVRON_LEFT,
            "Previous image",
        )
    };
    let mut position = div().absolute().top(gpui::relative(0.5)).mt(px(-18.0));
    position = if forward {
        position.right(px(8.0))
    } else {
        position.left(px(8.0))
    };
    position
        .id(format!("{id}-container"))
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(
            Button::icon(id, *theme, icon, label)
                .debug_selector(id)
                .variant(ButtonVariant::Ghost)
                .glass()
                .compact(px(36.0))
                .disabled(disabled)
                .on_activate(move |_, window, cx| {
                    cx.stop_propagation();
                    overlays::navigate_image(forward, window, cx);
                }),
        )
}

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
    overlays::close_preview(window, cx);
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
#[expect(
    clippy::too_many_lines,
    reason = "preview image, navigation and shared dismissal form one overlay"
)]
pub fn overlay(theme: &ArtisanTheme, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let (mut image, focus) = overlays::preview(window, cx)?;
    let gallery = overlays::gallery(window, cx);
    let motion = overlays::preview_motion(window, cx)?;
    if motion.closing && cx.reduce_motion() {
        overlays::dismiss("image-preview", window, cx);
        return None;
    }
    let presence = if cx.reduce_motion() {
        1.0
    } else {
        motion.presence(cx.background_executor().now())
    };
    if !cx.reduce_motion() && (motion.closing || presence < 1.0) {
        window.request_animation_frame();
    }
    let mut failed = false;
    if image.is_none()
        && let Some(gallery) = &gallery
    {
        match (gallery.load)(gallery.index, window, cx) {
            overlays::PreviewLoad::Ready(loaded) => {
                overlays::set_preview_image(std::sync::Arc::clone(&loaded), window, cx);
                image = Some(loaded);
            }
            overlays::PreviewLoad::Loading => {}
            overlays::PreviewLoad::Failed => failed = true,
        }
    }
    overlays::retain("image-preview", window, cx);
    let picture = image.as_ref().map_or_else(
        || {
            gpui::size(
                (window.viewport_size().width - WINDOW_MARGIN * 2.0)
                    .max(px(1.0))
                    .min(px(320.0)),
                px(180.0),
            )
        },
        |image| picture_size(image, window.viewport_size()),
    );
    let swipe = window.use_keyed_state(
        gpui::SharedString::from(format!("image-preview-swipe-{}", motion.serial)),
        cx,
        |_, _| Swipe::default(),
    );
    let offset = swipe.read(cx).offset;
    // The attachment tile's remove card, at its inset and nested radius.
    let radius = MediaFrame::attachment_content_radius();
    let outgoing = overlays::outgoing_image(window, cx).and_then(|(image, since, forward)| {
        let elapsed = cx
            .background_executor()
            .now()
            .saturating_duration_since(since);
        let progress = smooth_out(
            (elapsed.as_secs_f32() / MotionDuration::Fast.as_duration().as_secs_f32()).min(1.0),
        );
        if cx.reduce_motion() || progress >= 1.0 {
            overlays::clear_outgoing(window, cx);
            return None;
        }
        window.request_animation_frame();
        Some(
            img(image)
                .absolute()
                .inset_0()
                .size_full()
                .rounded(radius)
                .object_fit(ObjectFit::Contain)
                .opacity(1.0 - progress)
                .left(px(8.0 * if forward { -1.0 } else { 1.0 } * progress))
                .blur(px(3.0 * progress)),
        )
    });
    let inset = px(Button::CORNER_INSET);
    let close_button = div().absolute().top(inset).right(inset).child(
        Button::corner_close("image-preview-close", *theme, "Close image preview", radius)
            .debug_selector("image-preview-close")
            .on_activate(|_, window, cx| {
                cx.stop_propagation();
                close(window, cx);
            }),
    );
    let keys = focus.clone();
    let pressing = swipe.clone();
    let moving = swipe.clone();
    let clicking = swipe.clone();
    let resetting = swipe.clone();
    let releasing = swipe.clone();
    let can_swipe = gallery.is_some();
    let slide = gallery
        .as_ref()
        .filter(|_| !cx.reduce_motion())
        .map(|gallery| {
            (
                format!("image-preview-slide-{}-{}", motion.serial, gallery.index),
                if gallery.forward { 1.0 } else { -1.0 },
            )
        });
    let root = div()
        .id("image-preview")
        .debug_selector(|| "image-preview".to_owned())
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .opacity(presence)
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
            } else if event.keystroke.key == "left" || event.keystroke.key == "right" {
                cx.stop_propagation();
                overlays::navigate_image(event.keystroke.key == "right", window, cx);
            }
        })
        .on_mouse_move(move |event, _, cx| {
            if event.pressed_button != Some(MouseButton::Left) { return; }
            moving.update(cx, |state, cx| {
                if let Some(start) = state.start {
                    state.offset = event.position.x - start.x;
                    cx.notify();
                }
            });
        })
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            resetting.update(cx, |state, _| {
                if state.start.is_none() { state.suppress_click = false; }
            });
        })
        .capture_any_mouse_up(move |event, window, cx| {
            if event.button == MouseButton::Left {
                release_swipe(releasing.clone())(event, window, cx);
            }
        })
        .on_mouse_up_out(MouseButton::Left, release_swipe(swipe))
        // Anywhere but the picture closes it.
        .on_click(move |_, window, cx| {
            let suppressed = clicking.update(cx, |state, _| std::mem::take(&mut state.suppress_click));
            if !suppressed { close(window, cx); }
        })
        .child(
            div()
                .id("image-preview-stage")
                .relative()
                .left(offset)
                .transform(gpui::ElementTransform::scale(0.04_f32.mul_add(presence, 0.96)))
                .flex_none()
                .w(picture.width)
                .h(picture.height)
                .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                    if !can_swipe || motion.closing { return; }
                    pressing.update(cx, |state, _| {
                        state.start = Some(event.position);
                        state.offset = px(0.0);
                        state.suppress_click = false;
                    });
                })
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(image.map_or_else(
                    || div().size_full().flex().items_center().justify_center()
                        .text_color(theme.colors.text_muted.to_paint())
                        .child(if failed { "Couldn't load image" } else { "Loading image…" })
                        .into_any_element(),
                    |image| {
                        let image = img(image)
                        .id("image-preview-image")
                        .debug_selector(|| "image-preview-image".to_owned())
                        .size_full()
                        .rounded(radius)
                        .object_fit(ObjectFit::Contain);
                        if let Some((id, direction)) = slide {
                            div().size_full().child(image)
                                .with_animation(gpui::ElementId::Name(id.into()),
                                    Animation::new(MotionDuration::Fast.as_duration()).with_easing(smooth_out),
                                    move |image, progress| image.opacity(progress)
                                        .relative().left(px(8.0 * direction * (1.0 - progress)))
                                        .blur(px(3.0 * (1.0 - progress))))
                                .into_any_element()
                        } else { image.into_any_element() }
                    },
                ))
                .children(outgoing)
                .child(close_button),
        )
        .children(overlays::gallery(window, cx).filter(|gallery| gallery.count > 1).map(|gallery| {
            div().absolute().inset_0().children([
                navigation_button(theme, false, gallery.index == 0).into_any_element(),
                navigation_button(theme, true, gallery.index + 1 == gallery.count).into_any_element(),
            ]).child(div().absolute().bottom(px(12.0)).left_0().w_full().text_center()
                .text_color(theme.colors.text_muted.to_paint())
                .child(format!("{} / {}", gallery.index + 1, gallery.count)))
        }));
    Some(
        deferred(root)
            .with_priority(overlays::priority("image-preview", window, cx))
            .into_any_element(),
    )
}
