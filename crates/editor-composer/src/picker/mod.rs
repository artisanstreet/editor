//! The model selector (`native_model_selector`) and the card's project tab
//! (`home_project_picker`, `render_tab`), in the old Editor's look: the
//! trigger with the engine glyph, the model name and its muted details; the
//! glass menu with the engine tabs, the model list beside its preview and
//! policy controls; and the glass dropdowns those controls open.
//!
//! The surfaces are built from the shared `editor_ui` popover, glass-menu,
//! hover-surface and tooltip primitives; this module owns their state.

mod axis_menu;
mod engine_light;
mod model_menu;
mod options;
mod project_menu;
mod tooltip;
mod trigger;

use crate::{Composer, Menu};
use artisan_api::ProjectId;
use artisan_api::{
    EngineConfig, EngineId, EngineStatus, InstallState, Model, Permissions, SignInState, Speed,
};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::glass::card_shadows;
use editor_ui::gradient::vertical_gradient;
use editor_ui::hover_card::{DelayedTooltip, HoverFill, hover_fill};
use editor_ui::measure::Measured;
use editor_ui::picker_motion::{
    HoverSurface, MenuEntrance, MenuMotion, MenuPhase, animate_menu, smooth_out,
};
use editor_ui::popover::{above_trigger, glass_menu};
use editor_ui::theme::{
    ArtisanTheme, ColorTokens, RadiusStep, RadiusTokens, SurfaceStep, TextStep, TypographyTokens,
};
use engine_light::{EngineLight, engine_light};
use gpui::uniform_list;
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, ElementId, FontWeight, HighlightStyle,
    ImageSource, InteractiveElement as _, IntoElement, ParentElement as _, RenderImage,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledText, Task, div, img, point,
    prelude::FluentBuilder as _, px,
};
use options::axis_options;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tooltip::Tooltip;

/// The model menu's width, and the room it keeps from the window edges.
const MENU_WIDTH_PX: f32 = 480.0;
const MENU_MAX_HEIGHT_PX: f32 = 320.0;
const MENU_VIEWPORT_INSET_X_PX: f32 = 32.0;
/// How far each popup floats above its trigger.
const MENU_GAP_PX: f32 = 8.0;
const DROPDOWN_GAP_PX: f32 = 4.0;
const PROJECT_MENU_GAP_PX: f32 = 10.0;
const DROPDOWN_MIN_WIDTH_PX: f32 = 144.0;
const MODEL_PANEL_HEIGHT_PX: f32 = 192.0;
const MODEL_PREVIEW_WIDTH_PX: f32 = 224.0;
const MODEL_ROW_HEIGHT_PX: f32 = 48.0;
const COMPACT_CONTROL_HEIGHT_PX: f32 = 32.0;
const POLICY_CONTROL_HEIGHT_PX: f32 = 24.0;
/// Open and close of the model menu and its dropdowns: a short slide and
/// fade (`PICKER_MENU_MOTION_DURATION_MS`).
const MENU_MOTION: Duration = Duration::from_millis(100);
/// The project menu (`home_project_picker`).
const PROJECT_MENU_WIDTH_PX: f32 = 320.0;
/// The hover id of the project menu's "New project" row.
const NEW_PROJECT_ROW: &str = "new-project";
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
    pub(super) model_hover: HoverSurface,
    axis_hover: HoverSurface,
    project_hover: HoverSurface,
    light: Rc<RefCell<EngineLight>>,
    tabs_surface: Measured,
    light_tab: Measured,
    /// The engine whose tab `light_tab` measured.
    light_tab_engine: std::cell::Cell<Option<EngineId>>,
    /// The tooltip waiting for its delay or shown, and the place of the
    /// element it belongs to.
    tooltip: DelayedTooltip<Tooltip>,
    tooltip_anchor: Measured,
}

/// The selector's control gradient (`source_control_gradient`).
fn control_gradient() -> gpui::Background {
    vertical_gradient(SurfaceStep::S800.oklch(), SurfaceStep::S925.oklch())
}

impl Composer {
    // Opening and closing.

    /// Opens a picker menu: the model menu slides in; the project menu
    /// appears in place.
    fn reset_model_scroll(&mut self) {
        self.picker.model_glide.cancel();
        self.picker.model_hover.clear();
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
        self.picker.model_hover.clear();
        self.picker.project_hover.clear();
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
        self.picker.axis_hover.clear();
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
        self.picker.axis_hover.clear();
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
        config.effort.clone_from(&model.default_effort);
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
}
