//! Per-window overlay lifetime, focus restoration and layer order.

use crate::motion::{MotionDuration, smooth_out};
use gpui::{App, FocusHandle, Global, RenderImage, SharedString, Window, WindowId};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Copy)]
pub(crate) struct PreviewMotion {
    pub serial: u64,
    pub closing: bool,
    since: std::time::Instant,
    from: f32,
}
impl PreviewMotion {
    pub fn presence(self, now: std::time::Instant) -> f32 {
        let duration = if self.closing {
            MotionDuration::Quick
        } else {
            MotionDuration::Fast
        }
        .as_duration();
        let progress = smooth_out(
            (now.saturating_duration_since(self.since).as_secs_f32() / duration.as_secs_f32())
                .min(1.0),
        );
        let target = if self.closing { 0.0 } else { 1.0 };
        (target - self.from).mul_add(progress, self.from)
    }
}

pub(crate) enum PreviewLoad {
    Ready(Arc<RenderImage>),
    Loading,
    Failed,
}
pub(crate) type GalleryLoader = dyn Fn(usize, &mut Window, &mut App) -> PreviewLoad;

#[derive(Clone)]
pub(crate) struct ImageGallery {
    pub index: usize,
    pub count: usize,
    pub load: std::rc::Rc<GalleryLoader>,
    pub forward: bool,
}

pub const POPOVER_PRIORITY: usize = 20;
pub const MODAL_PRIORITY: usize = 100;

struct Session {
    id: SharedString,
    focus: FocusHandle,
    opener: Option<FocusHandle>,
    image: Option<Arc<RenderImage>>,
    gallery: Option<ImageGallery>,
    preview_motion: Option<PreviewMotion>,
    outgoing: Option<(Arc<RenderImage>, std::time::Instant, bool)>,
}
#[derive(Default)]
struct OverlayHost {
    windows: HashMap<WindowId, Vec<Session>>,
    preview_serial: u64,
}
impl Global for OverlayHost {}

fn initialize(cx: &mut App) {
    if cx.try_global::<OverlayHost>().is_some() {
        return;
    }
    cx.set_global(OverlayHost::default());
    cx.on_window_closed(|cx, id| {
        if let Some(sessions) = cx.global_mut::<OverlayHost>().windows.remove(&id) {
            for session in sessions {
                release_image(session.image, cx);
                release_image(session.outgoing.map(|(image, _, _)| image), cx);
            }
        }
    })
    .detach();
}

/// Capture the opener once. Calling again for the mounted overlay preserves its focus.
pub fn begin(id: impl Into<SharedString>, window: &mut Window, cx: &mut App) -> FocusHandle {
    initialize(cx);
    let id = id.into();
    let window_id = window.window_handle().window_id();
    if let Some(session) = cx
        .global::<OverlayHost>()
        .windows
        .get(&window_id)
        .and_then(|stack| stack.iter().find(|session| session.id == id))
    {
        return session.focus.clone();
    }
    let opener = window.focused(cx);
    let focus = cx.focus_handle();
    cx.global_mut::<OverlayHost>()
        .windows
        .entry(window_id)
        .or_default()
        .push(Session {
            id,
            focus: focus.clone(),
            opener,
            image: None,
            gallery: None,
            preview_motion: None,
            outgoing: None,
        });
    focus.focus(window, cx);
    focus
}

/// Remove an overlay and restore its actual opener when it was the top layer.
pub fn dismiss(id: &str, window: &mut Window, cx: &mut App) {
    let window_id = window.window_handle().window_id();
    if cx.try_global::<OverlayHost>().is_none() {
        return;
    }
    let host = cx.global_mut::<OverlayHost>();
    let Some(stack) = host.windows.get_mut(&window_id) else {
        return;
    };
    let Some(index) = stack.iter().position(|session| session.id == id) else {
        return;
    };
    let was_top = index + 1 == stack.len();
    let removed = stack.remove(index);
    // Closing a parent under a child must not leave the child restoring dead parent focus.
    if let Some(child) = stack.get_mut(index) {
        child.opener.clone_from(&removed.opener);
    }
    if stack.is_empty() {
        host.windows.remove(&window_id);
    }
    release_image(removed.image, cx);
    release_image(removed.outgoing.map(|(image, _, _)| image), cx);
    if was_top && let Some(opener) = removed.opener {
        opener.focus(window, cx);
    }
    window.refresh();
}

pub(crate) fn show_image(image: Arc<RenderImage>, window: &mut Window, cx: &mut App) {
    begin("image-preview", window, cx);
    let since = cx.background_executor().now();
    let host = cx.global_mut::<OverlayHost>();
    host.preview_serial = host.preview_serial.wrapping_add(1);
    let serial = host.preview_serial;
    update_preview(window, cx, |session| {
        session.gallery = None;
        session.preview_motion = Some(PreviewMotion {
            serial,
            closing: false,
            since,
            from: 0.0,
        });
    });
    clear_outgoing(window, cx);
    set_preview_image(image, window, cx);
    window.refresh();
}

pub(crate) fn preview_motion(window: &Window, cx: &App) -> Option<PreviewMotion> {
    cx.try_global::<OverlayHost>()?
        .windows
        .get(&window.window_handle().window_id())?
        .iter()
        .find(|session| session.id == "image-preview")?
        .preview_motion
}

pub(crate) fn close_preview(window: &mut Window, cx: &mut App) {
    if cx.reduce_motion() {
        dismiss("image-preview", window, cx);
        return;
    }
    let now = cx.background_executor().now();
    let serial = update_preview(window, cx, |session| {
        let motion = session.preview_motion.as_mut()?;
        if motion.closing {
            return None;
        }
        motion.from = motion.presence(now);
        motion.since = now;
        motion.closing = true;
        Some(motion.serial)
    })
    .flatten();
    let Some(serial) = serial else {
        return;
    };
    window.refresh();
    let timer = cx
        .background_executor()
        .timer(MotionDuration::Quick.as_duration());
    window
        .spawn(cx, async move |cx| {
            timer.await;
            cx.update(|window, cx| {
                if preview_motion(window, cx)
                    .is_some_and(|motion| motion.serial == serial && motion.closing)
                {
                    dismiss("image-preview", window, cx);
                }
            })
            .ok();
        })
        .detach();
}

fn update_preview<R>(
    window: &Window,
    cx: &mut App,
    update: impl FnOnce(&mut Session) -> R,
) -> Option<R> {
    cx.try_global::<OverlayHost>()?;
    cx.global_mut::<OverlayHost>()
        .windows
        .get_mut(&window.window_handle().window_id())?
        .iter_mut()
        .find(|session| session.id == "image-preview")
        .map(update)
}

pub(crate) fn show_gallery(
    image: Arc<RenderImage>,
    gallery: ImageGallery,
    window: &mut Window,
    cx: &mut App,
) {
    show_image(image, window, cx);
    update_preview(window, cx, |session| session.gallery = Some(gallery));
}

pub(crate) fn gallery(window: &Window, cx: &App) -> Option<ImageGallery> {
    cx.try_global::<OverlayHost>()?
        .windows
        .get(&window.window_handle().window_id())?
        .iter()
        .find(|session| session.id == "image-preview")?
        .gallery
        .clone()
}

pub(crate) fn navigate_image(forward: bool, window: &mut Window, cx: &mut App) {
    let now = cx.background_executor().now();
    let reduced = cx.reduce_motion();
    if reduced {
        clear_outgoing(window, cx);
    }
    let previous = update_preview(window, cx, |session| {
        if session.preview_motion.is_some_and(|motion| motion.closing) {
            return None;
        }
        let gallery = session.gallery.as_mut()?;
        let index = if forward {
            gallery.index.saturating_add(1).min(gallery.count - 1)
        } else {
            gallery.index.saturating_sub(1)
        };
        if index == gallery.index {
            return None;
        }
        gallery.index = index;
        gallery.forward = forward;
        let previous = session.outgoing.take().map(|(image, _, _)| image);
        if reduced {
            return session.image.take();
        }
        session.outgoing = session.image.take().map(|image| (image, now, forward));
        previous
    })
    .flatten();
    release_image(previous, cx);
    window.refresh();
}

pub(crate) fn outgoing_image(
    window: &Window,
    cx: &App,
) -> Option<(Arc<RenderImage>, std::time::Instant, bool)> {
    cx.try_global::<OverlayHost>()?
        .windows
        .get(&window.window_handle().window_id())?
        .iter()
        .find(|session| session.id == "image-preview")?
        .outgoing
        .clone()
}

pub(crate) fn clear_outgoing(window: &Window, cx: &mut App) {
    let image = update_preview(window, cx, |session| {
        session.outgoing.take().map(|(image, _, _)| image)
    })
    .flatten();
    release_image(image, cx);
}

pub(crate) fn set_preview_image(image: Arc<RenderImage>, window: &Window, cx: &mut App) {
    let previous = cx
        .global_mut::<OverlayHost>()
        .windows
        .get_mut(&window.window_handle().window_id())
        .and_then(|stack| {
            stack
                .iter_mut()
                .find(|session| session.id == "image-preview")
        })
        .and_then(|session| session.image.replace(image));
    release_image(previous, cx);
}

fn release_image(image: Option<Arc<RenderImage>>, cx: &mut App) {
    if let Some(image) = image {
        // Another visible owner may upload the image again. Each owner releases
        // its own last use, including a preview opened after a cache eviction.
        cx.defer(move |cx| {
            cx.drop_image(image, None);
            // Other visible owners can retain these pixels after this owner
            // releases them. Invalidate cached scenes before they reuse a tile.
            cx.refresh_windows();
        });
    }
}

pub(crate) fn preview(
    window: &Window,
    cx: &App,
) -> Option<(Option<Arc<RenderImage>>, FocusHandle)> {
    let session = cx
        .try_global::<OverlayHost>()?
        .windows
        .get(&window.window_handle().window_id())?
        .iter()
        .find(|session| session.id == "image-preview")?;
    Some((session.image.clone(), session.focus.clone()))
}

/// Modal layers follow the window's overlay stack; popovers have a lower shared tier.
pub fn priority(id: &str, window: &Window, cx: &App) -> usize {
    let index = cx
        .try_global::<OverlayHost>()
        .and_then(|host| host.windows.get(&window.window_handle().window_id()))
        .and_then(|stack| stack.iter().position(|entry| entry.id == id))
        .unwrap_or(0);
    MODAL_PRIORITY + index
}

/// Keyboard traversal stays within the active overlay's focus group.
pub fn cycle_focus(group: &FocusHandle, reverse: bool, window: &mut Window, cx: &mut App) {
    if reverse {
        window.focus_prev(cx);
    } else {
        window.focus_next(cx);
    }
    if group.contains_focused(window, cx) {
        return;
    }
    group.focus(window, cx);
    window.focus_next(cx);
    if reverse {
        let first = window.focused(cx);
        let mut last = first.clone();
        loop {
            window.focus_next(cx);
            let next = window.focused(cx);
            if !group.contains_focused(window, cx) || next == first || next == last {
                break;
            }
            last = next;
        }
        if let Some(last) = last {
            last.focus(window, cx);
        }
    }
}

/// Tie registration to the mounted component. Success handlers and navigation
/// can remove an overlay without invoking its explicit dismissal callback.
pub fn retain(id: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    let id = id.into();
    let lease = window.use_keyed_state(
        SharedString::from(format!("overlay-lease-{id}")),
        cx,
        |_, _| false,
    );
    if !*lease.read(cx) {
        lease.update(cx, |registered, _| *registered = true);
        cx.observe_release_in(&lease, window, move |_, window, cx| {
            dismiss(&id, window, cx);
        })
        .detach();
    }
}

/// Nonmodal child menus appear above the currently mounted modal stack.
pub fn popup_priority(window: &Window, cx: &App) -> usize {
    cx.try_global::<OverlayHost>()
        .and_then(|host| host.windows.get(&window.window_handle().window_id()))
        .map_or(POPOVER_PRIORITY, |stack| MODAL_PRIORITY + stack.len())
}
