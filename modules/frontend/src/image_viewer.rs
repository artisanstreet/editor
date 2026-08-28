//! Native full-screen image inspection surface.
//!
//! The viewer is the production-reached leaf described by INVENTORY ┬º6.5.2.
//! Its state is deliberately separate from GPUI: open/close transitions and
//! image-inspection leases can be tested without a window, while the view
//! composes the same state into a real GPUI surface. The caller owns the
//! inspection resource; [`ImageViewerInspectionAction`] only records the
//! retain/release operations that the caller must execute.
//!
//! Pinned GPUI 0.2.2 has no CSS filters, z-index style, or platform
//! accessibility tree. The implementation therefore preserves the audited
//! backdrop blur intent and semantic title metadata as typed values, uses
//! sibling paint order for the audited overlay/content stacking, and makes no
//! claim that the metadata is already exposed to OS accessibility APIs.

use std::time::Duration;

use artisan_assets::AssetId;
use artisan_ui::asset_seam::asset_glyph;
use artisan_ui::motion::{MotionDuration, MotionPlan, MotionPolicy, MotionRecipe};
use artisan_ui::theme::{ArtisanTheme, SurfaceStep, ThemeMode};
use gpui::prelude::{
    InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _,
};
use gpui::{
    Bounds, ClickEvent, Context, FocusHandle, ImageSource, KeyDownEvent, ObjectFit, Pixels,
    SharedString, Size, Window, div, img, point, px, transparent_black,
};

/// Stable inspection selectors used by the native behavior probe.
pub const IMAGE_VIEWER_ROOT_SELECTOR: &str = "artisan-image-viewer-root";
pub const IMAGE_VIEWER_BACKDROP_SELECTOR: &str = "artisan-image-viewer-backdrop";
pub const IMAGE_VIEWER_CONTENT_SELECTOR: &str = "artisan-image-viewer-content";
pub const IMAGE_VIEWER_DISMISS_SELECTOR: &str = "artisan-image-viewer-dismiss";
pub const IMAGE_VIEWER_IMAGE_SELECTOR: &str = "artisan-image-viewer-image";
pub const IMAGE_VIEWER_CLOSE_SELECTOR: &str = "artisan-image-viewer-close";
pub const IMAGE_VIEWER_IMAGE_GROUP: &str = "artisan-image-viewer-image-group";

/// Fallback title used when the inspected attachment has no display name.
pub const IMAGE_VIEWER_TITLE: &str = "Image preview";
/// Fallback alternative text for an image without a display name.
pub const IMAGE_VIEWER_IMAGE_ALT: &str = "Attached image";
/// Label retained by the owned full-size dismiss layer and close control.
pub const IMAGE_VIEWER_CLOSE_LABEL: &str = "Close image preview";
/// The legacy close affordance uses the quick 150 ms transition.
pub const IMAGE_VIEWER_CLOSE_TRANSITION: Duration = MotionDuration::Quick.as_duration();

/// The visual blur requested by the audited viewer contract.
///
/// GPUI 0.2.2 cannot paint backdrop filters, so this is an explicit intent
/// rather than a false claim that a filter is installed in the render tree.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BackdropBlurIntent {
    /// Legacy `backdrop-blur-md` intent.
    Medium,
}

/// The only resource operations a viewer may request from its caller.
///
/// A viewer never reaches into `ImageInspectionStore` itself. The owner drains
/// [`ImageViewerState::take_inspection_actions`] and executes these operations
/// against the inspection resource it owns.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ImageViewerInspectionAction {
    /// One viewer became open and needs one inspection retain.
    Retain,
    /// One viewer closed or was explicitly unmounted and needs one release.
    Release,
}

/// Semantic metadata retained alongside the native surface.
///
/// GPUI's pinned accessibility boundary does not expose an OS accessibility
/// tree. These labels are therefore honest metadata for the eventual platform
/// bridge and for deterministic tests, not an assertion that `aria-*` exists
/// in the native render tree.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ImageViewerSemantics {
    /// Name of the viewer content.
    pub title: SharedString,
    /// Name of the inspected image when a platform bridge is available.
    pub image_alt: SharedString,
    /// Name of the viewer surface.
    pub content_label: SharedString,
    /// Name of both close affordances.
    pub close_label: SharedString,
}

impl ImageViewerSemantics {
    fn from_name(name: Option<&SharedString>) -> Self {
        let named = name.filter(|value| !value.trim().is_empty());
        let title = named.cloned().unwrap_or_else(|| IMAGE_VIEWER_TITLE.into());
        let image_alt = named
            .cloned()
            .unwrap_or_else(|| IMAGE_VIEWER_IMAGE_ALT.into());
        let content_label: SharedString = match named {
            Some(value) => format!("{IMAGE_VIEWER_TITLE}: {value}").into(),
            None => IMAGE_VIEWER_TITLE.into(),
        };

        Self {
            title,
            image_alt,
            content_label,
            close_label: IMAGE_VIEWER_CLOSE_LABEL.into(),
        }
    }
}

/// Pure controlled state for one image viewer.
///
/// The state starts closed and applies the requested initial value as a real
/// transition. That means an initially open viewer emits the same single
/// [`ImageViewerInspectionAction::Retain`] as one opened by its controller.
pub struct ImageViewerState {
    open: bool,
    source: Option<ImageSource>,
    name: Option<SharedString>,
    inspection_held: bool,
    pending_inspection_actions: Vec<ImageViewerInspectionAction>,
    generation: u64,
}

impl ImageViewerState {
    /// Creates a viewer with its controlled initial state.
    #[must_use]
    pub fn new(source: Option<ImageSource>, name: Option<SharedString>, open: bool) -> Self {
        let mut state = Self {
            open: false,
            source,
            name,
            inspection_held: false,
            pending_inspection_actions: Vec::new(),
            generation: 0,
        };
        let _ = state.set_open(open);
        state
    }

    /// Whether the controlled viewer is currently open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// The source passed to GPUI's image element, without decoding it here.
    #[must_use]
    pub fn source(&self) -> Option<&ImageSource> {
        self.source.as_ref()
    }

    /// The caller-provided display name, if any.
    #[must_use]
    pub fn name(&self) -> Option<&SharedString> {
        self.name.as_ref()
    }

    /// Current semantic title, image alternative text, and action labels.
    #[must_use]
    pub fn semantics(&self) -> ImageViewerSemantics {
        ImageViewerSemantics::from_name(self.name.as_ref())
    }

    /// Monotonic state generation, useful for rejecting stale external work.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether a retain has been requested without a matching release.
    #[must_use]
    pub const fn inspection_lease_held(&self) -> bool {
        self.inspection_held
    }

    /// Changes controlled open state and queues at most one lease operation.
    ///
    /// Repeating the same value is a no-op. The method does not execute an
    /// effect; callers must drain and execute the returned actions.
    #[must_use]
    pub fn set_open(&mut self, open: bool) -> bool {
        if self.open == open {
            return false;
        }

        self.open = open;
        self.generation = self.generation.wrapping_add(1);
        if open {
            self.retain_inspection();
        } else {
            self.release_inspection();
        }
        true
    }

    /// Requests dismissal through the same controlled transition as Escape.
    #[must_use]
    pub fn dismiss(&mut self) -> bool {
        self.set_open(false)
    }

    /// Handles an Escape decision in the pure state model.
    #[must_use]
    pub fn handle_escape(&mut self) -> bool {
        self.dismiss()
    }

    /// Replaces the resource source without performing decoding or I/O.
    pub fn set_source(&mut self, source: Option<ImageSource>) {
        self.source = source;
    }

    /// Replaces the display name used by semantic metadata.
    pub fn set_name(&mut self, name: Option<SharedString>) {
        self.name = name;
    }

    /// Returns pending resource operations without executing them.
    #[must_use]
    pub fn pending_inspection_actions(&self) -> &[ImageViewerInspectionAction] {
        &self.pending_inspection_actions
    }

    /// Drains pending resource operations for the owner to execute.
    pub fn take_inspection_actions(&mut self) -> Vec<ImageViewerInspectionAction> {
        std::mem::take(&mut self.pending_inspection_actions)
    }

    /// Performs the explicit finalizer transition for an unmounted viewer.
    ///
    /// Rust `Drop` cannot hand an action back to the caller, so the owner must
    /// call this method before dropping/removing the viewer and then execute
    /// the drained release. It is idempotent even if the viewer is already
    /// closed.
    pub fn release_on_unmount(&mut self) -> bool {
        let changed = self.open || self.inspection_held;
        if changed {
            self.open = false;
            self.generation = self.generation.wrapping_add(1);
            self.release_inspection();
        }
        changed
    }

    fn retain_inspection(&mut self) {
        if !self.inspection_held {
            self.inspection_held = true;
            self.pending_inspection_actions
                .push(ImageViewerInspectionAction::Retain);
        }
    }

    fn release_inspection(&mut self) {
        if self.inspection_held {
            self.inspection_held = false;
            self.pending_inspection_actions
                .push(ImageViewerInspectionAction::Release);
        }
    }
}

/// Full-screen viewer geometry resolved from the window viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageViewerGeometry {
    /// Window/client viewport used by the native root.
    pub viewport: Size<Pixels>,
    /// Native titlebar inset reserved above the viewer content.
    pub titlebar_offset: Pixels,
    /// Legacy `p-8` content inset.
    pub padding: Pixels,
}

impl ImageViewerGeometry {
    /// Creates deterministic geometry inputs.
    #[must_use]
    pub const fn new(viewport: Size<Pixels>, titlebar_offset: Pixels, padding: Pixels) -> Self {
        Self {
            viewport,
            titlebar_offset,
            padding,
        }
    }

    /// Bounds of the viewport-filling content below the titlebar.
    #[must_use]
    pub fn content_bounds(self) -> Bounds<Pixels> {
        let viewport_width = finite_non_negative(self.viewport.width);
        let viewport_height = finite_non_negative(self.viewport.height);
        let titlebar = finite_non_negative(self.titlebar_offset).min(viewport_height);

        Bounds {
            origin: point(Pixels::ZERO, px(titlebar)),
            size: Size {
                width: px(viewport_width),
                height: px(viewport_height - titlebar),
            },
        }
    }

    /// Maximum image box after the audited content padding is applied.
    #[must_use]
    pub fn available_image_size(self) -> Size<Pixels> {
        let content = self.content_bounds();
        let padding = finite_non_negative(self.padding);
        let inset = padding * 2.0;

        Size {
            width: px(non_negative_finite(f32::from(content.size.width) - inset)),
            height: px(non_negative_finite(f32::from(content.size.height) - inset)),
        }
    }

    /// Fits an intrinsic image into the bounded content box without upscaling.
    ///
    /// The returned rectangle is centered in the content area. Invalid or
    /// zero-sized intrinsic dimensions produce a deterministic zero rectangle
    /// at the content center rather than a NaN or an out-of-bounds frame.
    #[must_use]
    pub fn image_bounds(self, intrinsic: Size<Pixels>) -> Bounds<Pixels> {
        let content = self.content_bounds();
        let available = self.available_image_size();
        let image_width = finite_non_negative(intrinsic.width);
        let image_height = finite_non_negative(intrinsic.height);
        let available_width = finite_non_negative(available.width);
        let available_height = finite_non_negative(available.height);

        let scale = if image_width > 0.0 && image_height > 0.0 {
            (available_width / image_width)
                .min(available_height / image_height)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let fitted = Size {
            width: px(image_width * scale),
            height: px(image_height * scale),
        };
        let origin = point(
            content.origin.x + (content.size.width - fitted.width) * 0.5,
            content.origin.y + (content.size.height - fitted.height) * 0.5,
        );

        Bounds {
            origin,
            size: fitted,
        }
    }
}

fn finite_non_negative(value: Pixels) -> f32 {
    non_negative_finite(f32::from(value))
}

fn non_negative_finite(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

/// Theme and motion values for one viewer render.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageViewerStyle {
    /// Exact legacy black backdrop alpha.
    pub backdrop_alpha: f32,
    /// Paint value resolved from the shared S1000 surface token.
    pub backdrop_color: gpui::Hsla,
    /// Retained `backdrop-blur-md` intent.
    pub backdrop_blur: BackdropBlurIntent,
    /// Audited overlay layer number, represented by sibling paint order.
    pub overlay_z_index: i32,
    /// Audited content layer number, represented by sibling paint order.
    pub content_z_index: i32,
    /// Content inset from the legacy `p-8` utility.
    pub content_padding: Pixels,
    /// Reserved native titlebar offset.
    pub titlebar_offset: Pixels,
    /// Close-control square edge.
    pub close_size: Pixels,
    /// Close-control inset from the content's top/right edges.
    pub close_inset: Pixels,
    /// Full/reduced motion decision for the 150 ms close affordance recipe.
    pub close_motion: MotionPlan,
}

impl ImageViewerStyle {
    /// Resolves the viewer from shared theme spacing/surface tokens.
    #[must_use]
    pub fn resolve(
        theme: ArtisanTheme,
        titlebar_offset: Pixels,
        motion_policy: MotionPolicy,
    ) -> Self {
        const BACKDROP_ALPHA: f32 = 0.70;

        Self {
            backdrop_alpha: BACKDROP_ALPHA,
            backdrop_color: theme
                .surfaces
                .value(SurfaceStep::S1000)
                .with_alpha(BACKDROP_ALPHA)
                .to_paint(),
            backdrop_blur: BackdropBlurIntent::Medium,
            overlay_z_index: 50,
            content_z_index: 51,
            content_padding: theme.spacing.steps(8.0),
            titlebar_offset,
            close_size: theme.density.control_sm,
            close_inset: theme.spacing.steps(2.0),
            close_motion: motion_policy.resolve(MotionRecipe::MenuClose),
        }
    }
}

/// Native GPUI full-screen image viewer.
pub struct ImageViewerView {
    state: ImageViewerState,
    theme: ArtisanTheme,
    motion_policy: MotionPolicy,
    titlebar_offset: Option<Pixels>,
    focus_handle: FocusHandle,
    close_focus_handle: FocusHandle,
    restore_focus_handle: Option<FocusHandle>,
}

impl ImageViewerView {
    /// Creates a viewer and focuses its root when it starts open.
    ///
    /// `source` is passed directly to GPUI's `img` element. Decoding and
    /// loading remain GPUI/application responsibilities rather than viewer
    /// behavior.
    #[must_use]
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        source: Option<ImageSource>,
        name: Option<SharedString>,
        theme: ArtisanTheme,
        open: bool,
    ) -> Self {
        let focus_handle = cx.focus_handle().tab_index(0).tab_stop(true);
        let close_focus_handle = cx.focus_handle().tab_index(1).tab_stop(true);
        if open {
            focus_handle.focus(window);
        }

        Self {
            state: ImageViewerState::new(source, name, open),
            theme,
            motion_policy: MotionPolicy::Full,
            titlebar_offset: None,
            focus_handle,
            close_focus_handle,
            restore_focus_handle: None,
        }
    }

    /// Supplies an explicit titlebar inset for a client-decorated host.
    #[must_use]
    pub fn with_titlebar_offset(mut self, titlebar_offset: Pixels) -> Self {
        self.titlebar_offset = Some(titlebar_offset);
        self
    }

    /// Supplies the caller's explicit reduced-motion decision.
    #[must_use]
    pub fn with_motion_policy(mut self, motion_policy: MotionPolicy) -> Self {
        self.motion_policy = motion_policy;
        self
    }

    /// Supplies the focus handle to restore after Escape or close.
    #[must_use]
    pub fn with_restore_focus(mut self, focus_handle: FocusHandle) -> Self {
        self.restore_focus_handle = Some(focus_handle);
        self
    }

    /// The controlled viewer state.
    #[must_use]
    pub fn state(&self) -> &ImageViewerState {
        &self.state
    }

    /// The viewer's tracked root focus handle.
    #[must_use]
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// The viewer's close-control focus handle.
    #[must_use]
    pub fn close_focus_handle(&self) -> &FocusHandle {
        &self.close_focus_handle
    }

    /// Whether the viewer is currently open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.state.is_open()
    }

    /// Semantic metadata retained for the caller/platform bridge.
    #[must_use]
    pub fn semantics(&self) -> ImageViewerSemantics {
        self.state.semantics()
    }

    /// Resolves the current theme/motion recipe for an inspected window.
    #[must_use]
    pub fn style(&self, window: &Window) -> ImageViewerStyle {
        ImageViewerStyle::resolve(
            self.theme,
            self.effective_titlebar_offset(window),
            self.motion_policy,
        )
    }

    /// Applies a controlled open/close value and performs focus movement only.
    /// Inspection actions remain queued for the resource owner.
    #[must_use]
    pub fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.state.set_open(open) {
            return false;
        }

        if open {
            self.focus_handle.focus(window);
        } else if let Some(focus_handle) = &self.restore_focus_handle {
            focus_handle.focus(window);
        }
        cx.notify();
        true
    }

    /// Replaces the source and schedules a repaint without decoding it.
    pub fn set_source(&mut self, source: Option<ImageSource>, cx: &mut Context<Self>) {
        self.state.set_source(source);
        cx.notify();
    }

    /// Replaces the name used by the semantic metadata and image alt intent.
    pub fn set_name(&mut self, name: Option<SharedString>, cx: &mut Context<Self>) {
        self.state.set_name(name);
        cx.notify();
    }

    /// Drains retain/release requests for the caller to execute.
    pub fn take_inspection_actions(&mut self) -> Vec<ImageViewerInspectionAction> {
        self.state.take_inspection_actions()
    }

    /// Performs the explicit finalizer transition before removing the view.
    pub fn release_on_unmount(&mut self) -> bool {
        self.state.release_on_unmount()
    }

    fn effective_titlebar_offset(&self, window: &Window) -> Pixels {
        self.titlebar_offset
            .or_else(|| window.client_inset())
            .unwrap_or(Pixels::ZERO)
    }

    fn handle_dismiss(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.set_open(false, window, cx);
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key.as_str() == "escape" {
            let _ = self.set_open(false, window, cx);
        }
    }

    fn close_button(&self, style: ImageViewerStyle, cx: &mut Context<Self>) -> impl IntoElement {
        let hover_background = match self.theme.mode {
            ThemeMode::Light => self.theme.colors.muted,
            ThemeMode::Dark => self.theme.colors.muted.with_alpha(0.5),
        }
        .to_paint();
        let foreground = self.theme.colors.foreground.to_paint();
        let focus_border = self.theme.colors.ring.to_paint();

        div()
            .id("artisan-image-viewer-close-button")
            .absolute()
            .top(style.close_inset)
            .right(style.close_inset)
            .size(style.close_size)
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(transparent_black())
            .text_color(self.theme.colors.muted_foreground.to_paint())
            .opacity(0.0)
            .group_hover(IMAGE_VIEWER_IMAGE_GROUP, |hover| hover.opacity(1.0))
            .hover(move |hover| {
                hover
                    .opacity(1.0)
                    .bg(hover_background)
                    .text_color(foreground)
            })
            .focus(move |focused| focused.opacity(1.0).border_1().border_color(focus_border))
            .track_focus(&self.close_focus_handle)
            .debug_selector(|| IMAGE_VIEWER_CLOSE_SELECTOR.to_string())
            .on_click(cx.listener(Self::handle_dismiss))
            .child(asset_glyph(AssetId::TABLER_X).size(px(16.0)))
    }
}

impl gpui::Render for ImageViewerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .id(IMAGE_VIEWER_ROOT_SELECTOR)
            .relative()
            .size_full()
            .debug_selector(|| IMAGE_VIEWER_ROOT_SELECTOR.to_string());

        if !self.state.is_open() {
            return root;
        }

        let style = self.style(window);
        let geometry = ImageViewerGeometry::new(
            window.viewport_size(),
            style.titlebar_offset,
            style.content_padding,
        );
        let content_bounds = geometry.content_bounds();
        let available_image_size = geometry.available_image_size();

        let backdrop = div()
            .absolute()
            .left(Pixels::ZERO)
            .top(Pixels::ZERO)
            .right(Pixels::ZERO)
            .bottom(Pixels::ZERO)
            .bg(style.backdrop_color)
            .debug_selector(|| IMAGE_VIEWER_BACKDROP_SELECTOR.to_string());

        let dismiss = div()
            .id(IMAGE_VIEWER_DISMISS_SELECTOR)
            .absolute()
            .left(Pixels::ZERO)
            .top(Pixels::ZERO)
            .right(Pixels::ZERO)
            .bottom(Pixels::ZERO)
            .debug_selector(|| IMAGE_VIEWER_DISMISS_SELECTOR.to_string())
            .on_click(cx.listener(Self::handle_dismiss));

        let mut content = div()
            .absolute()
            .left(Pixels::ZERO)
            .top(content_bounds.origin.y)
            .right(Pixels::ZERO)
            .h(content_bounds.size.height)
            .p(style.content_padding)
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .debug_selector(|| IMAGE_VIEWER_CONTENT_SELECTOR.to_string())
            .child(dismiss);

        if let Some(source) = self.state.source().cloned() {
            content = content.child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .group(IMAGE_VIEWER_IMAGE_GROUP)
                    .max_w(available_image_size.width)
                    .max_h(available_image_size.height)
                    .debug_selector(|| IMAGE_VIEWER_IMAGE_SELECTOR.to_string())
                    .child(
                        img(source)
                            .max_w(available_image_size.width)
                            .max_h(available_image_size.height)
                            .object_fit(ObjectFit::Contain),
                    ),
            );
        }

        content = content.child(self.close_button(style, cx));

        root = root
            .track_focus(&self.focus_handle)
            .tab_group()
            .on_key_down(cx.listener(Self::handle_key_down))
            // Paint order is overlay layer 50, then content layer 51. The
            // values are retained in ImageViewerStyle for inspection because
            // GPUI does not expose a numeric z-index refinement.
            .child(backdrop)
            .child(content);

        root
    }
}
