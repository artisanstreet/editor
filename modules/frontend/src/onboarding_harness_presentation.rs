//! Dependency-free presentation and completion policy for onboarding harnesses.
//!
//! This is the native value boundary for
//! `routes/components/onboarding/view.svelte`. It owns the static harness
//! catalog, derives renderer-facing setup facts, describes the forced refresh
//! sequence, and coordinates the completion save token. It does not access the
//! Effect runtime, browser DOM, transport, storage, or navigation service.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// The exact external setup documentation opened for Hermes.
pub const HERMES_SETUP_URL: &str =
    "https://hermes-agent.nousresearch.com/docs/user-guide/configuring-models";

/// The exact visible error used when the onboarding completion save fails.
pub const ONBOARDING_COMPLETION_FAILURE_MESSAGE: &str = "Onboarding could not be saved. Try again.";

/// The exact route requested after a successful onboarding completion save.
pub const ONBOARDING_COMPLETION_ROUTE: &str = "/";

/// The exact footer label rendered while the completion save is in flight.
pub const SAVING_FOOTER_LABEL: &str = "Saving…";

/// The exact footer label rendered when no completion save is in flight.
pub const CONTINUE_FOOTER_LABEL: &str = "Continue";

/// One static harness card from the onboarding catalog.
///
/// Text fields are owned so a native renderer can retain a card without any
/// dependency on the Svelte module lifetime. The ray coordinates are the
/// source values consumed by `ShaderGlassSurface`; this policy does not
/// interpret them as CSS or perform any visual calculation.
#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub struct HarnessCard {
    /// The stable engine/harness identifier.
    pub id: String,
    /// The exact card title.
    pub title: String,
    /// The exact card description.
    pub description: String,
    /// The exact source button color string.
    pub button_color: String,
    /// Whether the card shows experimental-support help when not ready.
    pub experimental: bool,
    /// The ray time/phase offset.
    pub phase: f64,
    /// The ray X offset.
    pub x: f64,
    /// The ray Y offset.
    pub y: f64,
    /// Whether setup uses the external Hermes authorization page.
    pub external_auth: bool,
}

impl HarnessCard {
    /// Creates an owned harness card without changing any supplied field.
    #[must_use = "use the constructed harness card"]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
        button_color: impl Into<String>,
        experimental: bool,
        phase: f64,
        x: f64,
        y: f64,
        external_auth: bool,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: description.into(),
            button_color: button_color.into(),
            experimental,
            phase,
            x,
            y,
            external_auth,
        }
    }

    /// Derives the renderer-facing setup facts for this card.
    #[must_use = "use the derived harness card presentation"]
    pub fn present(&self, setup: &HarnessSetupState) -> HarnessCardPresentation {
        present_harness_card(self, setup)
    }
}

/// The owned, ordered onboarding harness catalog.
///
/// [`Self::new`] always produces the six cards in the same order as the
/// legacy Svelte `harnesses` tuple. A caller can retain this value and use
/// [`Self::into_cards`] when it needs ownership of the complete card vector.
#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub struct HarnessCatalog {
    cards: Vec<HarnessCard>,
}

impl HarnessCatalog {
    /// Creates the exact six-card catalog in legacy presentation order.
    #[must_use = "use the constructed harness catalog"]
    pub fn new() -> Self {
        Self {
            cards: vec![
                HarnessCard::new(
                    "codex",
                    "Codex",
                    "OpenAI's terminal coding agent for GPT and Codex models.",
                    "#000000",
                    false,
                    0.0,
                    -0.4,
                    0.1,
                    false,
                ),
                HarnessCard::new(
                    "claude",
                    "Claude Code",
                    "Anthropic's terminal coding agent for Claude models.",
                    "#D97757",
                    false,
                    3.7,
                    0.25,
                    -0.2,
                    false,
                ),
                HarnessCard::new(
                    "cursor",
                    "Cursor",
                    "Cursor's CLI agent, using the models enabled on your account.",
                    "#1B1913",
                    true,
                    7.9,
                    0.55,
                    0.3,
                    false,
                ),
                HarnessCard::new(
                    "grok",
                    "Grok",
                    "xAI's Grok Build coding agent and model catalog.",
                    "#000000",
                    true,
                    11.4,
                    -0.15,
                    0.5,
                    false,
                ),
                HarnessCard::new(
                    "opencode2",
                    "OpenCode",
                    "Open-source terminal agent with built-in multi-provider support.",
                    "#211E1E",
                    true,
                    20.1,
                    -0.5,
                    -0.25,
                    false,
                ),
                HarnessCard::new(
                    "hermes",
                    "Hermes",
                    "Nous Research's terminal agent with tools, subagents, and provider profiles.",
                    "#0000F2",
                    true,
                    15.8,
                    0.4,
                    -0.45,
                    true,
                ),
            ],
        }
    }

    /// Borrows all cards in their exact presentation order.
    #[must_use = "use the borrowed harness cards"]
    pub fn cards(&self) -> &[HarnessCard] {
        &self.cards
    }

    /// Returns the number of cards in this catalog.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.cards.len()
    }

    /// Returns whether the catalog contains no cards.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// Returns the first card with exactly `id`, retaining its owned value.
    ///
    /// The lookup does not trim, fold case, or otherwise normalize the id.
    #[must_use]
    pub fn lookup(&self, id: &str) -> Option<HarnessCard> {
        self.cards.iter().find(|card| card.id == id).cloned()
    }

    /// Borrows the first card with exactly `id`, if one exists.
    #[must_use]
    pub fn lookup_ref(&self, id: &str) -> Option<&HarnessCard> {
        self.cards.iter().find(|card| card.id == id)
    }

    /// Returns forced refresh actions for this catalog without performing any
    /// installation or usage operation.
    #[must_use]
    pub fn forced_refresh_actions(&self) -> Vec<HarnessRefreshAction> {
        forced_refresh_actions_for(self.cards())
    }

    /// Transfers ownership of the ordered card vector to the caller.
    #[must_use]
    pub fn into_cards(self) -> Vec<HarnessCard> {
        self.cards
    }
}

impl Default for HarnessCatalog {
    fn default() -> Self {
        Self::new()
    }
}

/// Creates the exact owned onboarding harness catalog.
#[must_use = "use the constructed harness catalog"]
pub fn harness_catalog() -> HarnessCatalog {
    HarnessCatalog::new()
}

/// The setup operation exposed by a harness card.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum HarnessSetupAction {
    /// Install the harness before any authentication step.
    Install,
    /// Ask the installation adapter to begin authentication.
    Authenticate,
    /// Open an authorization URL already supplied by the installation state.
    OpenAuthorization,
    /// Open the fixed external setup page for the harness.
    OpenExternalSetup,
    /// No setup operation is currently available.
    #[default]
    None,
}

impl HarnessSetupAction {
    /// Returns the exact wire/UI spelling of this setup action.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Authenticate => "authenticate",
            Self::OpenAuthorization => "open_authorization",
            Self::OpenExternalSetup => "open_external_setup",
            Self::None => "none",
        }
    }

    /// Parses one exact setup-action spelling without normalization.
    #[must_use]
    pub fn from_raw(value: &str) -> Option<Self> {
        match value {
            "install" => Some(Self::Install),
            "authenticate" => Some(Self::Authenticate),
            "open_authorization" => Some(Self::OpenAuthorization),
            "open_external_setup" => Some(Self::OpenExternalSetup),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    /// Returns whether the action admits a setup click.
    #[must_use]
    pub const fn is_actionable(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// The renderer icon selected for one setup state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HarnessSetupIcon {
    /// The harness is ready and uses the check icon.
    Ready,
    /// Installation is the next setup operation and uses the download icon.
    Download,
    /// Any other non-ready setup operation uses the login icon.
    Login,
}

impl HarnessSetupIcon {
    /// Returns the exact icon intent name used by the legacy view.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Download => "download",
            Self::Login => "login",
        }
    }
}

/// The setup values consumed by a harness card renderer.
///
/// All text is retained as supplied. In particular, an empty `Some` value is
/// distinct from `None`, and whitespace or Unicode is never trimmed or
/// normalized.
#[must_use]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HarnessSetupState {
    /// The typed operation represented by the setup button.
    pub action: HarnessSetupAction,
    /// Whether installation and sign-in are complete.
    pub ready: bool,
    /// Whether the setup button is currently busy.
    pub busy: bool,
    /// The exact primary button label.
    pub label: String,
    /// The optional exact account email shown after the label.
    pub email: Option<String>,
    /// The optional exact failure text shown below the description.
    pub failure: Option<String>,
}

impl HarnessSetupState {
    /// Creates setup state while preserving every supplied value.
    #[must_use = "use the constructed setup state"]
    pub fn new(
        action: HarnessSetupAction,
        ready: bool,
        busy: bool,
        label: impl Into<String>,
        email: Option<String>,
        failure: Option<String>,
    ) -> Self {
        Self {
            action,
            ready,
            busy,
            label: label.into(),
            email,
            failure,
        }
    }

    /// Returns whether this setup state admits a button action.
    #[must_use]
    pub const fn is_actionable(&self) -> bool {
        self.action.is_actionable()
    }

    /// Derives the icon with the legacy ready-then-install precedence.
    #[must_use]
    pub const fn icon(&self) -> HarnessSetupIcon {
        setup_icon_for(self.ready, self.action)
    }
}

/// Renderer-facing facts derived from one card and its setup state.
///
/// The booleans intentionally mirror separate DOM/accessibility decisions:
/// installed status, experimental help, busy state, aria-disabled, the
/// cursor class, and actionable hover behavior are not interchangeable.
#[must_use]
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessCardPresentation {
    /// The setup operation represented by the button.
    pub action: HarnessSetupAction,
    /// The icon intent selected from ready state and action.
    pub icon: HarnessSetupIcon,
    /// Whether the harness is ready.
    pub ready: bool,
    /// Whether the installed/signed-in status should be rendered.
    pub show_installed_status: bool,
    /// Whether experimental-support help should be rendered.
    pub show_experimental_help: bool,
    /// Whether the setup button should expose its busy state.
    pub busy: bool,
    /// Whether the setup action is actionable.
    pub actionable: bool,
    /// The renderer value for `aria-disabled`.
    pub aria_disabled: bool,
    /// Whether the non-actionable cursor-default class should be applied.
    pub cursor_default: bool,
    /// Whether the actionable hover behavior should be applied.
    pub hover_enabled: bool,
    /// The exact setup label.
    pub label: String,
    /// The exact optional email.
    pub email: Option<String>,
    /// The exact optional failure text.
    pub failure: Option<String>,
}

impl HarnessCardPresentation {
    /// Returns the label and optional email exactly as the setup-label view
    /// concatenates them, including a separator for an empty email.
    #[must_use]
    pub fn rendered_label(&self) -> String {
        let email_len = self.email.as_ref().map_or(0, String::len);
        let separator_len = usize::from(self.email.is_some());
        let mut rendered = String::with_capacity(self.label.len() + separator_len + email_len);
        rendered.push_str(&self.label);
        if let Some(email) = &self.email {
            rendered.push(' ');
            rendered.push_str(email);
        }
        rendered
    }
}

/// Computes the setup icon using the exact legacy precedence.
#[must_use]
pub const fn setup_icon_for(ready: bool, action: HarnessSetupAction) -> HarnessSetupIcon {
    if ready {
        HarnessSetupIcon::Ready
    } else if matches!(action, HarnessSetupAction::Install) {
        HarnessSetupIcon::Download
    } else {
        HarnessSetupIcon::Login
    }
}

/// Computes whether a setup action admits a click.
#[must_use]
pub const fn setup_is_actionable(action: HarnessSetupAction) -> bool {
    action.is_actionable()
}

/// Derives all renderer-facing setup facts without performing an effect.
#[must_use = "use the derived harness card presentation"]
pub fn present_harness_card(
    card: &HarnessCard,
    setup: &HarnessSetupState,
) -> HarnessCardPresentation {
    let actionable = setup_is_actionable(setup.action);
    HarnessCardPresentation {
        action: setup.action,
        icon: setup_icon_for(setup.ready, setup.action),
        ready: setup.ready,
        show_installed_status: setup.ready,
        show_experimental_help: !setup.ready && card.experimental,
        busy: setup.busy,
        actionable,
        aria_disabled: !actionable,
        cursor_default: !actionable,
        hover_enabled: actionable,
        label: setup.label.clone(),
        email: setup.email.clone(),
        failure: setup.failure.clone(),
    }
}

/// One typed operation in the forced onboarding refresh sequence.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessRefreshAction {
    /// Refresh all installation reports first.
    RefreshInstallations,
    /// Force-load usage once for one harness.
    LoadUsage {
        /// The owned harness id passed to the usage adapter.
        harness_id: String,
        /// Whether the usage load bypasses the cache; forced refreshes use
        /// `true`.
        force: bool,
    },
}

/// Returns installation refresh followed by one forced usage load per card.
///
/// The input order is preserved exactly and every returned harness id is
/// owned by its action. No adapter operation is performed here.
#[must_use]
pub fn forced_refresh_actions_for(cards: &[HarnessCard]) -> Vec<HarnessRefreshAction> {
    let mut actions = Vec::with_capacity(cards.len() + 1);
    actions.push(HarnessRefreshAction::RefreshInstallations);
    actions.extend(cards.iter().map(|card| HarnessRefreshAction::LoadUsage {
        harness_id: card.id.clone(),
        force: true,
    }));
    actions
}

/// Returns the exact forced refresh sequence for the canonical catalog.
#[must_use]
pub fn forced_refresh_actions() -> Vec<HarnessRefreshAction> {
    harness_catalog().forced_refresh_actions()
}

/// An opaque monotonic identity for one completion save request.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CompletionSaveToken(u64);

impl CompletionSaveToken {
    /// Creates a token from an adapter-retained numeric identity.
    #[must_use = "use the constructed completion save token"]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric identity carried by this token.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for CompletionSaveToken {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

/// The result delivered by the completion-save adapter.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CompletionOutcome {
    /// The adapter confirmed the save.
    Succeeded,
    /// The adapter reported that the save failed.
    Failed,
}

/// One ordered adapter action emitted by the completion controller.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletionAction {
    /// Persist the exact onboarding completion field for the current token.
    SaveDefaults {
        /// The request token used to correlate the adapter result.
        token: CompletionSaveToken,
        /// The exact field requested by the legacy defaults controller.
        onboarding_completed: bool,
    },
    /// Navigate to the exact path after a successful save.
    Navigate {
        /// The owned route path passed to the navigation adapter.
        path: String,
    },
}

/// An ordered, possibly empty completion-controller transition.
#[must_use = "execute the returned onboarding adapter actions"]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompletionTransition {
    token: Option<CompletionSaveToken>,
    actions: Vec<CompletionAction>,
}

impl CompletionTransition {
    /// Creates a transition from an optional request token and ordered actions.
    #[must_use = "use the constructed completion transition"]
    pub fn new(token: Option<CompletionSaveToken>, actions: Vec<CompletionAction>) -> Self {
        Self { token, actions }
    }

    /// Returns the newly admitted token, if this transition admitted a save.
    #[must_use]
    pub const fn token(&self) -> Option<CompletionSaveToken> {
        self.token
    }

    /// Borrows adapter actions in their exact execution order.
    #[must_use = "inspect the ordered completion actions"]
    pub fn actions(&self) -> &[CompletionAction] {
        &self.actions
    }

    /// Transfers ownership of adapter actions in their exact execution order.
    #[must_use]
    pub fn into_actions(self) -> Vec<CompletionAction> {
        self.actions
    }

    /// Returns whether this transition contains no adapter action.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Returns the number of adapter actions in this transition.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.actions.len()
    }
}

/// Dependency-free controller for the onboarding completion save.
///
/// The controller owns visible error and saving state. Callers execute the
/// returned [`CompletionAction`] values, then pass the matching token and
/// adapter outcome to [`Self::settle`]. A late token cannot alter state.
#[must_use]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OnboardingCompletionController {
    saving: bool,
    visible_error: Option<String>,
    next_token: u64,
    current_token: Option<CompletionSaveToken>,
}

impl OnboardingCompletionController {
    /// Creates an idle controller with no visible error.
    #[must_use = "use the constructed completion controller"]
    pub const fn new() -> Self {
        Self {
            saving: false,
            visible_error: None,
            next_token: 0,
            current_token: None,
        }
    }

    /// Returns whether a completion save is currently in flight.
    #[must_use]
    pub const fn is_saving(&self) -> bool {
        self.saving
    }

    /// Returns the exact visible error, if one is present.
    #[must_use]
    pub fn visible_error(&self) -> Option<&str> {
        self.visible_error.as_deref()
    }

    /// Returns the token of the current save, if one is in flight.
    #[must_use]
    pub const fn current_token(&self) -> Option<CompletionSaveToken> {
        self.current_token
    }

    /// Returns the latest allocated numeric token.
    ///
    /// The initial value is zero; the first admitted request receives one.
    #[must_use]
    pub const fn next_token(&self) -> u64 {
        self.next_token
    }

    /// Returns the exact footer label for the current saving state.
    #[must_use]
    pub const fn footer_label(&self) -> &'static str {
        if self.saving {
            SAVING_FOOTER_LABEL
        } else {
            CONTINUE_FOOTER_LABEL
        }
    }

    /// Admits one completion request and emits its save action.
    ///
    /// Admission clears any visible error, marks the controller as saving,
    /// allocates the next monotonic token, and emits exactly one
    /// `onboarding_completed=true` adapter action. A request while saving is
    /// an empty transition and leaves every field unchanged. If the finite
    /// token space is exhausted, no wrapping token is reused and the request
    /// is also empty.
    #[must_use = "execute the returned completion adapter actions"]
    pub fn request_completion(&mut self) -> CompletionTransition {
        if self.saving {
            return CompletionTransition::default();
        }

        let Some(next_token) = self.next_token.checked_add(1) else {
            return CompletionTransition::default();
        };

        let token = CompletionSaveToken::new(next_token);
        self.next_token = next_token;
        self.current_token = Some(token);
        self.saving = true;
        self.visible_error = None;

        CompletionTransition::new(
            Some(token),
            vec![CompletionAction::SaveDefaults {
                token,
                onboarding_completed: true,
            }],
        )
    }

    /// Settles a save only when `token` is the current token.
    ///
    /// A stale token is inert and returns an empty transition. Settling the
    /// current token always clears saving and the current token. Failure sets
    /// the exact visible error and emits no navigation. Success clears the
    /// error and emits one owned navigation action for `/`.
    #[must_use = "execute the returned completion adapter actions"]
    pub fn settle(
        &mut self,
        token: CompletionSaveToken,
        outcome: CompletionOutcome,
    ) -> CompletionTransition {
        if self.current_token != Some(token) {
            return CompletionTransition::default();
        }

        self.current_token = None;
        self.saving = false;

        match outcome {
            CompletionOutcome::Succeeded => {
                self.visible_error = None;
                CompletionTransition::new(
                    None,
                    vec![CompletionAction::Navigate {
                        path: String::from(ONBOARDING_COMPLETION_ROUTE),
                    }],
                )
            }
            CompletionOutcome::Failed => {
                self.visible_error = Some(String::from(ONBOARDING_COMPLETION_FAILURE_MESSAGE));
                CompletionTransition::default()
            }
        }
    }
}
