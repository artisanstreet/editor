//! Controlled GPUI lip-card primitive for the reached thread composer.
//!
//! A [`LipCard`] has two caller-owned, distinct children: a persistent lip and
//! a panel. The lip is always mounted. A closed card omits the panel entirely,
//! which gives the closed state zero panel height and makes descendant events
//! and focus impossible. [`LipCardPhase::Closing`] is the explicit exception:
//! callers may keep the panel mounted while a collapse animation finishes.
//!
//! Pinned GPUI 0.2.2 has an animation clock and opacity, but no blur filter and
//! no interpolated `auto` height. The motion plan records those facts instead
//! of promising CSS behavior that GPUI cannot provide. Full motion therefore
//! uses the shared accordion recipe for an opacity fade inside a static
//! rectangular overflow clip; the caller settles the explicit transition and
//! removes the panel when the recipe completes.

use gpui::{
    AnimationExt, AnyElement, App, Background, BoxShadow, Div, ElementId, InteractiveElement,
    IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, linear_color_stop,
    linear_gradient, px,
};

use crate::motion::{MotionAnimation, MotionPlan, MotionPolicy, MotionRecipe};
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ShadowLayer, SurfaceStep, ThemeMode};

/// Stable debug selector for a card root.
pub const LIP_CARD_ROOT_SELECTOR: &str = "artisan-lip-card";

/// Stable debug selector for the persistent lip wrapper.
pub const LIP_CARD_LIP_SELECTOR: &str = "artisan-lip-card-lip";

/// Stable debug selector for a mounted panel wrapper.
pub const LIP_CARD_PANEL_SELECTOR: &str = "artisan-lip-card-panel";

const CARD_RADIUS: RadiusStep = RadiusStep::X2l;

/// The two reached lip-card paint treatments.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum LipCardVariant {
    /// Paint the audited top-to-bottom surface gradient and card elevation.
    #[default]
    Solid,
    /// Keep the same rounded and clipped geometry, but omit the fill.
    Glass,
}

/// Theme-resolved paint and structural values for a [`LipCard`].
///
/// The light solid recipe is `surface-200 → surface-125`; the dark solid
/// recipe is `surface-850 → surface-900`. A glass recipe has no background
/// fill, while retaining the rounded clipping and shared four-layer card
/// elevation so the two variants have the same structural silhouette.
#[derive(Clone, Debug, PartialEq)]
pub struct LipCardStyle {
    /// The selected paint treatment.
    pub variant: LipCardVariant,
    /// The resolved vertical background, or `None` for glass.
    pub background: Option<Background>,
    /// The shared `rounded-2xl` geometry: 18 px.
    pub corner_radius: gpui::Pixels,
    /// The audited `card` shadow stack.
    pub card_shadows: Vec<BoxShadow>,
}

impl LipCardStyle {
    /// Resolves the audited lip-card recipe for one theme mode and variant.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, variant: LipCardVariant) -> Self {
        let background = match variant {
            LipCardVariant::Solid => {
                let (top, bottom) = match theme.mode {
                    ThemeMode::Light => (SurfaceStep::S200, SurfaceStep::S125),
                    ThemeMode::Dark => (SurfaceStep::S850, SurfaceStep::S900),
                };

                Some(linear_gradient(
                    0.0,
                    linear_color_stop(theme.surfaces.value(top).to_paint(), 0.0),
                    linear_color_stop(theme.surfaces.value(bottom).to_paint(), 1.0),
                ))
            }
            LipCardVariant::Glass => None,
        };

        Self {
            variant,
            background,
            corner_radius: RadiusTokens::value(CARD_RADIUS),
            card_shadows: theme
                .elevation
                .card_shadow
                .into_iter()
                .map(ShadowLayer::to_box_shadow)
                .collect(),
        }
    }

    /// Resolves the solid treatment for `theme`.
    #[must_use]
    pub fn solid(theme: ArtisanTheme) -> Self {
        Self::resolve(theme, LipCardVariant::Solid)
    }

    /// Resolves the transparent glass treatment for `theme`.
    #[must_use]
    pub fn glass(theme: ArtisanTheme) -> Self {
        Self::resolve(theme, LipCardVariant::Glass)
    }
}

/// The deterministic presentation phase of a controlled lip card.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum LipCardPhase {
    /// The panel is absent and the card has zero panel height.
    #[default]
    Closed,
    /// The panel is mounted while the accordion-expand recipe fades it in.
    Opening,
    /// The panel is mounted at its settled open layout.
    Open,
    /// The panel is mounted while the accordion-collapse recipe fades it out.
    Closing,
}

impl LipCardPhase {
    /// Returns the settled phase for a controlled target value.
    #[must_use]
    pub const fn settled(open: bool) -> Self {
        if open { Self::Open } else { Self::Closed }
    }

    /// Returns the controlled target represented by this phase.
    #[must_use]
    pub const fn target_open(self) -> bool {
        matches!(self, Self::Opening | Self::Open)
    }

    /// Whether the panel child must be mounted for this phase.
    #[must_use]
    pub const fn panel_present(self) -> bool {
        !matches!(self, Self::Closed)
    }

    /// Whether the phase is settled rather than an explicit transition.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        matches!(self, Self::Closed | Self::Open)
    }

    /// Closed-state inertness is structural: no descendant exists to receive
    /// a pointer event or focus request.
    #[must_use]
    pub const fn is_inert(self) -> bool {
        matches!(self, Self::Closed)
    }

    /// Whether pointer input should be blocked while this phase is rendered.
    #[must_use]
    pub const fn is_pointer_inert(self) -> bool {
        matches!(self, Self::Closed | Self::Closing)
    }

    /// Whether descendants are absent and therefore focus-inert.
    #[must_use]
    pub const fn is_focus_inert(self) -> bool {
        matches!(self, Self::Closed)
    }

    /// The shared motion recipe associated with a transition phase.
    #[must_use]
    pub const fn recipe(self) -> Option<MotionRecipe> {
        match self {
            Self::Opening => Some(MotionRecipe::AccordionExpand),
            Self::Closing => Some(MotionRecipe::AccordionCollapse),
            Self::Closed | Self::Open => None,
        }
    }

    /// Resolves a new phase from the previous phase and controlled target.
    ///
    /// Reversing an in-flight close creates a new `Opening` phase, and
    /// reversing an in-flight opening creates a new `Closing` phase. That
    /// directional truth table is what prevents a stale entrance phase from
    /// being replayed when a controlled value changes quickly.
    #[must_use]
    pub const fn transition(self, open: bool, animate: bool, motion_policy: MotionPolicy) -> Self {
        if !animate || matches!(motion_policy, MotionPolicy::Reduced) {
            return Self::settled(open);
        }

        match (self, open) {
            (Self::Closed, false) => Self::Closed,
            (Self::Open, true) => Self::Open,
            (Self::Closed | Self::Opening | Self::Closing, true) => Self::Opening,
            (Self::Opening | Self::Open | Self::Closing, false) => Self::Closing,
        }
    }
}

/// Small caller-owned state record for the lip-card phase machine.
///
/// This is not product composer state. A product boundary may retain this
/// copyable record alongside its controlled `open` value and call
/// [`Self::transition`] when that value changes. The generation is included in
/// GPUI animation IDs so each reversed transition gets a fresh animation
/// identity without timers or background tasks.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct LipCardState {
    phase: LipCardPhase,
    generation: u64,
}

impl LipCardState {
    /// Creates a settled state for the initial controlled value.
    #[must_use]
    pub const fn new(open: bool) -> Self {
        Self {
            phase: LipCardPhase::settled(open),
            generation: 0,
        }
    }

    /// Returns the current deterministic phase.
    #[must_use]
    pub const fn phase(self) -> LipCardPhase {
        self.phase
    }

    /// Returns the transition generation used for animation identity.
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.generation
    }

    /// Returns the controlled target represented by this state.
    #[must_use]
    pub const fn is_open(self) -> bool {
        self.phase.target_open()
    }

    /// Resolves the next phase and advances the generation only when the
    /// phase changes direction or enters a transition.
    #[must_use]
    pub fn transition(self, open: bool, animate: bool, motion_policy: MotionPolicy) -> Self {
        let phase = self.phase.transition(open, animate, motion_policy);
        let generation = if phase == self.phase {
            self.generation
        } else {
            self.generation.wrapping_add(1)
        };
        Self { phase, generation }
    }

    /// Settles an explicit transition at its current target without changing
    /// the transition generation.
    #[must_use]
    pub const fn settle(self) -> Self {
        Self {
            phase: LipCardPhase::settled(self.phase.target_open()),
            generation: self.generation,
        }
    }

    /// Whether the state is already at a settled phase.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        self.phase.is_settled()
    }
}

/// The GPUI-supported status of one visual effect in a lip-card motion plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LipCardEffectPlan {
    /// Resolve the effect at its final value immediately.
    Immediate,
    /// Animate the effect with the shared accordion clock.
    Animated,
    /// The pinned GPUI version has no corresponding visual primitive.
    UnsupportedByGpui,
}

/// The clipping behavior used by the panel wrapper.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LipCardClipPlan {
    /// Use GPUI's rectangular `overflow_hidden` content mask.
    StaticOverflowHidden,
}

/// The honest height behavior of the panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LipCardHeightPlan {
    /// A closed card has no panel element and therefore zero panel height.
    ZeroWhenClosed,
    /// The panel uses its natural layout height; GPUI does not interpolate it.
    NaturalLayout,
}

/// The resolved motion/effect plan for one lip-card phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LipCardMotionPlan {
    /// The requested phase before immediate-mode settling is applied.
    pub phase: LipCardPhase,
    /// The shared motion-policy decision.
    pub motion: MotionPlan,
    /// Opacity is the one animated visual effect supported here.
    pub opacity: LipCardEffectPlan,
    /// GPUI 0.2.2 has no blur/filter style primitive.
    pub blur: LipCardEffectPlan,
    /// The supported rectangular clip plan.
    pub clip: LipCardClipPlan,
    /// The explicit no-auto-height-interpolation height plan.
    pub height: LipCardHeightPlan,
}

impl LipCardMotionPlan {
    /// Resolves a phase using the explicit animate flag and shared policy.
    #[must_use]
    pub fn for_phase(phase: LipCardPhase, animate: bool, motion_policy: MotionPolicy) -> Self {
        let motion = match (animate, phase.recipe()) {
            (true, Some(recipe)) => motion_policy.resolve(recipe),
            (false, _) | (true, None) => MotionPlan::Immediate,
        };
        let opacity = if motion.animation().is_some() {
            LipCardEffectPlan::Animated
        } else {
            LipCardEffectPlan::Immediate
        };
        let effective_phase = if opacity == LipCardEffectPlan::Immediate {
            LipCardPhase::settled(phase.target_open())
        } else {
            phase
        };

        Self {
            phase,
            motion,
            opacity,
            blur: LipCardEffectPlan::UnsupportedByGpui,
            clip: LipCardClipPlan::StaticOverflowHidden,
            height: if effective_phase.panel_present() {
                LipCardHeightPlan::NaturalLayout
            } else {
                LipCardHeightPlan::ZeroWhenClosed
            },
        }
    }

    /// Returns the phase actually rendered after immediate-mode settling.
    #[must_use]
    pub const fn effective_phase(self) -> LipCardPhase {
        match self.motion {
            MotionPlan::Immediate => LipCardPhase::settled(self.phase.target_open()),
            MotionPlan::Animate(_) => self.phase,
        }
    }

    /// Returns the shared recipe selected by the requested phase.
    #[must_use]
    pub const fn recipe(self) -> Option<MotionRecipe> {
        self.phase.recipe()
    }

    /// Returns the runtime animation specification, if full motion selected.
    #[must_use]
    pub const fn animation(self) -> Option<MotionAnimation> {
        self.motion.animation()
    }

    /// Whether the rendered panel is mounted.
    #[must_use]
    pub const fn panel_present(self) -> bool {
        self.effective_phase().panel_present()
    }

    /// Whether the rendered panel should block pointer input.
    #[must_use]
    pub const fn is_pointer_inert(self) -> bool {
        self.effective_phase().is_pointer_inert()
    }

    /// Whether the rendered panel is structurally focus-inert.
    #[must_use]
    pub const fn is_focus_inert(self) -> bool {
        self.effective_phase().is_focus_inert()
    }
}

/// A controlled two-child lip card.
///
/// `style` is resolved by the caller from [`LipCardStyle::resolve`]. The
/// component owns only presentation: it does not handle events, persist
/// values, schedule timers, or infer a motion preference. Use
/// [`LipCardState`] when the caller wants an explicit open/close transition;
/// a plain `open` value is always rendered as a settled phase.
#[derive(IntoElement)]
pub struct LipCard {
    root: Div,
    lip: AnyElement,
    panel: AnyElement,
    style: LipCardStyle,
    open: bool,
    phase: LipCardPhase,
    animate: bool,
    motion_policy: MotionPolicy,
    transition_id: u64,
    debug_selector: Option<SharedString>,
}

impl LipCard {
    /// Constructs a settled controlled card from distinct lip and panel
    /// children. The panel is omitted when `open` is false.
    #[must_use]
    pub fn new(
        lip: impl IntoElement,
        panel: impl IntoElement,
        style: LipCardStyle,
        open: bool,
    ) -> Self {
        let root = root_for_style(&style);
        Self {
            root,
            lip: lip.into_any_element(),
            panel: panel.into_any_element(),
            style,
            open,
            phase: LipCardPhase::settled(open),
            animate: true,
            motion_policy: MotionPolicy::Full,
            transition_id: 0,
            debug_selector: None,
        }
    }

    /// Constructs a card by resolving the supplied theme and variant.
    #[must_use]
    pub fn from_theme(
        lip: impl IntoElement,
        panel: impl IntoElement,
        theme: ArtisanTheme,
        variant: LipCardVariant,
        open: bool,
    ) -> Self {
        Self::new(lip, panel, LipCardStyle::resolve(theme, variant), open)
    }

    /// Sets the controlled target and resets this render recipe to a settled
    /// phase. Use [`Self::with_state`] for a transition phase.
    #[must_use]
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self.phase = LipCardPhase::settled(open);
        self
    }

    /// Applies a caller-owned deterministic phase state, including its
    /// generation for animation identity.
    #[must_use]
    pub fn with_state(mut self, state: LipCardState) -> Self {
        self.open = state.is_open();
        self.phase = state.phase();
        self.transition_id = state.generation();
        self
    }

    /// Applies one explicit phase without inventing a transition.
    #[must_use]
    pub fn with_phase(mut self, phase: LipCardPhase) -> Self {
        self.open = phase.target_open();
        self.phase = phase;
        self
    }

    /// Enables or disables full-motion rendering for transition phases.
    #[must_use]
    pub const fn animate(mut self, animate: bool) -> Self {
        self.animate = animate;
        self
    }

    /// Selects the shared full/reduced motion policy.
    #[must_use]
    pub const fn motion_policy(mut self, motion_policy: MotionPolicy) -> Self {
        self.motion_policy = motion_policy;
        self
    }

    /// Overrides the transition generation used in the GPUI animation ID.
    #[must_use]
    pub const fn transition_id(mut self, transition_id: u64) -> Self {
        self.transition_id = transition_id;
        self
    }

    /// Uses a custom root debug selector. Child selectors receive `-lip` and
    /// `-panel` suffixes; with no override the three public stable constants
    /// are used.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns the controlled target supplied to this render recipe.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Returns the requested phase before immediate-mode settling.
    #[must_use]
    pub const fn phase(&self) -> LipCardPhase {
        self.phase
    }

    /// Returns the resolved visual style retained by this card.
    #[must_use]
    pub fn visual_style(&self) -> LipCardStyle {
        self.style.clone()
    }

    /// Resolves the motion plan for the current phase and flags.
    #[must_use]
    pub fn motion_plan(&self) -> LipCardMotionPlan {
        LipCardMotionPlan::for_phase(self.phase, self.animate, self.motion_policy)
    }
}

impl Styled for LipCard {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.root.style()
    }
}

impl RenderOnce for LipCard {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let plan = LipCardMotionPlan::for_phase(self.phase, self.animate, self.motion_policy);
        let effective_phase = plan.effective_phase();
        let selector = self.debug_selector.as_ref().map(ToString::to_string);
        let root_selector = selector
            .clone()
            .unwrap_or_else(|| LIP_CARD_ROOT_SELECTOR.to_string());
        let lip_selector = selector.as_deref().map_or_else(
            || LIP_CARD_LIP_SELECTOR.to_string(),
            |selector| format!("{selector}-lip"),
        );
        let panel_selector = selector.as_deref().map_or_else(
            || LIP_CARD_PANEL_SELECTOR.to_string(),
            |selector| format!("{selector}-panel"),
        );
        let transition_id = self.transition_id;
        let Self {
            mut root,
            lip,
            panel,
            ..
        } = self;

        root = root.debug_selector(move || root_selector);
        let lip = div()
            .flex_shrink_0()
            .debug_selector(move || lip_selector)
            .child(lip);
        root = root.child(lip);

        if plan.panel_present() {
            let mut panel = div()
                .relative()
                .flex()
                .flex_col()
                .overflow_hidden()
                .tab_stop(false)
                .debug_selector(move || panel_selector)
                .child(div().overflow_hidden().child(panel));

            if effective_phase.is_pointer_inert() {
                // This transparent, absolute hitbox is inserted after the
                // caller panel so a Closing phase cannot receive pointer
                // activation while its content is kept for the fade-out.
                panel = panel.child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .left(px(0.0))
                        .occlude(),
                );
            }

            root = root.child(animate_panel(panel, plan, transition_id));
        }

        root
    }
}

/// Returns a controlled card with the default settled motion settings.
#[must_use]
pub fn lip_card(
    lip: impl IntoElement,
    panel: impl IntoElement,
    style: LipCardStyle,
    open: bool,
) -> LipCard {
    LipCard::new(lip, panel, style, open)
}

fn root_for_style(style: &LipCardStyle) -> Div {
    let mut root = div()
        .flex()
        .flex_col()
        .rounded(style.corner_radius)
        .overflow_hidden()
        .shadow(style.card_shadows.clone());
    if let Some(background) = style.background {
        root = root.bg(background);
    }
    root
}

fn animate_panel(panel: Div, plan: LipCardMotionPlan, transition_id: u64) -> AnyElement {
    let Some(animation) = plan.animation() else {
        return panel.into_any_element();
    };

    let opening = matches!(plan.phase, LipCardPhase::Opening);
    let animation_id = format!(
        "artisan-lip-card-{}-{transition_id}",
        if opening { "opening" } else { "closing" }
    );
    let initial_opacity = if opening { 0.0 } else { 1.0 };

    panel
        .opacity(initial_opacity)
        .with_animation(
            ElementId::Name(animation_id.into()),
            animation.gpui_clock(),
            move |panel, progress| {
                let eased = smooth_out_sample(progress);
                let opacity = if opening { eased } else { 1.0 - eased };
                panel.opacity(opacity.clamp(0.0, 1.0))
            },
        )
        .into_any_element()
}

fn smooth_out_sample(progress: f32) -> f32 {
    if progress <= 0.0 {
        return 0.0;
    }
    if progress >= 1.0 {
        return 1.0;
    }

    let mut lower = 0.0_f32;
    let mut upper = 1.0_f32;
    for _ in 0..48 {
        let t = lower.midpoint(upper);
        if smooth_out_axis(t, 0.22, 0.36) < progress {
            lower = t;
        } else {
            upper = t;
        }
    }

    smooth_out_axis(lower.midpoint(upper), 1.0, 1.0)
}

fn smooth_out_axis(t: f32, first: f32, second: f32) -> f32 {
    let inverse = 1.0 - t;
    3.0 * inverse * inverse * t * first + 3.0 * inverse * t * t * second + t * t * t
}
