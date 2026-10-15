//! Per-window overlay lifetime, focus restoration and layer order.

use gpui::{App, FocusHandle, Global, RenderImage, SharedString, Window, WindowId};
use std::{collections::HashMap, sync::Arc};

pub const POPOVER_PRIORITY: usize = 20;
pub const MODAL_PRIORITY: usize = 100;

struct Session {
    id: SharedString,
    focus: FocusHandle,
    opener: Option<FocusHandle>,
    image: Option<Arc<RenderImage>>,
}
#[derive(Default)]
struct OverlayHost {
    windows: HashMap<WindowId, Vec<Session>>,
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
        child.opener = removed.opener.clone();
    }
    if stack.is_empty() {
        host.windows.remove(&window_id);
    }
    release_image(removed.image, cx);
    if was_top {
        if let Some(opener) = removed.opener {
            opener.focus(window, cx);
        }
    }
    window.refresh();
}

pub(crate) fn show_image(image: Arc<RenderImage>, window: &mut Window, cx: &mut App) {
    begin("image-preview", window, cx);
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
    window.refresh();
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

pub(crate) fn preview(window: &Window, cx: &App) -> Option<(Arc<RenderImage>, FocusHandle)> {
    let session = cx
        .try_global::<OverlayHost>()?
        .windows
        .get(&window.window_handle().window_id())?
        .iter()
        .find(|session| session.id == "image-preview")?;
    Some((session.image.clone()?, session.focus.clone()))
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
            dismiss(&id, window, cx)
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
