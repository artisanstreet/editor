//! The model selector (`native_model_selector`) and the card's project tab
//! (`home_project_picker`, `render_tab`), in the old Editor's look: the
//! trigger with the engine glyph, the model name and its muted details; the
//! glass menu with the engine tabs, the model list beside its preview and
//! policy controls; and the glass dropdowns those controls open.

use crate::{Composer, Menu};
use artisan_api::ProjectId;
use artisan_api::{
    EngineConfig, EngineId, EngineStatus, InstallState, Model, Permissions, SignInState, Speed,
};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::glass::{
    GlassStrength, card_shadows, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use editor_ui::gradient::{hover_fill_gradient, vertical_gradient};
use editor_ui::picker_motion::{
    HoverRect, MenuEntrance, MenuMotion, MenuPhase, SlidingHoverState, animate_menu,
    sliding_hover_pill, smooth_out,
};
use editor_ui::theme::{ArtisanTheme, SurfaceStep, WorkspaceColors};
use gpui::uniform_list;
use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, Bounds, Context, ElementId, FontWeight,
    HighlightStyle, ImageSource, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderImage, SharedString, StatefulInteractiveElement as _, Styled as _, StyledText, Task,
    anchored, canvas, deferred, div, img, point, prelude::FluentBuilder as _, px,
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

const MENU_WIDTH_PX: f32 = 480.0;
const MENU_MAX_HEIGHT_PX: f32 = 320.0;
const MENU_VIEWPORT_INSET_X_PX: f32 = 32.0;
const MENU_GAP_PX: f32 = 8.0;
const DROPDOWN_GAP_PX: f32 = 4.0;
const DROPDOWN_MIN_WIDTH_PX: f32 = 144.0;
const MODEL_PANEL_HEIGHT_PX: f32 = 192.0;
const MODEL_PREVIEW_WIDTH_PX: f32 = 224.0;
const MODEL_ROW_HEIGHT_PX: f32 = 48.0;
const COMPACT_CONTROL_HEIGHT_PX: f32 = 32.0;
const POLICY_CONTROL_HEIGHT_PX: f32 = 24.0;
/// Open and close of the model menu and its dropdowns: a short slide and
/// fade (`PICKER_MENU_MOTION_DURATION_MS`).
const MENU_MOTION: Duration = Duration::from_millis(100);
/// A tooltip shows once the pointer rests this long.
const TOOLTIP_DELAY: Duration = Duration::from_millis(500);
const OPTION_TOOLTIP_WIDTH_PX: f32 = 320.0;
const OPTION_TOOLTIP_GAP_PX: f32 = 8.0;
const ENGINE_TOOLTIP_WIDTH_PX: f32 = 240.0;
/// The project menu (`home_project_picker`).
const PROJECT_MENU_WIDTH_PX: f32 = 320.0;
/// The hover id of the project menu's "New project" row.
const NEW_PROJECT_ROW: &str = "new-project";
const PROJECT_MENU_GAP_PX: f32 = 10.0;
const PROJECT_MENU_RADIUS_PX: f32 = 10.0;
const PROJECT_CONTROL_RADIUS_PX: f32 = 6.0;
/// The engine light under the active engine tab.
const ENGINE_LIGHT_WIDTH_PX: f32 = 32.0;
const ENGINE_LIGHT_HEIGHT_PX: f32 = 24.0;
const ENGINE_LIGHT_RASTER_SCALE: u32 = 4;
const ENGINE_LIGHT_RASTER_WIDTH: u32 = 128;
const ENGINE_LIGHT_RASTER_HEIGHT: u32 = 96;
/// The light's slide between tabs.
const ENGINE_LIGHT_FLIGHT: Duration = Duration::from_millis(250);
/// The Editor-local setting holding favourite model ids per engine.
const FAVORITES_KEY: &str = "model_favorites";

fn engine_icon(engine: EngineId) -> AssetId {
    match engine {
        EngineId::Claude => AssetId::SVGL_CLAUDE_AI,
        EngineId::Codex => AssetId::SVGL_OPENAI,
        EngineId::OpenCode => AssetId::BRANDS_OPENCODE,
        EngineId::Grok => AssetId::SVGL_GROK,
        EngineId::Cursor => AssetId::SVGL_CURSOR,
    }
}

/// The lab line under a model's name.
const fn engine_lab(engine: EngineId) -> &'static str {
    match engine {
        EngineId::Claude => "Anthropic",
        EngineId::Codex => "OpenAI",
        EngineId::OpenCode => "OpenCode",
        EngineId::Grok => "xAI",
        EngineId::Cursor => "Cursor",
    }
}

/// A context window as the selector names it: `400K`, `1M`.
fn context_label(tokens: u64) -> String {
    if tokens >= 1_000_000 && tokens.is_multiple_of(1_000_000) {
        format!("{}M", tokens / 1_000_000)
    } else {
        format!("{}K", tokens / 1_000)
    }
}

/// An effort as the selector names it: `medium` reads `Medium`.
fn humanize(word: &str) -> String {
    if word == "xhigh" {
        return "Extra High".to_owned();
    }
    let mut out = String::with_capacity(word.len());
    for (index, part) in word.split(['_', '-']).enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            if index == 0 {
                out.extend(first.to_uppercase());
            } else {
                out.push(first);
            }
            out.push_str(chars.as_str());
        }
    }
    out
}

/// What stands between the user and an engine's models, in the order the
/// user must deal with it: install, then sign in. Only a signed-in engine
/// with no models says so.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Gate {
    NotInstalled,
    SignIn,
    SigningIn,
    Checking,
    NoModels,
}

impl Gate {
    fn of(status: &EngineStatus) -> Option<Self> {
        if !matches!(status.install, InstallState::Installed { .. }) {
            return Some(Self::NotInstalled);
        }
        match &status.sign_in {
            SignInState::SignedOut | SignInState::Failed { .. } => Some(Self::SignIn),
            SignInState::LoggingIn { .. } => Some(Self::SigningIn),
            SignInState::Unknown if status.models.is_empty() => Some(Self::Checking),
            SignInState::SignedIn { .. } if status.models.is_empty() => Some(Self::NoModels),
            _ => None,
        }
    }

    /// Why the engine's tab is greyed (`engine_unavailable_reason`).
    fn reason(self, engine: EngineId) -> String {
        let label = engine.display_name();
        match self {
            Self::NotInstalled => format!("{label} isn't installed on this Forge."),
            Self::SignIn => format!("Sign in to {label} to use its models."),
            Self::SigningIn => format!("Signing in to {label}…"),
            Self::Checking => format!("Checking {label}…"),
            Self::NoModels => format!("{label} hasn't reported any models."),
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::NotInstalled => "Not installed",
            Self::SignIn => "Sign in",
            Self::SigningIn => "Signing in…",
            Self::Checking => "Checking…",
            Self::NoModels => "No models",
        }
    }
}

/// One of the model's policy controls in the preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    /// Another variant of the model (OpenCode `model.variant`).
    Variant,
    Thinking,
    Speed,
    Context,
    Permission,
}

impl Axis {
    /// The controls in the preview, top to bottom (`render_policy_controls`).
    const ALL: [Self; 5] = [
        Self::Variant,
        Self::Thinking,
        Self::Speed,
        Self::Context,
        Self::Permission,
    ];

    const fn title(self) -> &'static str {
        match self {
            Self::Variant => "Variants",
            Self::Thinking => "Efforts",
            Self::Speed => "Speed",
            Self::Context => "Context Window",
            Self::Permission => "Permissions",
        }
    }

    const fn icon(self) -> AssetId {
        match self {
            Self::Variant | Self::Thinking => AssetId::TABLER_BRAIN,
            Self::Speed => AssetId::TABLER_BOLT_FILLED,
            Self::Context => AssetId::TABLER_ARROWS_HORIZONTAL,
            Self::Permission => AssetId::TABLER_LOCK,
        }
    }

    /// Whether the preview shows the control: Thinking with any option,
    /// the rest only with a choice to make.
    const fn shown(self, options: usize) -> bool {
        match self {
            Self::Thinking | Self::Context => options > 0,
            Self::Variant | Self::Speed | Self::Permission => options > 1,
        }
    }
}

/// One option of a policy dropdown.
struct AxisOption {
    label: String,
    /// Metadata marks modes that belong below the regular efforts.
    special: bool,
    /// What the option does, shown as its tooltip.
    description: Option<String>,
    /// A caution shown ahead of the description, in the warning colour.
    advisory: Option<String>,
    selected: bool,
    apply: EngineConfig,
}

/// Where a picker tooltip sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TooltipPlacement {
    /// Beside the row, on whichever side has room (policy options).
    Side,
    /// Centred above the element (engine tabs).
    Above,
}

/// The tooltip waiting for its delay or shown.
#[derive(Clone, Debug)]
struct Tooltip {
    key: String,
    /// Painted ahead of `text` in the warning colour.
    advisory: Option<String>,
    text: String,
    placement: TooltipPlacement,
    bounds: Option<Bounds<Pixels>>,
    visible: bool,
}

/// The engine light's slide from one tab to another.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LightFlight {
    from_left: f32,
    to_left: f32,
    generation: u64,
}

/// The light under the active engine tab.
#[derive(Debug, Default)]
struct EngineLight {
    lit: Option<EngineId>,
    left: f32,
    width: f32,
    flight: Option<LightFlight>,
    generation: u64,
}

impl EngineLight {
    /// Records the active tab's place in its strip: a new tab slides the
    /// light from the old one; a resize moves it at once.
    #[expect(
        clippy::float_cmp,
        reason = "the light re-measures only when the measured place changes exactly"
    )]
    fn measure(&mut self, engine: EngineId, left: f32, width: f32) -> bool {
        let moved = self.lit != Some(engine);
        if !moved && self.left == left && self.width == width {
            return false;
        }
        self.flight = if moved && self.lit.is_some() {
            self.generation = self.generation.wrapping_add(1);
            Some(LightFlight {
                from_left: light_left(self.left, self.width),
                to_left: light_left(left, width),
                generation: self.generation,
            })
        } else {
            None
        };
        self.lit = Some(engine);
        self.left = left;
        self.width = width;
        true
    }
}

/// The light's left edge, centred under a tab.
fn light_left(tab_left: f32, tab_width: f32) -> f32 {
    tab_left + ((tab_width - ENGINE_LIGHT_WIDTH_PX) / 2.0).max(0.0)
}

/// The picker surfaces' retained motion: which menu and dropdown are
/// painted (open, or playing their exit), the shared hover pills, the
/// engine light and the delayed tooltip.
#[derive(Default)]
pub(crate) struct PickerState {
    models: Vec<(String, usize)>,
    projects: Vec<ProjectId>,
    favorites: HashSet<String>,
    shown: Option<Menu>,
    menu: Rc<RefCell<MenuMotion>>,
    menu_settle: Option<Task<()>>,
    shown_axis: Option<Axis>,
    pending_axis: Option<Axis>,
    axis: Rc<RefCell<MenuMotion>>,
    axis_settle: Option<Task<()>>,
    axis_focus: Option<gpui::FocusHandle>,
    axis_highlight: Option<usize>,
    pub(super) model_scroll: gpui::UniformListScrollHandle,
    pub(super) model_glide: editor_ui::wheel_glide::WheelGlide,
    model_glide_scheduled: bool,
    pub(super) model_hover: Rc<RefCell<SlidingHoverState>>,
    model_surface: Rc<RefCell<Option<Bounds<Pixels>>>>,
    axis_hover: Rc<RefCell<SlidingHoverState>>,
    axis_surface: Rc<RefCell<Option<Bounds<Pixels>>>>,
    project_hover: Rc<RefCell<SlidingHoverState>>,
    project_surface: Rc<RefCell<Option<Bounds<Pixels>>>>,
    light: Rc<RefCell<EngineLight>>,
    tabs_surface: Rc<RefCell<Option<Bounds<Pixels>>>>,
    tooltip: Rc<RefCell<Option<Tooltip>>>,
    tooltip_generation: u64,
    tooltip_task: Option<Task<()>>,
}

/// A probe recording a surface's bounds, refreshing when they move.
fn surface_probe(bounds: &Rc<RefCell<Option<Bounds<Pixels>>>>) -> impl IntoElement {
    let surface = Rc::clone(bounds);
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            let changed = surface.borrow_mut().replace(bounds) != Some(bounds);
            if changed {
                window.defer(cx, |window, _| window.refresh());
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A probe measuring one row into its surface's hover pill.
fn row_probe(
    id: String,
    hover: &Rc<RefCell<SlidingHoverState>>,
    surface: &Rc<RefCell<Option<Bounds<Pixels>>>>,
) -> impl IntoElement {
    let (hover, surface) = (Rc::clone(hover), Rc::clone(surface));
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            let Some(surface) = *surface.borrow() else {
                return;
            };
            if hover
                .borrow_mut()
                .measure(&id, HoverRect::within(bounds, surface))
            {
                window.defer(cx, |window, _| window.refresh());
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// A probe recording an element's bounds into the tooltip keyed `key`.
fn tooltip_probe(key: String, tooltip: &Rc<RefCell<Option<Tooltip>>>) -> impl IntoElement {
    let tooltip = Rc::clone(tooltip);
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            let changed = tooltip.borrow_mut().as_mut().is_some_and(|target| {
                target.key == key && target.bounds.replace(bounds) != Some(bounds)
            });
            if changed {
                window.defer(cx, |window, _| window.refresh());
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The selector's control gradient (`source_control_gradient`).
fn control_gradient() -> gpui::Background {
    vertical_gradient(SurfaceStep::S800.oklch(), SurfaceStep::S925.oklch())
}

/// A strong-glass surface of `radius`.
pub(super) fn glass_panel(
    id: impl Into<SharedString>,
    theme: &ArtisanTheme,
    radius: f32,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .occlude()
        .relative()
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(radius))
        .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
        .bg(glass_foreground_base(theme))
        .text_color(theme.colors.foreground.to_paint())
        .shadow(glass_card_shadows())
        .child(glass_material_layer(GlassStrength::Strong, px(radius)))
        .child(glass_highlight_layer(GlassStrength::Strong, px(radius)))
}

/// One dropdown row: label, and the check on the chosen one.
fn option_row(
    id: SharedString,
    label: impl Into<SharedString>,
    chosen: bool,
    theme: &ArtisanTheme,
) -> gpui::Stateful<gpui::Div> {
    let theme = *theme;
    let label = label.into();
    let mut row = div()
        .id(id)
        .role(gpui::Role::MenuItem)
        .aria_label(label.clone())
        .aria_selected(chosen)
        .focusable()
        .tab_index(0)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .gap(px(10.0))
        .pl(px(12.0))
        .pr(px(32.0))
        .py(px(8.0))
        .rounded(px(14.0))
        .text_size(px(14.0))
        .line_height(px(20.0))
        .cursor_pointer()
        .child(div().flex_1().min_w(px(0.0)).truncate().child(label));
    if chosen {
        row = row.child(
            div()
                .absolute()
                .right(px(8.0))
                .top_0()
                .bottom(px(0.0))
                .w(px(14.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    asset_glyph(AssetId::TABLER_CHECK)
                        .size(px(14.0))
                        .text_color(theme.colors.muted_foreground.to_paint()),
                ),
        );
    }
    row
}

impl Composer {
    // Opening and closing.

    /// Opens a picker menu: the model menu slides in; the project menu
    /// appears in place.
    fn reset_model_scroll(&mut self) {
        self.picker.model_glide.cancel();
        self.picker.model_hover.borrow_mut().clear();
        self.picker
            .model_scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(0.0)));
    }

    fn model_wheel(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let handle = self.picker.model_scroll.0.borrow().base_handle.clone();
        let max = f32::from(handle.max_offset().y).max(0.0);
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if max <= 0.0 || delta.abs() <= f32::EPSILON {
            return;
        }
        cx.stop_propagation();
        let offset = handle.offset();
        let current = f32::from(offset.y);
        if event.delta.precise() || cx.reduce_motion() {
            self.picker.model_glide.cancel();
            handle.set_offset(point(offset.x, px((current + delta).clamp(-max, 0.0))));
            cx.notify();
            return;
        }
        self.picker.model_glide.push(current, delta, max);
        self.schedule_model_glide(window, cx);
    }

    fn schedule_model_glide(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if !self.picker.model_glide.active() || self.picker.model_glide_scheduled {
            return;
        }
        self.picker.model_glide_scheduled = true;
        cx.on_next_frame(window, |this, window, cx| {
            this.picker.model_glide_scheduled = false;
            if this.menu != Some(Menu::Model) {
                this.picker.model_glide.cancel();
                return;
            }
            let handle = this.picker.model_scroll.0.borrow().base_handle.clone();
            let offset = handle.offset();
            let max = f32::from(handle.max_offset().y).max(0.0);
            if let Some(next) = this.picker.model_glide.step(f32::from(offset.y), max) {
                handle.set_offset(point(offset.x, px(next)));
                cx.notify();
            }
            this.schedule_model_glide(window, cx);
        });
    }

    pub(crate) fn open_menu(&mut self, menu: Menu, cx: &mut Context<Self>) {
        if !self.metadata_ready(cx) || !self.draft_editable(cx) {
            return;
        }
        self.close_axis_now();
        self.clear_tooltip();
        self.menu = Some(menu);
        self.picker.shown = Some(menu);
        if menu == Menu::Model {
            self.reset_model_scroll();
            self.browsing = self.effective_config(cx).map(|config| config.engine);
            let generation = self.picker.menu.borrow_mut().begin_open();
            self.settle_menu(generation, true, cx);
        }
        self.refresh_picker_catalog(cx);
        cx.notify();
    }

    /// Closes the open menu; the model menu stays painted for its exit.
    pub(crate) fn close_menu(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        self.close_axis_now();
        self.clear_tooltip();
        self.picker.model_glide.cancel();
        self.picker.model_hover.borrow_mut().clear();
        self.picker.project_hover.borrow_mut().clear();
        if menu == Menu::Model {
            let generation = self.picker.menu.borrow_mut().begin_close();
            self.settle_menu(generation, false, cx);
        } else {
            self.picker.shown = None;
        }
        cx.notify();
    }

    /// Forgets every picker surface at once (the composer moved on).
    pub(crate) fn reset_picker(&mut self) {
        self.reset_model_scroll();
        self.menu = None;
        self.axis = None;
        self.picker.shown = None;
        self.picker.menu.borrow_mut().hide();
        self.picker.menu_settle = None;
        self.picker.models.clear();
        self.picker.projects.clear();
        self.picker.favorites.clear();
        self.close_axis_now();
        self.clear_tooltip();
    }

    fn settle_menu(&mut self, generation: u64, open: bool, cx: &mut Context<Self>) {
        let settle = move |this: &mut Self| {
            let mut motion = this.picker.menu.borrow_mut();
            let settled = if open {
                motion.finish_open(generation)
            } else {
                motion.finish_close(generation)
            };
            drop(motion);
            if settled && !open {
                this.picker.shown = None;
            }
            settled
        };
        if cx.reduce_motion() {
            settle(self);
            self.picker.menu_settle = None;
            return;
        }
        self.picker.menu_settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MENU_MOTION).await;
            let _ = this.update(cx, |this, cx| {
                if settle(this) {
                    this.picker.menu_settle = None;
                    cx.notify();
                }
            });
        }));
    }

    /// Opens a policy dropdown, closes it, or swaps it for another once the
    /// open one has closed.
    fn toggle_axis(&mut self, axis: Axis, cx: &mut Context<Self>) {
        match self.axis {
            Some(open) if open == axis => self.close_axis(cx),
            Some(_) => {
                self.picker.pending_axis = Some(axis);
                self.close_axis(cx);
            }
            None => self.open_axis(axis, cx),
        }
        cx.notify();
    }

    fn open_axis(&mut self, axis: Axis, cx: &mut Context<Self>) {
        self.picker
            .axis_focus
            .get_or_insert_with(|| cx.focus_handle());
        self.picker.axis_highlight = None;
        self.axis = Some(axis);
        self.picker.pending_axis = None;
        self.picker.shown_axis = Some(axis);
        let generation = self.picker.axis.borrow_mut().begin_open();
        self.settle_axis(generation, true, cx);
    }

    fn close_axis(&mut self, cx: &mut Context<Self>) {
        if self.axis.take().is_none() {
            return;
        }
        self.clear_tooltip();
        self.picker.axis_hover.borrow_mut().clear();
        let generation = self.picker.axis.borrow_mut().begin_close();
        self.settle_axis(generation, false, cx);
    }

    /// Drops the dropdown without its exit (its menu is going away).
    fn close_axis_now(&mut self) {
        self.axis = None;
        self.picker.pending_axis = None;
        self.picker.shown_axis = None;
        self.picker.axis.borrow_mut().hide();
        self.picker.axis_settle = None;
        self.picker.axis_hover.borrow_mut().clear();
    }

    fn settle_axis(&mut self, generation: u64, open: bool, cx: &mut Context<Self>) {
        let settle = move |this: &mut Self, cx: &mut Context<Self>| {
            let mut motion = this.picker.axis.borrow_mut();
            let settled = if open {
                motion.finish_open(generation)
            } else {
                motion.finish_close(generation)
            };
            drop(motion);
            if settled && !open {
                this.picker.shown_axis = None;
                if let Some(next) = this.picker.pending_axis.take()
                    && this.menu == Some(Menu::Model)
                {
                    this.open_axis(next, cx);
                }
            }
            settled
        };
        if cx.reduce_motion() {
            self.picker.axis_settle = None;
            settle(self, cx);
            return;
        }
        self.picker.axis_settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MENU_MOTION).await;
            let _ = this.update(cx, |this, cx| {
                if settle(this, cx) {
                    cx.notify();
                }
            });
        }));
    }

    // Tooltips.

    /// Mounts a tooltip hidden and shows it once the pointer rests.
    fn begin_tooltip(
        &mut self,
        key: String,
        advisory: Option<String>,
        text: String,
        placement: TooltipPlacement,
        cx: &mut Context<Self>,
    ) {
        self.picker.tooltip_generation = self.picker.tooltip_generation.wrapping_add(1);
        let generation = self.picker.tooltip_generation;
        *self.picker.tooltip.borrow_mut() = Some(Tooltip {
            key,
            advisory,
            text,
            placement,
            bounds: None,
            visible: false,
        });
        self.picker.tooltip_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TOOLTIP_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.picker.tooltip_generation == generation
                    && this.menu == Some(Menu::Model)
                    && let Some(tooltip) = this.picker.tooltip.borrow_mut().as_mut()
                {
                    tooltip.visible = true;
                }
                cx.notify();
            });
        }));
    }

    fn clear_tooltip(&mut self) {
        self.picker.tooltip_generation = self.picker.tooltip_generation.wrapping_add(1);
        self.picker.tooltip.borrow_mut().take();
        self.picker.tooltip_task = None;
    }

    fn clear_tooltip_for(&mut self, key: &str) {
        let matches = self
            .picker
            .tooltip
            .borrow()
            .as_ref()
            .is_some_and(|tooltip| tooltip.key == key);
        if matches {
            self.clear_tooltip();
        }
    }

    /// The shown tooltip (`render_option_tooltip`): beside a policy option
    /// on the side with room, or centred above an engine tab.
    pub(crate) fn render_tooltip(
        &self,
        theme: &ArtisanTheme,
        viewport: gpui::Size<Pixels>,
    ) -> Option<AnyElement> {
        let tooltip = self.picker.tooltip.borrow().clone()?;
        let bounds = tooltip.bounds?;
        if !tooltip.visible {
            return None;
        }
        let viewport_width = f32::from(viewport.width).max(1.0);
        if tooltip.placement == TooltipPlacement::Above {
            let width = ENGINE_TOOLTIP_WIDTH_PX.min(viewport_width);
            return Some(
                deferred(
                    anchored()
                        .anchor(Anchor::BottomCenter)
                        .position(point(bounds.center().x, bounds.top()))
                        .offset(point(px(0.0), px(-OPTION_TOOLTIP_GAP_PX)))
                        .snap_to_window()
                        .child(tooltip_surface(
                            theme,
                            width,
                            false,
                            tooltip.advisory,
                            &tooltip.text,
                        )),
                )
                .with_priority(3)
                .into_any_element(),
            );
        }
        let right_space = viewport_width - f32::from(bounds.right()) - OPTION_TOOLTIP_GAP_PX;
        let left_space = f32::from(bounds.left()) - OPTION_TOOLTIP_GAP_PX;
        let show_right = right_space >= OPTION_TOOLTIP_WIDTH_PX || right_space >= left_space;
        let available = if show_right { right_space } else { left_space };
        let width = OPTION_TOOLTIP_WIDTH_PX.min(available.max(1.0));
        let (anchor, position, offset) = if show_right {
            (
                Anchor::LeftCenter,
                point(bounds.right(), bounds.center().y),
                point(px(OPTION_TOOLTIP_GAP_PX), px(0.0)),
            )
        } else {
            (
                Anchor::RightCenter,
                point(bounds.left(), bounds.center().y),
                point(px(-OPTION_TOOLTIP_GAP_PX), px(0.0)),
            )
        };
        Some(
            deferred(
                anchored()
                    .anchor(anchor)
                    .position(position)
                    .offset(offset)
                    .child(tooltip_surface(
                        theme,
                        width,
                        true,
                        tooltip.advisory,
                        &tooltip.text,
                    )),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }

    // Favourites.

    /// The favourite model ids of `engine`, from the Editor's settings.
    fn favorites(&self, engine: EngineId, cx: &gpui::App) -> Vec<String> {
        self.core
            .read(cx)
            .settings
            .get(FAVORITES_KEY)
            .and_then(|value| value.get(engine.as_str()))
            .and_then(serde_json::Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stars or unstars a model; favourites list first.
    fn toggle_favorite(&mut self, engine: EngineId, model: &str, cx: &mut Context<Self>) {
        let mut favorites = self.favorites(engine, cx);
        if let Some(index) = favorites.iter().position(|id| id == model) {
            favorites.remove(index);
        } else {
            favorites.push(model.to_owned());
        }
        let mut all = self
            .core
            .read(cx)
            .settings
            .get(FAVORITES_KEY)
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default();
        if favorites.is_empty() {
            all.remove(engine.as_str());
        } else {
            all.insert(engine.as_str().to_owned(), serde_json::json!(favorites));
        }
        self.core.update(cx, |core, cx| {
            core.set_setting(FAVORITES_KEY, serde_json::Value::Object(all), cx);
        });
        self.refresh_picker_catalog(cx);
        cx.notify();
    }

    /// Only changed host data, an opened picker or an explicit choice rebuilds its ordering.
    pub(crate) fn refresh_picker_catalog(&mut self, cx: &Context<Self>) {
        if self.menu.is_none() {
            return;
        }
        let Some(host) = self.host(cx) else {
            return;
        };
        let host = host.read(cx);
        let current = self.effective_config(cx);
        let engine = self
            .browsing
            .or_else(|| current.as_ref().map(|config| config.engine));
        self.picker.favorites = engine
            .map(|engine| self.favorites(engine, cx).into_iter().collect())
            .unwrap_or_default();
        self.picker.models = engine
            .and_then(|engine| host.engines.get(&engine))
            .map(|status| {
                let mut models: Vec<_> = status.models.iter().enumerate().collect();
                let chosen = current.as_ref().and_then(|config| config.model.as_deref());
                models.sort_by_key(|(index, model)| {
                    (
                        !self.picker.favorites.contains(&model.id),
                        chosen != Some(model.id.as_str()),
                        *index,
                    )
                });
                models
                    .into_iter()
                    .map(|(index, model)| (model.id.clone(), index))
                    .collect()
            })
            .unwrap_or_default();
        let mut projects: Vec<_> = host
            .projects
            .values()
            .filter(|project| !project.removed)
            .collect();
        projects.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        self.picker.projects = projects
            .into_iter()
            .map(|project| project.id.clone())
            .collect();
    }
    fn choose_model(&mut self, engine: EngineId, id: &str, cx: &mut Context<Self>) {
        let Some(host) = self.host(cx) else {
            return;
        };
        let Some(model) = host
            .read(cx)
            .engines
            .get(&engine)
            .and_then(|status| status.models.iter().find(|model| model.id == id))
        else {
            return;
        };
        let mut config = self
            .effective_config(cx)
            .filter(|config| config.engine == engine)
            .unwrap_or_else(|| EngineConfig::new(engine));
        config.model = Some(id.to_owned());
        config.effort = model.default_effort.clone();
        config.speed = None;
        config.variant = None;
        model.normalize_context_window(&mut config);
        self.choose_config(config, cx);
    }
    fn choose_project(&mut self, id: ProjectId, cx: &mut Context<Self>) {
        if !self.draft_editable(cx) {
            return;
        }
        self.project = Some(id);
        self.edited(cx);
        self.close_menu(cx);
    }
    // Rendering.

    /// The model selector's trigger: engine glyph, model name, its muted
    /// details (context, effort) and the selector mark.
    pub(crate) fn render_model_trigger(
        &self,
        theme: &ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *theme;
        let config = self.effective_config(cx);
        let host = self.host(cx);
        let model: Option<Model> = config.as_ref().and_then(|config| {
            let id = config.model.as_ref()?;
            host.as_ref()?
                .read(cx)
                .engines
                .get(&config.engine)?
                .models
                .iter()
                .find(|model| &model.id == id)
                .cloned()
        });
        let muted = theme.colors.muted_foreground.to_paint();
        let token = || div().truncate().text_size(px(14.0)).line_height(px(20.0));
        let mut body = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .min_w(px(0.0))
            .overflow_hidden();
        match &config {
            Some(config) => {
                let name = model.as_ref().map_or_else(
                    || {
                        config
                            .model
                            .clone()
                            .unwrap_or_else(|| "No model".to_owned())
                    },
                    |model| model.name.clone(),
                );
                body = body.child(token().child(name));
                if let Some(context) = model
                    .as_ref()
                    .and_then(|model| config.context_window_override().or(model.context_window))
                {
                    body = body.child(token().text_color(muted).child(context_label(context)));
                }
                if let Some(effort) = &config.effort {
                    body = body.child(token().text_color(muted).child(humanize(effort)));
                }
                body = body.children(variant_and_speed_tokens(config, model.as_ref(), muted));
            }
            None => body = body.child(token().child("No model")),
        }
        let trigger = div()
            .id("composer-model")
            .role(gpui::Role::Button)
            .aria_label("Choose a model")
            .aria_expanded(self.menu == Some(Menu::Model))
            .focusable()
            .tab_index(0)
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(COMPACT_CONTROL_HEIGHT_PX))
            .max_w(px(360.0))
            .px(px(8.0))
            .rounded(px(10.0))
            .cursor_pointer()
            .hover(move |style| style.bg(hover_fill_gradient(theme)))
            .text_color(theme.colors.foreground.to_paint())
            .when_some(config.as_ref(), |trigger, config| {
                trigger.child(
                    asset_glyph(engine_icon(config.engine))
                        .size(px(16.0))
                        .flex_shrink_0(),
                )
            })
            .child(body)
            .child(
                asset_glyph(AssetId::TABLER_SELECTOR)
                    .size(px(14.0))
                    .flex_shrink_0()
                    .text_color(muted),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if this.menu == Some(Menu::Model) {
                    this.close_menu(cx);
                } else {
                    this.close_menu(cx);
                    this.open_menu(Menu::Model, cx);
                }
            }));
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .min_w(px(0.0))
            .child(trigger);
        if self.picker.shown == Some(Menu::Model) {
            root = root.child(
                div().absolute().top_0().left_0().child(
                    deferred(
                        anchored()
                            .anchor(Anchor::BottomLeft)
                            .offset(gpui::point(px(0.0), px(-MENU_GAP_PX)))
                            .snap_to_window_with_margin(px(8.0))
                            .child(self.model_menu(&theme, cx)),
                    )
                    .with_priority(1),
                ),
            );
        }
        root
    }

    /// The card's tab: a new thread's project picker (a ghost select), or
    /// the project the thread belongs to as a quiet label.
    pub(crate) fn render_tab(
        &self,
        theme: &ArtisanTheme,
        desktop: WorkspaceColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = *theme;
        let host = self.host(cx)?;
        let host = host.read(cx);
        let project_name =
            |id: &artisan_api::ProjectId| host.projects.get(id).map(|project| project.name.clone());
        let face = |label: SharedString| {
            div()
                .h(px(28.0))
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(13.0))
                .text_color(desktop.secondary)
                .child(asset_glyph(AssetId::TABLER_FOLDER).size(px(15.0)))
                .child(div().truncate().max_w(px(240.0)).child(label))
        };
        match &self.target {
            crate::ComposerTarget::Thread { thread, .. } => {
                let thread = host.threads.get(thread)?;
                let name = project_name(&thread.project_id)?;
                Some(face(name.into()).into_any_element())
            }
            // A new thread's project is always a choice; the one it was
            // opened for is only where it starts (master's
            // `home_project_picker`).
            crate::ComposerTarget::NewThread { .. } => {
                let open = self.menu == Some(Menu::Project);
                let label: SharedString = self
                    .project()
                    .as_ref()
                    .and_then(project_name)
                    .map_or_else(|| "Choose a project".into(), Into::into);
                let foreground = desktop.foreground;
                let trigger = div()
                    .id("composer-project")
                    .role(gpui::Role::Button)
                    .aria_label(label.clone())
                    .aria_expanded(open)
                    .focusable()
                    .tab_index(0)
                    .debug_selector(|| "composer-project".to_owned())
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(10.0))
                    .text_size(px(13.0))
                    .text_color(if open {
                        desktop.foreground
                    } else {
                        desktop.secondary
                    })
                    .cursor_pointer()
                    .hover(move |style| style.text_color(foreground))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.menu == Some(Menu::Project) {
                            this.close_menu(cx);
                        } else {
                            this.close_menu(cx);
                            this.open_menu(Menu::Project, cx);
                        }
                    }))
                    .child(asset_glyph(AssetId::TABLER_FOLDER).size(px(15.0)))
                    .child(div().truncate().max_w(px(240.0)).child(label))
                    .child(asset_glyph(AssetId::TABLER_SELECTOR).size(px(14.0)));
                let mut root = div().relative().child(trigger);
                if self.picker.shown == Some(Menu::Project) {
                    root = root.child(
                        div().absolute().top_0().left_0().child(
                            deferred(
                                anchored()
                                    .anchor(Anchor::BottomLeft)
                                    .offset(gpui::point(px(0.0), px(-PROJECT_MENU_GAP_PX)))
                                    .snap_to_window_with_margin(px(8.0))
                                    .child(self.project_menu(&theme, desktop, cx)),
                            )
                            .with_priority(1),
                        ),
                    );
                }
                Some(root.into_any_element())
            }
        }
    }

    /// The project menu (`home_project_picker`): one row per project, with
    /// the shared hover pill flying between them.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder keeps the list, its rows, their probes and the hover pill in visual order"
    )]
    fn project_menu(
        &self,
        theme: &ArtisanTheme,
        desktop: WorkspaceColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *theme;
        let width = PROJECT_MENU_WIDTH_PX.min(self.viewport_width - MENU_VIEWPORT_INSET_X_PX);
        let count = self.picker.projects.len();
        let project_rows = uniform_list(
            "composer-project-list",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let Some(host) = this.host(cx) else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| {
                        let project = host
                            .read(cx)
                            .projects
                            .get(this.picker.projects.get(index)?)?;
                        let chosen = this.project().as_ref() == Some(&project.id);
                        let id = project.id.clone();
                        let hover_id = id.to_string();
                        Some(
                            div()
                                .id(SharedString::from(format!("composer-project-{id}")))
                                .role(gpui::Role::MenuItem)
                                .aria_label(project.name.clone())
                                .aria_selected(chosen)
                                .focusable()
                                .tab_index(0)
                                .debug_selector({
                                    let id = id.clone();
                                    move || format!("composer-project-row-{id}")
                                })
                                .relative()
                                .flex()
                                .items_center()
                                .w_full()
                                .gap(px(10.0))
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(PROJECT_CONTROL_RADIUS_PX))
                                .cursor_pointer()
                                .on_hover(cx.listener({
                                    let hover_id = hover_id.clone();
                                    move |this, hovered: &bool, _, cx| {
                                        if *hovered {
                                            this.picker
                                                .project_hover
                                                .borrow_mut()
                                                .set_active(hover_id.clone());
                                            cx.notify();
                                        }
                                    }
                                }))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.choose_project(id.clone(), cx);
                                }))
                                .child(row_probe(
                                    hover_id,
                                    &this.picker.project_hover,
                                    &this.picker.project_surface,
                                ))
                                .child(
                                    asset_glyph(AssetId::TABLER_FOLDER)
                                        .size(px(16.0))
                                        .text_color(desktop.secondary),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .truncate()
                                        .text_size(px(14.0))
                                        .text_color(desktop.foreground)
                                        .child(project.name.clone()),
                                )
                                .when(chosen, |row| {
                                    row.child(
                                        div()
                                            .text_size(px(16.0))
                                            .text_color(desktop.secondary)
                                            .child("✓"),
                                    )
                                })
                                .h(px(36.0))
                                .into_any_element(),
                        )
                    })
                    .collect()
            }),
        )
        .h(px((count.min(8).max(1) * 36) as f32));
        let mut list = div().flex().flex_col().p(px(8.0)).child(project_rows);
        // The distinct intake row after a hairline: adding a project.
        list = list.child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_shrink_0()
                        .h(px(1.0))
                        .w_full()
                        .bg(desktop.popover_line)
                        .my(px(4.0))
                        .mx(px(4.0)),
                )
                .child(
                    div()
                        .id("composer-project-new")
                        .role(gpui::Role::MenuItem)
                        .aria_label("New project")
                        .focusable()
                        .tab_index(0)
                        .debug_selector(|| "composer-project-new".to_owned())
                        .relative()
                        .flex()
                        .items_center()
                        .w_full()
                        .gap(px(10.0))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(PROJECT_CONTROL_RADIUS_PX))
                        .cursor_pointer()
                        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.picker
                                    .project_hover
                                    .borrow_mut()
                                    .set_active(NEW_PROJECT_ROW.to_owned());
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_menu(cx);
                            cx.emit(crate::ComposerEvent::NewProject);
                        }))
                        .child(row_probe(
                            NEW_PROJECT_ROW.to_owned(),
                            &self.picker.project_hover,
                            &self.picker.project_surface,
                        ))
                        .child(
                            div()
                                .w(px(16.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .flex_shrink_0()
                                .text_size(px(16.0))
                                .text_color(desktop.secondary)
                                .child("+"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_size(px(14.0))
                                .text_color(desktop.foreground)
                                .child("New project"),
                        ),
                ),
        );
        let radius = px(PROJECT_MENU_RADIUS_PX);
        div()
            .id("composer-project-menu")
            .occlude()
            .relative()
            .flex()
            .flex_col()
            .w(px(width))
            .rounded(radius)
            .overflow_hidden()
            .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
            .bg(glass_foreground_base(&theme))
            .shadow(glass_card_shadows())
            .child(glass_material_layer(GlassStrength::Strong, radius))
            .child(glass_highlight_layer(GlassStrength::Strong, radius))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.picker.project_hover.borrow_mut().hide();
                    cx.notify();
                }
            }))
            .child(surface_probe(&self.picker.project_surface))
            .child(sliding_hover_pill(
                "composer-project-hover",
                &theme,
                &self.picker.project_hover,
                px(PROJECT_CONTROL_RADIUS_PX),
                true,
                cx.reduce_motion(),
            ))
            .child(list)
            .into_any_element()
    }

    /// The glass model menu: engine tabs over the model list and the
    /// previewed model's details and policy controls. It slides in, and
    /// out again before it unmounts.
    fn model_menu(&self, theme: &ArtisanTheme, cx: &mut Context<Self>) -> AnyElement {
        let theme = *theme;
        let width = MENU_WIDTH_PX.min(self.viewport_width - MENU_VIEWPORT_INSET_X_PX);
        let engines: Vec<EngineStatus> = self
            .host(cx)
            .map(|host| host.read(cx).engines.values().cloned().collect())
            .unwrap_or_default();
        let current = self.effective_config(cx);
        let browsing = self
            .browsing
            .or(current.as_ref().map(|config| config.engine))
            .or(engines.first().map(|status| status.engine));
        let closing = self.picker.menu.borrow().phase() == MenuPhase::Closing;
        let panel = glass_panel("composer-model-menu", &theme, 22.0)
            .w(px(width))
            .max_h(px(MENU_MAX_HEIGHT_PX))
            .p(px(8.0))
            .gap(px(8.0))
            .when(!closing, |panel| {
                panel.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    if this.axis.is_none() {
                        this.close_menu(cx);
                    }
                }))
            })
            .child(self.engine_tabs(&engines, browsing, &theme, cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h(px(MODEL_PANEL_HEIGHT_PX))
                    .flex_shrink_0()
                    .gap(px(8.0))
                    .child(self.model_list(&engines, browsing, current.as_ref(), &theme, cx))
                    .child(self.model_preview(&engines, current.as_ref(), &theme, cx)),
            );
        animate_menu(
            panel,
            &self.picker.menu,
            "composer-model-menu",
            MenuEntrance::Slide,
            MENU_MOTION,
            MENU_MOTION,
        )
    }

    /// The engine tabs, with the light under the active one sliding from
    /// tab to tab. A tab that cannot offer models now is greyed and says
    /// why on hover.
    fn engine_tabs(
        &self,
        engines: &[EngineStatus],
        browsing: Option<EngineId>,
        theme: &ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *theme;
        let mut tabs = div()
            .id("composer-engine-tabs")
            .relative()
            .flex()
            .flex_row()
            .w_full()
            .h(px(40.0))
            .flex_shrink_0()
            .gap(px(4.0))
            .p(px(4.0))
            .rounded(px(10.0))
            .overflow_hidden()
            .bg(control_gradient())
            .shadow(card_shadows(&theme))
            .child(surface_probe(&self.picker.tabs_surface));
        if browsing.is_some() && self.picker.light.borrow().lit.is_some() {
            tabs = tabs.child(engine_light(&theme, &self.picker.light.borrow()));
        }
        for status in engines {
            let engine = status.engine;
            let gate = Gate::of(status);
            let active = browsing == Some(engine);
            let key = format!("engine:{engine}");
            let light = Rc::clone(&self.picker.light);
            let surface = Rc::clone(&self.picker.tabs_surface);
            let probe = canvas(
                |_, _, _| {},
                move |bounds, (), window, cx| {
                    if !active {
                        return;
                    }
                    let Some(surface) = *surface.borrow() else {
                        return;
                    };
                    let left = f32::from(bounds.left() - surface.left());
                    let width = f32::from(bounds.size.width);
                    if light.borrow_mut().measure(engine, left, width) {
                        window.defer(cx, |window, _| window.refresh());
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("composer-engine-{engine}")))
                    .role(gpui::Role::Button)
                    .aria_label(engine.display_name())
                    .aria_selected(active)
                    .focusable()
                    .tab_index(0)
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(32.0))
                    .flex_shrink_0()
                    .rounded(px(14.0))
                    .cursor_pointer()
                    .text_color(theme.colors.foreground.to_paint())
                    .when(gate.is_some(), |tab| tab.opacity(0.35))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reset_model_scroll();
                        this.browsing = Some(engine);
                        this.refresh_picker_catalog(cx);
                        this.close_axis(cx);
                        cx.notify();
                    }))
                    .when_some(gate, |tab, gate| {
                        let reason = gate.reason(engine);
                        let key = key.clone();
                        tab.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.axis.is_none() {
                                this.begin_tooltip(
                                    key.clone(),
                                    None,
                                    reason.clone(),
                                    TooltipPlacement::Above,
                                    cx,
                                );
                            } else {
                                this.clear_tooltip_for(&key);
                            }
                            cx.notify();
                        }))
                    })
                    .child(probe)
                    .child(tooltip_probe(key, &self.picker.tooltip))
                    .child(
                        asset_glyph(engine_icon(engine))
                            .size(px(16.0))
                            .flex_shrink_0(),
                    ),
            );
        }
        tabs
    }

    /// The model list: favourites first, then the chosen model, then the
    /// engine's order; the shared hover pill flies between the rows.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI list builder keeps the gate, rows, favourite stars and their wiring in visual order"
    )]
    fn model_list(
        &self,
        engines: &[EngineStatus],
        browsing: Option<EngineId>,
        _current: Option<&EngineConfig>,
        theme: &ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *theme;
        let muted = theme.colors.muted_foreground.to_paint();
        let mut list = div()
            .id("composer-model-list")
            .debug_selector(|| "composer-model-viewport".into())
            .relative()
            .overflow_hidden()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.picker.model_hover.borrow_mut().hide();
                    cx.notify();
                }
            }))
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .gap(px(3.0))
            .child(surface_probe(&self.picker.model_surface))
            .child(sliding_hover_pill(
                "composer-model-hover",
                &theme,
                &self.picker.model_hover,
                px(14.0),
                true,
                cx.reduce_motion(),
            ));
        let Some(status) =
            browsing.and_then(|engine| engines.iter().find(|status| status.engine == engine))
        else {
            return list.child(
                div()
                    .p(px(12.0))
                    .text_color(muted)
                    .child("Forge reported no engines."),
            );
        };
        let engine = status.engine;
        match Gate::of(status) {
            Some(gate @ (Gate::NotInstalled | Gate::SignIn)) => {
                let action = if gate == Gate::SignIn {
                    format!("Sign in to {}", engine.display_name())
                } else {
                    format!("Install {}", engine.display_name())
                };
                return list.child(
                    div()
                        .p(px(12.0))
                        .text_size(px(14.0))
                        .text_color(muted)
                        .child(format!(
                            "{} is not ready: {}.",
                            engine.display_name(),
                            gate.label().to_lowercase()
                        ))
                        .child(
                            div()
                                .id(SharedString::from(format!("composer-sign-in-{engine}")))
                                .role(gpui::Role::Button)
                                .aria_label(action.clone())
                                .focusable()
                                .tab_index(0)
                                .mt(px(8.0))
                                .cursor_pointer()
                                .text_color(theme.colors.foreground.to_paint())
                                .child(action)
                                .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx))),
                        ),
                );
            }
            Some(gate) if status.models.is_empty() => {
                return list.child(div().p(px(12.0)).text_color(muted).child(gate.label()));
            }
            _ => {}
        }
        let count = self.picker.models.len();
        let rows = uniform_list(
            "composer-model-rows",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let Some(host) = this.host(cx) else {
                    return Vec::new();
                };
                let Some(status) = host.read(cx).engines.get(&engine) else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| {
                        let (id, model_index) = this.picker.models.get(index)?;
                        let model = status
                            .models
                            .get(*model_index)
                            .filter(|model| &model.id == id)?;
                        let favorite = this.picker.favorites.contains(&model.id);
                        let model_id = model.id.clone();
                        let hover_id = model.id.clone();
                        let star_id = model.id.clone();
                        Some(
                            div()
                                .pb(px(3.0))
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "composer-model-{engine}-{}",
                                            model.id
                                        )))
                                        .role(gpui::Role::MenuItem)
                                        .aria_label(model.name.clone())
                                        .focusable()
                                        .tab_index(0)
                                        .debug_selector({
                                            let id = model.id.clone();
                                            move || format!("composer-model-row-{id}")
                                        })
                                        .relative()
                                        .flex()
                                        .flex_shrink_0()
                                        .items_center()
                                        .gap(px(8.0))
                                        .h(px(MODEL_ROW_HEIGHT_PX))
                                        .px(px(10.0))
                                        .rounded(px(14.0))
                                        .cursor_pointer()
                                        .on_hover(cx.listener(
                                            move |this, hovered: &bool, _, cx| {
                                                if *hovered && this.menu == Some(Menu::Model) {
                                                    this.picker
                                                        .model_hover
                                                        .borrow_mut()
                                                        .set_active(hover_id.clone());
                                                    cx.notify();
                                                }
                                            },
                                        ))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.choose_model(engine, &model_id, cx)
                                        }))
                                        .child(row_probe(
                                            model.id.clone(),
                                            &this.picker.model_hover,
                                            &this.picker.model_surface,
                                        ))
                                        .child(
                                            asset_glyph(engine_icon(engine))
                                                .size(px(20.0))
                                                .text_color(theme.colors.foreground.to_paint())
                                                .flex_shrink_0(),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .flex_1()
                                                .min_w(px(0.0))
                                                .overflow_hidden()
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_size(px(14.0))
                                                        .line_height(px(20.0))
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(model.name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_size(px(12.0))
                                                        .line_height(px(16.0))
                                                        .text_color(muted)
                                                        .child(engine_lab(engine)),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id(SharedString::from(format!(
                                                    "composer-model-favorite-{engine}-{}",
                                                    model.id
                                                )))
                                                .role(gpui::Role::Button)
                                                .aria_label(if favorite {
                                                    "Remove favorite"
                                                } else {
                                                    "Add favorite"
                                                })
                                                .focusable()
                                                .tab_index(0)
                                                .debug_selector({
                                                    let id = model.id.clone();
                                                    move || format!("composer-model-favorite-{id}")
                                                })
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .size(px(28.0))
                                                .flex_shrink_0()
                                                .rounded_full()
                                                .hover(move |style| {
                                                    style.bg(hover_fill_gradient(theme))
                                                })
                                                .text_color(if favorite {
                                                    theme.colors.favorite.to_paint()
                                                } else {
                                                    muted
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    this.toggle_favorite(engine, &star_id, cx);
                                                }))
                                                .child(
                                                    asset_glyph(if favorite {
                                                        AssetId::TABLER_STAR_FILLED
                                                    } else {
                                                        AssetId::TABLER_STAR
                                                    })
                                                    .size(px(16.0)),
                                                ),
                                        )
                                        .into_any_element(),
                                )
                                .into_any_element(),
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.picker.model_scroll)
        .h(px(MODEL_PANEL_HEIGHT_PX));
        // Painted after the native list, matching transcript wheel routing.
        // This consumes wheel ticks before native scrolling applies them.
        list = list.child(rows).child(
            div()
                .absolute()
                .inset_0()
                .on_scroll_wheel(cx.listener(Self::model_wheel)),
        );
        list
    }

    /// The chosen model's name, description and policy controls.
    fn model_preview(
        &self,
        engines: &[EngineStatus],
        current: Option<&EngineConfig>,
        theme: &ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *theme;
        let mut preview = div()
            .id("composer-model-preview")
            .flex()
            .flex_col()
            .justify_between()
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .w(px(MODEL_PREVIEW_WIDTH_PX))
            .flex_shrink_0()
            .min_w(px(0.0))
            .gap(px(8.0))
            .p(px(10.0));
        let Some(config) = current else {
            return preview;
        };
        let Some(status) = engines.iter().find(|status| status.engine == config.engine) else {
            return preview;
        };
        let model = config
            .model
            .as_ref()
            .and_then(|id| status.models.iter().find(|model| &model.id == id));
        let name = model.map_or_else(
            || config.model.clone().unwrap_or_default(),
            |model| model.name.clone(),
        );
        let mut summary = div().flex().flex_col().gap(px(4.0)).child(
            div()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(14.0))
                .line_height(px(20.0))
                .child(name),
        );
        let line = model
            .and_then(|model| model.description.clone())
            .unwrap_or_else(|| {
                let mut facts = vec![format!(
                    "{} by {}",
                    config.engine.display_name(),
                    engine_lab(config.engine)
                )];
                if let Some(context) = model
                    .and_then(|model| config.context_window_override().or(model.context_window))
                {
                    facts.push(format!("{} context", context_label(context)));
                }
                if model.is_some_and(|model| model.supports_images) {
                    facts.push("Reads images".to_owned());
                }
                facts.join(" · ")
            });
        summary = summary.child(
            div()
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(line),
        );
        preview = preview.child(summary);
        let mut controls = div().flex().flex_col().gap(px(6.0));
        for axis in Axis::ALL {
            let options = axis_options(axis, config, model);
            if axis.shown(options.len()) {
                controls = controls.child(self.axis_control(axis, options, &theme, cx));
            }
        }
        preview.child(controls)
    }

    /// One policy control and, while open or closing, its glass dropdown
    /// with the shared hover pill and option tooltips.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder keeps the trigger, the dropdown, its rows and their probes in visual order"
    )]
    fn axis_control(
        &self,
        axis: Axis,
        options: Vec<AxisOption>,
        theme: &ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = *theme;
        let muted = theme.colors.muted_foreground.to_paint();
        let value = options
            .iter()
            .find(|option| option.selected)
            .or(options.first())
            .map(|option| option.label.clone())
            .unwrap_or_default();
        let trigger = div()
            .id(SharedString::from(format!("composer-axis-{axis:?}")))
            .role(gpui::Role::Button)
            .aria_label(format!("{}: {value}", axis.title()))
            .aria_expanded(self.axis == Some(axis))
            .focusable()
            .tab_index(0)
            .debug_selector(move || format!("composer-axis-{axis:?}"))
            .flex()
            .items_center()
            .gap(px(7.0))
            .h(px(POLICY_CONTROL_HEIGHT_PX))
            .px(px(8.0))
            .rounded(px(8.0))
            .bg(control_gradient())
            .shadow(card_shadows(&theme))
            .cursor_pointer()
            .relative()
            .child(
                div()
                    .absolute()
                    .size_full()
                    .debug_selector(move || format!("composer-axis-{axis:?}-trigger")),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_axis(axis, cx);
                if let Some(focus) = &this.picker.axis_focus {
                    focus.focus(window, cx);
                }
            }))
            .child(
                asset_glyph(axis.icon())
                    .size(px(14.0))
                    .flex_shrink_0()
                    .text_color(muted),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .justify_center()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(theme.colors.foreground.to_paint())
                    .child(value),
            )
            .child(
                asset_glyph(AssetId::TABLER_SELECTOR)
                    .size(px(14.0))
                    .text_color(muted),
            );
        let mut control = div()
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .child(trigger);
        if self.picker.shown_axis != Some(axis) {
            return control;
        }
        let ids: Vec<String> = (0..options.len()).map(|index| index.to_string()).collect();
        self.picker.axis_hover.borrow_mut().clear_if_missing(&ids);
        let closing = self.picker.axis.borrow().phase() == MenuPhase::Closing;
        let mut rows = div()
            .id(SharedString::from(format!("composer-axis-rows-{axis:?}")))
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.picker.axis_hover.borrow_mut().clear();
                    this.clear_tooltip();
                    cx.notify();
                }
            }));
        let header = |label: &'static str, special: bool| {
            div()
                .debug_selector(move || {
                    if special {
                        format!("composer-axis-{axis:?}-special-header")
                    } else {
                        format!("composer-axis-{axis:?}-header")
                    }
                })
                .px(px(12.0))
                .py(px(4.0))
                .text_size(px(12.0))
                .line_height(px(16.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child(label)
        };
        rows = rows.child(header(axis.title(), false));
        let keyboard_options: Vec<_> = options.iter().map(|option| option.apply.clone()).collect();
        let chosen = options
            .iter()
            .position(|option| option.selected)
            .unwrap_or(0);
        let mut special_shown = false;
        for (index, option) in options.into_iter().enumerate() {
            if option.special && !special_shown {
                special_shown = true;
                rows = rows
                    .child(
                        div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(theme.workspace.line),
                    )
                    .child(header("Special Efforts", true));
            }
            let config = option.apply;
            let key = format!("axis:{axis:?}:{index}");
            let hover_id = index.to_string();
            let (advisory, description) = (option.advisory, option.description);
            let tooltip_key = key.clone();
            rows = rows.child(
                option_row(
                    format!("composer-axis-{axis:?}-{index}").into(),
                    option.label,
                    option.selected,
                    &theme,
                )
                .debug_selector(move || format!("composer-axis-{axis:?}-{index}"))
                .child(row_probe(
                    hover_id.clone(),
                    &self.picker.axis_hover,
                    &self.picker.axis_surface,
                ))
                .child(tooltip_probe(key, &self.picker.tooltip))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        if this.axis != Some(axis) {
                            return;
                        }
                        this.picker.axis_highlight = Some(index);
                        this.picker
                            .axis_hover
                            .borrow_mut()
                            .set_active(hover_id.clone());
                        if advisory.is_some() || description.is_some() {
                            this.begin_tooltip(
                                tooltip_key.clone(),
                                advisory.clone(),
                                description.clone().unwrap_or_default(),
                                TooltipPlacement::Side,
                                cx,
                            );
                        } else {
                            this.clear_tooltip();
                        }
                    } else {
                        this.clear_tooltip_for(&tooltip_key);
                    }
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.close_axis(cx);
                    this.choose_config(config.clone(), cx);
                })),
            );
        }
        let panel = glass_panel(format!("composer-axis-options-{axis:?}"), &theme, 18.0)
            .when_some(self.picker.axis_focus.as_ref(), |panel, focus| {
                panel.track_focus(focus)
            })
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if keyboard_options.is_empty() {
                    return;
                }
                let current = this.picker.axis_highlight.unwrap_or(chosen);
                let next = match event.keystroke.key.as_str() {
                    "home" => 0,
                    "end" => keyboard_options.len() - 1,
                    "up" => current.saturating_sub(1),
                    "down" => (current + 1).min(keyboard_options.len() - 1),
                    "enter" | "space" => {
                        this.choose_config(keyboard_options[current].clone(), cx);
                        this.close_axis(cx);
                        cx.stop_propagation();
                        return;
                    }
                    "escape" => {
                        if this.axis.is_some() {
                            this.close_axis(cx);
                        } else {
                            this.close_menu(cx);
                        }
                        cx.stop_propagation();
                        return;
                    }
                    _ => return,
                };
                this.picker.axis_highlight = Some(next);
                this.picker
                    .axis_hover
                    .borrow_mut()
                    .set_active(next.to_string());
                cx.stop_propagation();
                cx.notify();
            }))
            .w(px(MODEL_PREVIEW_WIDTH_PX - 20.0))
            .min_w(px(DROPDOWN_MIN_WIDTH_PX))
            .p(px(4.0))
            .when(!closing, |panel| {
                panel.on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_axis(cx)))
            })
            .child(surface_probe(&self.picker.axis_surface))
            .child(sliding_hover_pill(
                "composer-axis-hover",
                &theme,
                &self.picker.axis_hover,
                px(14.0),
                true,
                cx.reduce_motion(),
            ))
            .child(rows);
        control = control.child(
            div().absolute().top_0().left_0().child(
                deferred(
                    anchored()
                        .anchor(Anchor::BottomLeft)
                        .offset(gpui::point(px(0.0), px(-DROPDOWN_GAP_PX)))
                        .snap_to_window_with_margin(px(8.0))
                        .child(animate_menu(
                            panel,
                            &self.picker.axis,
                            "composer-axis-options",
                            MenuEntrance::Slide,
                            MENU_MOTION,
                            MENU_MOTION,
                        )),
                )
                .with_priority(2),
            ),
        );
        control
    }
}

/// A tooltip's glass surface (`render_option_tooltip_surface`): the
/// advisory first, in the warning colour, then the description.
fn tooltip_surface(
    theme: &ArtisanTheme,
    width: f32,
    side: bool,
    advisory: Option<String>,
    text: &str,
) -> AnyElement {
    let advisory = advisory.filter(|advisory| !advisory.is_empty());
    let advisory_color = if advisory.as_deref() == Some("Additional info:") {
        theme.colors.banner_info
    } else {
        theme.colors.destructive
    };
    let mut body = String::new();
    let mut advisory_end = 0;
    if let Some(advisory) = advisory {
        body.push_str(&advisory);
        advisory_end = body.len();
        if !text.is_empty() {
            body.push(' ');
        }
    }
    body.push_str(text);
    let body =
        StyledText::new(SharedString::from(body)).with_highlights((advisory_end > 0).then(|| {
            (
                0..advisory_end,
                HighlightStyle {
                    color: Some(advisory_color.to_paint()),
                    font_weight: Some(FontWeight::MEDIUM),
                    ..HighlightStyle::default()
                },
            )
        }));
    div()
        .id("composer-picker-tooltip")
        .debug_selector(|| "composer-picker-tooltip".to_owned())
        .when(side, |surface| surface.w(px(width)))
        .max_w(px(width))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(18.0))
        .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
        .bg(glass_foreground_base(theme))
        .shadow(glass_card_shadows())
        .text_size(px(12.0))
        .line_height(px(16.0))
        .whitespace_normal()
        .text_color(theme.colors.foreground.to_paint())
        .relative()
        .overflow_hidden()
        .child(glass_material_layer(GlassStrength::Strong, px(18.0)))
        .child(glass_highlight_layer(GlassStrength::Strong, px(18.0)))
        .child(body)
        .into_any_element()
}

/// The light under the active engine tab (`render_engine_light`): a soft
/// glow baked into a cached raster, sliding from the last tab to this one.
fn engine_light(theme: &ArtisanTheme, light: &EngineLight) -> AnyElement {
    let target = light_left(light.left, light.width);
    let image = img(ImageSource::Render(engine_light_image(theme)))
        .absolute()
        .top(px(-2.0))
        .left(px(target))
        .w(px(ENGINE_LIGHT_WIDTH_PX))
        .h(px(ENGINE_LIGHT_HEIGHT_PX));
    let Some(flight) = light.flight else {
        return image.into_any_element();
    };
    let LightFlight {
        from_left,
        to_left,
        generation,
    } = flight;
    image
        .left(px(from_left))
        .with_animation(
            ElementId::Name(format!("composer-engine-light-{generation}").into()),
            Animation::new(ENGINE_LIGHT_FLIGHT).with_easing(smooth_out),
            move |image, progress| image.left(px(from_left + (to_left - from_left) * progress)),
        )
        .into_any_element()
}

thread_local! {
    static ENGINE_LIGHT_IMAGE: RefCell<Option<Arc<RenderImage>>> = const { RefCell::new(None) };
}

fn engine_light_image(theme: &ArtisanTheme) -> Arc<RenderImage> {
    ENGINE_LIGHT_IMAGE.with(|image| {
        image
            .borrow_mut()
            .get_or_insert_with(|| Arc::new(RenderImage::new(vec![engine_light_frame(theme)])))
            .clone()
    })
}

/// The light's raster: the foreground colour under a vertical alpha
/// profile, blurred 2 px, then masked by a soft ellipse.
#[allow(clippy::cast_precision_loss)]
fn engine_light_frame(theme: &ArtisanTheme) -> image::Frame {
    let foreground = theme.colors.foreground.to_srgb();
    let red = color_channel_to_byte(foreground.r);
    let green = color_channel_to_byte(foreground.g);
    let blue = color_channel_to_byte(foreground.b);
    // The blur runs before the mask: bake it into the cached alpha, with
    // transparent padding for the filter's edge samples.
    let padding = 6 * ENGINE_LIGHT_RASTER_SCALE;
    let gradient = image::GrayImage::from_fn(
        ENGINE_LIGHT_RASTER_WIDTH + 2 * padding,
        ENGINE_LIGHT_RASTER_HEIGHT + 2 * padding,
        |x, y| {
            let inside = x >= padding
                && x < padding + ENGINE_LIGHT_RASTER_WIDTH
                && y >= padding
                && y < padding + ENGINE_LIGHT_RASTER_HEIGHT;
            let alpha = if inside {
                interpolate_profile(
                    (y - padding) as f32 / ENGINE_LIGHT_RASTER_HEIGHT as f32,
                    &[(0.0, 0.32), (0.26, 0.10), (0.52, 0.02), (0.74, 0.0)],
                )
            } else {
                0.0
            };
            image::Luma([color_channel_to_byte(alpha)])
        },
    );
    let blurred = image::imageops::blur(&gradient, 2.0 * ENGINE_LIGHT_RASTER_SCALE as f32);
    let buffer = image::ImageBuffer::from_fn(
        ENGINE_LIGHT_RASTER_WIDTH,
        ENGINE_LIGHT_RASTER_HEIGHT,
        |x, y| {
            let nx = (x as f32 + 0.5) / ENGINE_LIGHT_RASTER_WIDTH as f32;
            let ny = (y as f32 + 0.5) / ENGINE_LIGHT_RASTER_HEIGHT as f32;
            let distance = (((nx - 0.5) / 0.48).powi(2) + ((ny - 0.35) / 0.70).powi(2)).sqrt();
            let mask = interpolate_profile(
                distance,
                &[(0.0, 1.0), (0.42, 0.5), (0.68, 0.1), (0.88, 0.0)],
            );
            let alpha = f32::from(blurred.get_pixel(x + padding, y + padding)[0]) / 255.0;
            // RenderImage takes BGRA.
            image::Rgba([blue, green, red, color_channel_to_byte(alpha * mask)])
        },
    );
    image::Frame::new(buffer)
}

fn interpolate_profile(value: f32, stops: &[(f32, f32)]) -> f32 {
    let Some(&(first_position, first_alpha)) = stops.first() else {
        return 0.0;
    };
    if value <= first_position {
        return first_alpha;
    }
    for pair in stops.windows(2) {
        let [(start_position, start_alpha), (end_position, end_alpha)] = pair else {
            continue;
        };
        if value <= *end_position {
            let span = *end_position - *start_position;
            if span <= f32::EPSILON {
                return *end_alpha;
            }
            let progress = (value - *start_position) / span;
            return start_alpha + ((*end_alpha - *start_alpha) * progress);
        }
    }
    stops.last().map_or(0.0, |(_, alpha)| *alpha)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn color_channel_to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The trigger's `<variant> <speed>` tokens: the chosen variant, muted, and
/// a speed tier other than the model's default, in its gradient.
fn variant_and_speed_tokens(
    config: &EngineConfig,
    model: Option<&Model>,
    muted: gpui::Hsla,
) -> Vec<AnyElement> {
    let token = || div().truncate().text_size(px(14.0)).line_height(px(20.0));
    let mut tokens = Vec::new();
    let Some(model) = model else {
        return tokens;
    };
    if let Some(variant) = config
        .variant
        .as_ref()
        .and_then(|id| model.variants.iter().find(|variant| &variant.id == id))
    {
        tokens.push(
            token()
                .text_color(muted)
                .child(variant.label.clone())
                .into_any_element(),
        );
    }
    if let Some(speed) = config.speed.as_ref().and_then(|id| {
        model
            .speeds
            .iter()
            .find(|speed| &speed.id == id && !speed.default)
    }) {
        let label = speed.label.clone();
        tokens.push(
            token()
                .debug_selector(|| "composer-model-speed".to_owned())
                .child(match speed_gradient(speed) {
                    Some(gradient) => gradient_text(label, gradient),
                    None => StyledText::new(label),
                })
                .into_any_element(),
        );
    }
    tokens
}

/// The glyph gradient of an accelerated tier (`speed_label_gradient`):
/// amber into yellow for Fast, neon green into pink for Superfast.
fn speed_gradient(speed: &Speed) -> Option<(u32, u32)> {
    if speed.id == "superfast" {
        Some((0x00_FF_66, 0xFF_00_C8))
    } else if speed.id == "fast" || speed.label.eq_ignore_ascii_case("fast") {
        Some((0xFF_B3_00, 0xFF_EE_00))
    } else {
        None
    }
}

/// `text` with its glyphs walking from one colour to the other, left to
/// right (`gradient_label_text`).
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn gradient_text(text: String, (start, end): (u32, u32)) -> StyledText {
    let count = text.chars().count();
    let channel = |shift: u32, progress: f32| {
        let from = ((start >> shift) & 0xff) as f32;
        let to = ((end >> shift) & 0xff) as f32;
        (from + (to - from) * progress).round().clamp(0.0, 255.0) as u32
    };
    let mut highlights = Vec::with_capacity(count);
    let mut at = 0;
    for (index, character) in text.chars().enumerate() {
        let progress = if count > 1 {
            index as f32 / (count - 1) as f32
        } else {
            0.0
        };
        let color =
            (channel(16, progress) << 16) | (channel(8, progress) << 8) | channel(0, progress);
        let next = at + character.len_utf8();
        highlights.push((
            at..next,
            HighlightStyle {
                color: Some(gpui::rgb_to_hsla(gpui::rgb(color))),
                ..HighlightStyle::default()
            },
        ));
        at = next;
    }
    StyledText::new(text).with_highlights(highlights)
}

/// What a permission level does on `engine`, as Forge launches it.
const fn permissions_description(engine: EngineId, permissions: Permissions) -> &'static str {
    match (engine, permissions) {
        (EngineId::Codex, Permissions::Auto) => {
            "Sandboxed to the project; Codex's reviewer decides what else may run."
        }
        (EngineId::Claude, Permissions::Auto) => {
            "Claude's auto mode: safe actions run, risky ones are checked first."
        }
        (EngineId::Grok, Permissions::Auto) => "Grok's auto mode decides what runs and what asks.",
        (EngineId::Cursor | EngineId::OpenCode, Permissions::Auto) => {
            "Edits run; it asks before terminal commands."
        }
        (_, Permissions::Full) => "Everything runs, nothing asks. No sandbox.",
    }
}

/// The model's variants, the base model first as "Default"
/// (`variants_for_model`).
fn variant_options(config: &EngineConfig, model: Option<&Model>) -> Vec<AxisOption> {
    let variants = model
        .map(|model| model.variants.as_slice())
        .unwrap_or_default();
    if variants.is_empty() {
        return Vec::new();
    }
    std::iter::once(AxisOption {
        special: false,
        label: "Default".to_owned(),
        description: None,
        advisory: None,
        selected: config.variant.is_none(),
        apply: EngineConfig {
            variant: None,
            ..config.clone()
        },
    })
    .chain(variants.iter().map(|variant| AxisOption {
        special: false,
        label: variant.label.clone(),
        description: None,
        advisory: None,
        selected: config.variant.as_ref() == Some(&variant.id),
        apply: EngineConfig {
            variant: Some(variant.id.clone()),
            ..config.clone()
        },
    }))
    .collect()
}

/// The model's speed tiers; with none chosen, its default is.
fn speed_options(config: &EngineConfig, model: Option<&Model>) -> Vec<AxisOption> {
    model
        .map(|model| model.speeds.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|speed| AxisOption {
            special: false,
            label: speed.label.clone(),
            description: speed.description.clone(),
            advisory: None,
            selected: config
                .speed
                .as_ref()
                .map_or(speed.default, |chosen| chosen == &speed.id),
            apply: EngineConfig {
                speed: Some(speed.id.clone()),
                ..config.clone()
            },
        })
        .collect()
}

/// The options one policy control offers, each with the settings it
/// applies.
fn axis_options(axis: Axis, config: &EngineConfig, model: Option<&Model>) -> Vec<AxisOption> {
    let engine = config.engine;
    match axis {
        Axis::Variant => variant_options(config, model),
        Axis::Thinking => {
            let Some(model) = model else {
                return Vec::new();
            };
            let options = model.efforts.iter().map(|effort| {
                let note = model
                    .effort_notes
                    .iter()
                    .find(|note| &note.effort == effort);
                AxisOption {
                    special: note.is_some_and(|note| note.special),
                    label: humanize(effort),
                    description: note.and_then(|note| note.description.clone()),
                    advisory: note.and_then(|note| note.advisory.clone()),
                    selected: config.effort.as_ref() == Some(effort),
                    apply: EngineConfig {
                        effort: Some(effort.clone()),
                        ..config.clone()
                    },
                }
            });
            let (mut regular, special): (Vec<_>, Vec<_>) =
                options.partition(|option| !option.special);
            regular.extend(special);
            regular
        }
        Axis::Context => model.map_or_else(Vec::new, |model| {
            model
                .context_windows
                .iter()
                .map(|window| {
                    let mut apply = config.clone();
                    if let Some(tokens) = window.override_tokens {
                        apply.options.insert("context_window".into(), tokens.into());
                    } else {
                        apply.options.remove("context_window");
                    }
                    AxisOption {
                        special: false,
                        label: context_label(window.tokens),
                        description: None,
                        advisory: None,
                        selected: config.context_window_override() == window.override_tokens,
                        apply,
                    }
                })
                .collect()
        }),
        Axis::Speed => speed_options(config, model),
        Axis::Permission => [
            (Permissions::Auto, "Auto"),
            (Permissions::Full, "Full access"),
        ]
        .into_iter()
        .map(|(permissions, label)| AxisOption {
            special: false,
            label: label.to_owned(),
            description: Some(permissions_description(engine, permissions).to_owned()),
            advisory: None,
            selected: config.permissions == permissions,
            apply: EngineConfig {
                permissions,
                ..config.clone()
            },
        })
        .collect(),
    }
}

#[cfg(test)]
mod effort_presentation_tests {
    use super::*;

    #[test]
    fn special_metadata_partitions_efforts_without_changing_canonical_values() {
        let model: Model = serde_json::from_value(serde_json::json!({
            "id": "test-model", "name": "Test model",
            "efforts": ["custom_mode", "xhigh", "ultra", "low", "another_mode"],
            "effort_notes": [
                {"effort": "custom_mode", "special": true, "description": "First mode"},
                {"effort": "ultra", "special": false},
                {"effort": "another_mode", "special": true, "advisory": "Additional usage"}
            ]
        }))
        .unwrap();
        for engine in [EngineId::Claude, EngineId::Codex, EngineId::OpenCode] {
            let config = EngineConfig {
                effort: Some("custom_mode".into()),
                ..EngineConfig::new(engine)
            };
            let options = axis_options(Axis::Thinking, &config, Some(&model));
            let canonical: Vec<_> = options
                .iter()
                .map(|option| option.apply.effort.as_deref().unwrap())
                .collect();
            assert_eq!(
                canonical,
                ["xhigh", "ultra", "low", "custom_mode", "another_mode"]
            );
            assert_eq!(
                options
                    .iter()
                    .map(|option| option.special)
                    .collect::<Vec<_>>(),
                [false, false, false, true, true]
            );
            assert_eq!(options[0].label, "Extra High");
            assert_eq!(
                options[1].label, "Ultra",
                "names alone do not make an effort special"
            );
            assert!(options[3].selected);
            assert_eq!(options[3].description.as_deref(), Some("First mode"));
            assert_eq!(options[4].advisory.as_deref(), Some("Additional usage"));
            assert!(options.iter().all(|option| option.apply.engine == engine));
        }
    }

    #[test]
    fn advertised_ultra_mode_keeps_its_submitted_effort_id() {
        let model: Model = serde_json::from_value(serde_json::json!({
            "id": "test-model", "name": "Test model",
            "efforts": ["low", "medium", "high", "xhigh", "ultra"],
            "effort_notes": [{"effort": "ultra", "special": true}]
        }))
        .unwrap();
        let config = EngineConfig {
            effort: Some("ultra".into()),
            ..EngineConfig::new(EngineId::Codex)
        };
        let options = axis_options(Axis::Thinking, &config, Some(&model));
        let ultra = options.last().unwrap();
        assert!(ultra.special && ultra.selected);
        assert_eq!(ultra.label, "Ultra");
        assert_eq!(ultra.apply.effort.as_deref(), Some("ultra"));
        assert_eq!(options[3].apply.effort.as_deref(), Some("xhigh"));
        assert_eq!(options[3].label, "Extra High");
    }
}
