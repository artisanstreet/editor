//! Native GPUI onboarding wizard surface (`/onboarding`).
//!
//! Native counterpart of `routes/onboarding/+page.svelte` and
//! `routes/components/onboarding/view.svelte`, minus the Effect controllers,
//! streams, browser DOM, and navigation service, which the application owns.
//! Given one resolved [`OnboardingHarnessEntry`] per harness plus the
//! completion footer state, this module renders the centered wizard: the
//! `Set up your harnesses` heading, the three-column harness card grid, and
//! the Continue footer.
//!
//! Setup-button and Continue clicks are recorded as
//! [`OnboardingSetupClick`] values and a completion-request flag for the host
//! to drain; installation, authentication, browser, refresh, and navigation
//! effects stay out. The redirect-guard policy in
//! [`crate::onboarding_route`] is untouched by this surface.
//!
//! Fidelity notes (Tailwind/shader features without a GPUI equivalent):
//!
//! - `ShaderGlassSurface` rays (`ray_offset_x/y`, `ray_time_offset`) and the
//!   `color-mix(in oklab, …)` card/button blends have no GPUI equivalent.
//!   Cards use the theme `card` fill and setup buttons use the verbatim theme
//!   `foreground` fill; the per-harness `button_color` source string is
//!   retained on [`crate::onboarding_harness_presentation::HarnessCard`] for
//!   a later seam.
//! - The section gradient (`bg-linear-to-t` from `--onboarding-card-from` to
//!   `--onboarding-card-to`, driven by the changed-files style config) is a
//!   runtime value outside the theme, so the section uses the theme `popover`
//!   fill with a hairline border.
//! - The setup-label text-swap animation (`t-text-swap`, `is-exit`,
//!   `is-enter-start`) is static here; label/email concatenation reuses
//!   [`crate::setup_label_transition_policy::SetupLabelDisplayedValue`]
//!   semantics (one ASCII space before a present email).
//! - The busy icon swap (`t-icon-swap`) renders the loader glyph statically;
//!   GPUI has no CSS spin here.
//! - The experimental-support tooltip renders its badge inline with the exact
//!   help copy in [`ONBOARDING_EXPERIMENTAL_HELP_COPY`]; tooltip hosting
//!   stays with the application.
//! - `aria-busy` / `aria-disabled` / `role="status"` have no GPUI
//!   counterpart; every addressable node keeps a stable debug selector, and
//!   non-actionable buttons simply expose no click handler (full opacity,
//!   matching the legacy `cursor-default` treatment).
//! - `svelte:window onfocus` refresh stays with the host, which already owns
//!   the typed forced-refresh sequence
//!   ([`crate::onboarding_harness_presentation::HarnessRefreshAction`]).

use artisan_assets::AssetId;
use artisan_ui::icon::{IconSize, IconStyle, IconTint, icon};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::{
    ClickEvent, Context, Div, FontWeight, InteractiveElement as _, ParentElement as _, Render,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};

use crate::onboarding_harness_presentation::{
    CONTINUE_FOOTER_LABEL, HarnessCard, HarnessSetupAction, HarnessSetupIcon, HarnessSetupState,
    SAVING_FOOTER_LABEL, present_harness_card,
};

/// Debug selector for the onboarding screen root; matches
/// [`crate::native_route::NativeRoute::Onboarding::selector_suffix`].
pub const ONBOARDING_SCREEN_SELECTOR: &str = "route-onboarding";

/// Exact wizard heading (`view.svelte` header `h1`).
pub const ONBOARDING_TITLE: &str = "Set up your harnesses";

/// Exact document title (`+page.svelte` `svelte:head` title).
pub const ONBOARDING_WINDOW_TITLE: &str = "Set up Artisan";

/// Exact experimental-support help copy (tooltip body with the inline
/// `experimental` emphasis flattened to plain text).
pub const ONBOARDING_EXPERIMENTAL_HELP_COPY: &str =
    "This harness has experimental support and is not fully tested.";

/// Exact accessible name of the ready badge (`aria-label` on the installed
/// status check).
pub const ONBOARDING_INSTALLED_STATUS_COPY: &str = "Installed and signed in";

/// Legacy `grid-cols-3`: GPUI has a real grid, so the column count transfers
/// exactly instead of being approximated with wrapping rows.
pub const ONBOARDING_CARD_GRID_COLUMNS: u16 = 3;

/// Legacy `text-lg` (18 px) has no theme text-size token, so it is a named
/// constant (`theme.css` has no `text-lg` role; `dialog_title_text` is 16 px).
pub const ONBOARDING_TITLE_TEXT_PX: f32 = 18.0;

/// Legacy `rounded-2xl` (16 px) sits between ramp steps (`Xl` 14 px, `X2l`
/// 18 px), so it is a named constant rather than a forced ramp fit.
pub const ONBOARDING_SECTION_RADIUS_PX: f32 = 16.0;

/// Legacy `aspect-3/2 h-52`: 208 px tall, therefore 312 px wide.
pub const ONBOARDING_CARD_WIDTH_PX: f32 = 312.0;

/// Legacy `h-52` (13 spacing steps of 4 px).
pub const ONBOARDING_CARD_HEIGHT_PX: f32 = 208.0;

/// Legacy `max-w-64` description/failure width (64 spacing steps).
pub const ONBOARDING_DESCRIPTION_MAX_WIDTH_PX: f32 = 256.0;

/// One harness card plus its resolved renderer-facing setup state.
///
/// The setup state carries the exact legacy copy (label, email, failure) as
/// projected by the presentation policy; this surface only selects layout,
/// icons, and interaction from it.
#[derive(Clone, Debug, PartialEq)]
pub struct OnboardingHarnessEntry {
    /// The static catalog card (id, title, description, accents).
    pub card: HarnessCard,
    /// The resolved setup facts rendered into the card.
    pub setup: HarnessSetupState,
}

impl OnboardingHarnessEntry {
    /// Pairs one catalog card with its setup state without changing either.
    #[must_use]
    pub fn new(card: HarnessCard, setup: HarnessSetupState) -> Self {
        Self { card, setup }
    }
}

/// One setup-button activation recorded for the host to execute.
///
/// The host maps [`HarnessSetupAction`] onto the installation controller
/// (`install`, `authenticate`, `open_authorization`, `open_external_setup`)
/// exactly as `RunSetup` does; `None` actions never produce a click.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OnboardingSetupClick {
    /// The harness whose setup button was activated.
    pub harness_id: String,
    /// The setup operation represented by the button.
    pub action: HarnessSetupAction,
}

impl OnboardingSetupClick {
    /// Records one setup activation without normalizing either field.
    #[must_use]
    pub fn new(harness_id: impl Into<String>, action: HarnessSetupAction) -> Self {
        Self {
            harness_id: harness_id.into(),
            action,
        }
    }
}

/// Builds the stable selector for one harness card under the screen root.
#[must_use]
pub fn harness_card_selector(harness_id: &str) -> String {
    format!("{ONBOARDING_SCREEN_SELECTOR}-card-{harness_id}")
}

/// Builds the stable selector for one harness setup button.
#[must_use]
pub fn harness_setup_button_selector(harness_id: &str) -> String {
    format!("{ONBOARDING_SCREEN_SELECTOR}-setup-{harness_id}")
}

/// Resolves the provider brand glyph for a harness id.
///
/// The mapping mirrors `EngineMarkFor` (`lib/engine/presentation.ts`):
/// Codex wears the `OpenAI` mark, Claude/Cursor/Grok wear their product marks,
/// and `OpenCode`/`Hermes` wear their brand marks. Unknown ids fall back to
/// the neutral question-mark placeholder, matching `unknown_engine_mark`.
#[must_use]
pub fn brand_asset_for(harness_id: &str) -> AssetId {
    match harness_id {
        "codex" => AssetId::SVGL_OPENAI,
        "claude" => AssetId::SVGL_CLAUDE_AI,
        "cursor" => AssetId::SVGL_CURSOR,
        "grok" => AssetId::SVGL_GROK,
        "opencode2" => AssetId::BRANDS_OPENCODE,
        "hermes" => AssetId::BRANDS_HERMES,
        _ => AssetId::TABLER_QUESTION_MARK,
    }
}

/// Selects the setup-button glyph for one resolved setup state.
///
/// This matches the `HarnessSetupIcon` state enum with the exact legacy
/// `SetupIcon` precedence (ready, then install, then login); a busy state
/// swaps in the loader glyph exactly as the `t-icon-swap` `data-state="b"`
/// branch does.
#[must_use]
pub fn setup_icon_asset(setup: &HarnessSetupState) -> AssetId {
    if setup.busy {
        return AssetId::TABLER_LOADER_2;
    }
    match setup.icon() {
        HarnessSetupIcon::Ready => AssetId::TABLER_CIRCLE_CHECK,
        HarnessSetupIcon::Download => AssetId::TABLER_DOWNLOAD,
        HarnessSetupIcon::Login => AssetId::TABLER_LOGIN,
    }
}

/// Native onboarding wizard: heading, harness card grid, Continue footer.
///
/// Owns only what it paints (theme, per-harness entries, footer state) plus
/// the drained interaction flags. Data fetching, refresh, installation,
/// browser, and navigation stay with the application.
pub struct OnboardingScreen {
    theme: ArtisanTheme,
    harnesses: Vec<OnboardingHarnessEntry>,
    completion_saving: bool,
    completion_error: Option<String>,
    pending_setup_click: Option<OnboardingSetupClick>,
    completion_requested: bool,
}

impl OnboardingScreen {
    /// Builds the wizard from its theme and one entry per harness card.
    ///
    /// Entries render in the supplied order; callers pass catalog order (see
    /// [`crate::onboarding_harness_presentation::HarnessCatalog`]).
    #[must_use]
    pub fn new(theme: ArtisanTheme, harnesses: Vec<OnboardingHarnessEntry>) -> Self {
        Self {
            theme,
            harnesses,
            completion_saving: false,
            completion_error: None,
            pending_setup_click: None,
            completion_requested: false,
        }
    }

    /// Borrows the harness entries in render order.
    #[must_use]
    pub fn harnesses(&self) -> &[OnboardingHarnessEntry] {
        &self.harnesses
    }

    /// Replaces the harness entries wholesale (controller push path).
    pub fn replace_harnesses(&mut self, harnesses: Vec<OnboardingHarnessEntry>) {
        self.harnesses = harnesses;
    }

    /// Returns whether a completion save is currently in flight.
    #[must_use]
    pub const fn is_completion_saving(&self) -> bool {
        self.completion_saving
    }

    /// Marks a completion save as in flight or idle.
    pub fn set_completion_saving(&mut self, saving: bool) {
        self.completion_saving = saving;
    }

    /// Returns the exact visible completion error, if one is present.
    #[must_use]
    pub fn completion_error(&self) -> Option<&str> {
        self.completion_error.as_deref()
    }

    /// Sets the visible completion error without touching any other field.
    pub fn set_completion_error(&mut self, error: Option<String>) {
        self.completion_error = error;
    }

    /// Returns the exact footer label for the current saving state.
    #[must_use]
    pub fn footer_label(&self) -> &'static str {
        if self.completion_saving {
            SAVING_FOOTER_LABEL
        } else {
            CONTINUE_FOOTER_LABEL
        }
    }

    /// Drains the recorded setup click, if the host has not yet consumed it.
    pub fn take_pending_setup_click(&mut self) -> Option<OnboardingSetupClick> {
        self.pending_setup_click.take()
    }

    /// Drains the recorded Continue request (`true` exactly once per click).
    pub fn take_completion_request(&mut self) -> bool {
        std::mem::replace(&mut self.completion_requested, false)
    }

    /// Renders one harness card: title row, description, failure, button.
    fn render_harness_card(
        &self,
        entry: &OnboardingHarnessEntry,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = self.theme;
        let presentation = present_harness_card(&entry.card, &entry.setup);
        let card_selector = harness_card_selector(&entry.card.id);
        let button_selector = harness_setup_button_selector(&entry.card.id);

        let title_row = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(icon(IconStyle::resolve(
                theme,
                brand_asset_for(&entry.card.id),
                IconSize::Default,
                IconTint::Inherit,
            )))
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.foreground.to_paint())
                    .child(SharedString::from(entry.card.title.clone())),
            )
            .child(self.render_status_badge(entry));

        let mut upper = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(3.0))
            .child(title_row)
            .child(
                div()
                    .max_w(px(ONBOARDING_DESCRIPTION_MAX_WIDTH_PX))
                    .text_sm()
                    .line_height(px(20.0))
                    .text_color(theme.colors.foreground.to_paint())
                    .child(SharedString::from(entry.card.description.clone())),
            );
        if let Some(failure) = presentation.failure.clone() {
            upper = upper.child(
                div()
                    .max_w(px(ONBOARDING_DESCRIPTION_MAX_WIDTH_PX))
                    .text_xs()
                    .line_height(px(16.0))
                    .text_color(theme.colors.destructive.to_paint())
                    .child(SharedString::from(failure)),
            );
        }

        let button_face = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(theme.spacing.steps(2.0))
            .w_full()
            .h(theme.density.control_default)
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .bg(theme.colors.foreground.to_paint())
            .text_color(theme.colors.background.to_paint())
            .child(icon(IconStyle::resolve(
                theme,
                setup_icon_asset(&entry.setup),
                IconSize::Default,
                IconTint::Inherit,
            )))
            .child(Self::render_setup_label(entry));

        // Only actionable states expose a click handler; anything else is the
        // legacy non-actionable button (full opacity, no navigation effect).
        // `HarnessSetupAction::None` never reaches the listener because the
        // handler is attached only while `actionable` holds.
        let mut button = button_face
            .id(SharedString::from(button_selector.clone()))
            .debug_selector(move || button_selector.clone());
        if presentation.actionable {
            let harness_id = entry.card.id.clone();
            let action = presentation.action;
            button = button.on_click(cx.listener(move |view: &mut Self, _: &ClickEvent, _, cx| {
                view.pending_setup_click =
                    Some(OnboardingSetupClick::new(harness_id.clone(), action));
                cx.notify();
            }));
        }

        div()
            .id(SharedString::from(card_selector.clone()))
            .w(px(ONBOARDING_CARD_WIDTH_PX))
            .h(px(ONBOARDING_CARD_HEIGHT_PX))
            .flex_shrink_0()
            .rounded(RadiusTokens::value(RadiusStep::Xl))
            .bg(theme.colors.card.to_paint())
            .border_1()
            .border_color(theme.colors.border.to_paint())
            .debug_selector(move || card_selector.clone())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .justify_between()
                    .size_full()
                    .p(theme.spacing.steps(4.0))
                    .child(upper)
                    .child(button),
            )
    }

    /// Renders the title-row status badge: the ready check, the experimental
    /// badge, or nothing, with the exact legacy precedence (ready wins).
    fn render_status_badge(&self, entry: &OnboardingHarnessEntry) -> Div {
        let theme = self.theme;
        let presentation = present_harness_card(&entry.card, &entry.setup);
        if presentation.show_installed_status {
            let selector = format!("{}-installed", harness_card_selector(&entry.card.id));
            return div()
                .flex_shrink_0()
                .text_color(theme.colors.foreground.to_paint())
                .debug_selector(move || selector.clone())
                .child(icon(IconStyle::resolve(
                    theme,
                    AssetId::TABLER_CHECK,
                    IconSize::Compact,
                    IconTint::Inherit,
                )));
        }
        if presentation.show_experimental_help {
            let selector = format!("{}-experimental", harness_card_selector(&entry.card.id));
            return div()
                .flex_shrink_0()
                .text_color(theme.colors.muted_foreground.to_paint())
                .debug_selector(move || selector.clone())
                .child(icon(IconStyle::resolve(
                    theme,
                    AssetId::TABLER_TEST_PIPE,
                    IconSize::Compact,
                    IconTint::Inherit,
                )));
        }
        div()
    }

    /// Renders the setup-button label with its optional account email.
    ///
    /// The email follows one ASCII space after the label at 85% opacity,
    /// matching `setup-label.svelte` (`text-background/85`); the text-swap
    /// transition itself is a documented static gap.
    fn render_setup_label(entry: &OnboardingHarnessEntry) -> Div {
        let mut label = div()
            .flex()
            .items_center()
            .text_sm()
            .child(SharedString::from(entry.setup.label.clone()));
        if let Some(email) = entry.setup.email.clone() {
            label = label
                .child(" ")
                .child(div().opacity(0.85).child(SharedString::from(email)));
        }
        label
    }

    /// Renders the right-aligned footer: completion error plus Continue.
    fn render_footer(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme;
        let saving = self.completion_saving;
        let mut column = div()
            .flex()
            .flex_col()
            .items_end()
            .gap(theme.spacing.steps(2.0));
        if let Some(error) = self.completion_error.clone() {
            column = column.child(
                div()
                    .text_xs()
                    .text_color(theme.colors.destructive.to_paint())
                    .debug_selector(|| format!("{ONBOARDING_SCREEN_SELECTOR}-completion-error"))
                    .child(SharedString::from(error)),
            );
        }
        let mut continue_button = div()
            .id(SharedString::from(format!(
                "{ONBOARDING_SCREEN_SELECTOR}-continue"
            )))
            .debug_selector(|| format!("{ONBOARDING_SCREEN_SELECTOR}-continue"))
            .flex()
            .items_center()
            .gap(theme.spacing.steps(2.0))
            .h(theme.density.control_default)
            .px(theme.spacing.steps(4.0))
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .bg(theme.colors.foreground.to_paint())
            .text_color(theme.colors.background.to_paint())
            .text_sm()
            .child(SharedString::from(self.footer_label().to_owned()))
            .child(icon(IconStyle::resolve(
                theme,
                AssetId::TABLER_ARROW_RIGHT,
                IconSize::Default,
                IconTint::Inherit,
            )));
        // The legacy `disabled={completion_saving}` dims the button and
        // drops its click path; the in-flight label reads `Saving…`.
        if saving {
            continue_button = continue_button.opacity(0.5);
        } else {
            continue_button =
                continue_button.on_click(cx.listener(|view: &mut Self, _: &ClickEvent, _, cx| {
                    view.completion_requested = true;
                    cx.notify();
                }));
        }
        column = column.child(continue_button);
        div().flex().justify_end().child(column)
    }
}

impl Render for OnboardingScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let theme = self.theme;
        let entries = self.harnesses.clone();
        let mut grid = div()
            .grid()
            .grid_cols(ONBOARDING_CARD_GRID_COLUMNS)
            .gap(theme.spacing.steps(3.0))
            .debug_selector(|| format!("{ONBOARDING_SCREEN_SELECTOR}-grid"));
        for entry in &entries {
            grid = grid.child(self.render_harness_card(entry, cx));
        }

        div()
            .id(SharedString::from(ONBOARDING_SCREEN_SELECTOR))
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .overflow_y_scroll()
            .bg(theme.colors.background.to_paint())
            .text_color(theme.colors.foreground.to_paint())
            .p(theme.spacing.steps(6.0))
            .debug_selector(|| ONBOARDING_SCREEN_SELECTOR.to_owned())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.steps(8.0))
                    .rounded(px(ONBOARDING_SECTION_RADIUS_PX))
                    .bg(theme.colors.popover.to_paint())
                    .border_1()
                    .border_color(theme.colors.border.to_paint())
                    .p(theme.spacing.steps(8.0))
                    .child(
                        div()
                            .text_size(px(ONBOARDING_TITLE_TEXT_PX))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.colors.foreground.to_paint())
                            .debug_selector(|| format!("{ONBOARDING_SCREEN_SELECTOR}-title"))
                            .child(ONBOARDING_TITLE),
                    )
                    .child(grid)
                    .child(self.render_footer(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onboarding_harness_presentation::{
        CONTINUE_FOOTER_LABEL as POLICY_CONTINUE_LABEL, ONBOARDING_COMPLETION_FAILURE_MESSAGE,
        SAVING_FOOTER_LABEL as POLICY_SAVING_LABEL,
    };

    fn ready_setup() -> HarnessSetupState {
        HarnessSetupState::new(
            HarnessSetupAction::None,
            true,
            false,
            "Signed in",
            None,
            None,
        )
    }

    fn download_setup() -> HarnessSetupState {
        HarnessSetupState::new(
            HarnessSetupAction::Install,
            false,
            false,
            "Download",
            None,
            None,
        )
    }

    #[test]
    fn brand_assets_cover_the_catalog() {
        assert_eq!(brand_asset_for("codex"), AssetId::SVGL_OPENAI);
        assert_eq!(brand_asset_for("claude"), AssetId::SVGL_CLAUDE_AI);
        assert_eq!(brand_asset_for("cursor"), AssetId::SVGL_CURSOR);
        assert_eq!(brand_asset_for("grok"), AssetId::SVGL_GROK);
        assert_eq!(brand_asset_for("opencode2"), AssetId::BRANDS_OPENCODE);
        assert_eq!(brand_asset_for("hermes"), AssetId::BRANDS_HERMES);
        assert_eq!(brand_asset_for("unknown"), AssetId::TABLER_QUESTION_MARK);
    }

    #[test]
    fn setup_icon_matches_the_state_enum() {
        assert_eq!(
            setup_icon_asset(&ready_setup()),
            AssetId::TABLER_CIRCLE_CHECK
        );
        assert_eq!(
            setup_icon_asset(&download_setup()),
            AssetId::TABLER_DOWNLOAD
        );
        let login = HarnessSetupState::new(
            HarnessSetupAction::Authenticate,
            false,
            false,
            "Sign In",
            None,
            None,
        );
        assert_eq!(setup_icon_asset(&login), AssetId::TABLER_LOGIN);
        // Busy wins over every icon intent, matching the `t-icon-swap`
        // `data-state="b"` branch.
        let busy = HarnessSetupState::new(
            HarnessSetupAction::None,
            false,
            true,
            "Installing…",
            None,
            None,
        );
        assert_eq!(setup_icon_asset(&busy), AssetId::TABLER_LOADER_2);
        // Ready wins over install, matching `SetupIcon`.
        let ready_install = HarnessSetupState::new(
            HarnessSetupAction::Install,
            true,
            false,
            "Signed in",
            None,
            None,
        );
        assert_eq!(
            setup_icon_asset(&ready_install),
            AssetId::TABLER_CIRCLE_CHECK
        );
    }

    #[test]
    fn selectors_nest_under_the_screen_root() {
        assert_eq!(
            harness_card_selector("codex"),
            "route-onboarding-card-codex"
        );
        assert_eq!(
            harness_setup_button_selector("codex"),
            "route-onboarding-setup-codex"
        );
    }

    #[test]
    fn footer_labels_match_the_presentation_policy() {
        assert_eq!(CONTINUE_FOOTER_LABEL, POLICY_CONTINUE_LABEL);
        assert_eq!(SAVING_FOOTER_LABEL, POLICY_SAVING_LABEL);
    }

    #[test]
    fn copy_strings_are_exact() {
        assert_eq!(ONBOARDING_TITLE, "Set up your harnesses");
        assert_eq!(ONBOARDING_WINDOW_TITLE, "Set up Artisan");
        assert_eq!(
            ONBOARDING_EXPERIMENTAL_HELP_COPY,
            "This harness has experimental support and is not fully tested."
        );
        assert_eq!(ONBOARDING_INSTALLED_STATUS_COPY, "Installed and signed in");
        assert_eq!(
            ONBOARDING_COMPLETION_FAILURE_MESSAGE,
            "Onboarding could not be saved. Try again."
        );
    }

    #[test]
    fn completion_flags_drain_exactly_once() {
        let theme = ArtisanTheme::for_mode(artisan_ui::theme::ThemeMode::Dark);
        let mut screen = OnboardingScreen::new(theme, Vec::new());
        assert_eq!(screen.footer_label(), "Continue");
        assert!(!screen.take_completion_request());
        screen.set_completion_saving(true);
        assert_eq!(screen.footer_label(), "Saving…");
        screen.set_completion_error(Some("boom".to_owned()));
        assert_eq!(screen.completion_error(), Some("boom"));
        assert!(screen.take_pending_setup_click().is_none());
    }
}
