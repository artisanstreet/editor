//! Dependency-free policy for the thread-title setting.
//!
//! The legacy settings surface keeps the session-defaults controller as the
//! source of truth for availability and the selected title mode. This leaf
//! models only the values and transitions that the surface needs: a caller
//! supplies authoritative controller updates, executes the returned command,
//! and settles the local save flight. Streams, transport, rendering, and
//! asynchronous execution stay at that adapter boundary.
//!
//! Availability affects the rendered disabled state, but it is intentionally
//! not an admission guard here. The Svelte handler guards only its local
//! `saving` bit; its disabled control normally prevents an unavailable click.

#![allow(clippy::module_name_repetitions)]

/// The exact reader-facing message shown when Forge does not confirm a save.
pub const THREAD_TITLE_SETTINGS_SAVE_FAILURE_MESSAGE: &str =
    "Couldn't verify the new default. Forge did not confirm the change.";

/// A decoded thread-title mode, including raw modes added by a newer schema.
///
/// The current protocol recognizes `summary` and `latest_message`. Unknown
/// values are retained verbatim so an adapter can preserve them, while the
/// presentation policy treats them as non-summary modes.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub enum ThreadTitleMode {
    /// Prefer the harness-generated summary title when one is available.
    #[default]
    Summary,
    /// Use the latest user-message title.
    LatestMessage,
    /// A future or otherwise unrecognized raw mode, preserved verbatim.
    Unknown(String),
}

impl ThreadTitleMode {
    /// The recognized protocol modes in their canonical order.
    pub const ALL: [Self; 2] = [Self::Summary, Self::LatestMessage];

    /// Classifies one exact raw protocol value without trimming or folding it.
    ///
    /// Every value other than the two current literals becomes
    /// [`Self::Unknown`] and retains the supplied UTF-8 text exactly.
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        match raw {
            "summary" => Self::Summary,
            "latest_message" => Self::LatestMessage,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Classifies an owned raw value without cloning an unknown mode.
    #[must_use]
    pub fn from_owned(raw: String) -> Self {
        match raw.as_str() {
            "summary" => Self::Summary,
            "latest_message" => Self::LatestMessage,
            _ => Self::Unknown(raw),
        }
    }

    /// Returns the exact raw value represented by this mode.
    #[must_use]
    pub fn as_raw(&self) -> &str {
        match self {
            Self::Summary => "summary",
            Self::LatestMessage => "latest_message",
            Self::Unknown(raw) => raw,
        }
    }

    /// Returns the exact raw value represented by this mode.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.as_raw()
    }

    /// Returns an owned raw value without normalizing unknown modes.
    #[must_use]
    pub fn into_raw(self) -> String {
        match self {
            Self::Summary => String::from("summary"),
            Self::LatestMessage => String::from("latest_message"),
            Self::Unknown(raw) => raw,
        }
    }

    /// Returns whether this is exactly the current `summary` mode.
    #[must_use]
    pub const fn is_summary(&self) -> bool {
        matches!(self, Self::Summary)
    }
}

impl From<&str> for ThreadTitleMode {
    fn from(raw: &str) -> Self {
        Self::from_raw(raw)
    }
}

impl From<String> for ThreadTitleMode {
    fn from(raw: String) -> Self {
        Self::from_owned(raw)
    }
}

/// The authoritative fields consumed from one session-defaults snapshot.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ThreadTitleSettingsAuthoritativeState {
    /// Whether the Forge-backed session-defaults surface is available.
    pub available: bool,
    /// The exact current thread-title mode from the defaults snapshot.
    pub thread_title_mode: ThreadTitleMode,
}

impl ThreadTitleSettingsAuthoritativeState {
    /// Creates one authoritative snapshot without performing I/O or coercion.
    #[must_use]
    pub const fn new(available: bool, thread_title_mode: ThreadTitleMode) -> Self {
        Self {
            available,
            thread_title_mode,
        }
    }

    /// Borrows the current mode without copying an unknown raw value.
    #[must_use]
    pub fn mode(&self) -> &ThreadTitleMode {
        &self.thread_title_mode
    }
}

/// The typed persistence command emitted by an admitted toggle.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ThreadTitleSettingsPersistenceCommand {
    /// Persist the exact mode requested by the switch interaction.
    SetThreadTitleMode {
        /// The mode passed to the session-defaults controller.
        mode: ThreadTitleMode,
    },
}

impl ThreadTitleSettingsPersistenceCommand {
    /// Returns the exact requested mode by shared reference.
    #[must_use]
    pub fn mode(&self) -> &ThreadTitleMode {
        match self {
            Self::SetThreadTitleMode { mode } => mode,
        }
    }

    /// Returns the exact requested mode by shared reference.
    #[must_use]
    pub fn requested_mode(&self) -> &ThreadTitleMode {
        self.mode()
    }

    /// Consumes the command and returns its exact requested mode.
    #[must_use]
    pub fn into_mode(self) -> ThreadTitleMode {
        match self {
            Self::SetThreadTitleMode { mode } => mode,
        }
    }

    /// Returns the controller operation represented by this command.
    #[must_use]
    pub const fn operation() -> &'static str {
        "session.defaults.update"
    }
}

/// Why a toggle was not admitted.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThreadTitleSettingsSaveRejection {
    /// A previous save is still in flight.
    Saving,
}

/// The result observed when an admitted save completes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThreadTitleSettingsSaveOutcome {
    /// Forge confirmed the requested persistence operation.
    Succeeded,
    /// Forge did not confirm the requested persistence operation.
    Failed,
}

/// Authoritative thread-title settings plus local presentation state.
///
/// `available` and `thread_title_mode` are replaced only by
/// [`Self::apply_authoritative_update`]. An admitted toggle never changes the
/// mode optimistically. `saving` and `message` are local to this surface and
/// therefore survive streamed authoritative updates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadTitleSettingsState {
    /// Whether the Forge-backed session-defaults surface is available.
    pub available: bool,
    /// The exact current authoritative thread-title mode.
    pub thread_title_mode: ThreadTitleMode,
    /// Whether an admitted persistence command is in flight.
    pub saving: bool,
    /// Reader-facing status text; empty means that no status is shown.
    pub message: String,
}

impl ThreadTitleSettingsState {
    /// Creates local state from an authoritative session-defaults snapshot.
    #[must_use]
    pub fn new(authoritative: ThreadTitleSettingsAuthoritativeState) -> Self {
        Self {
            available: authoritative.available,
            thread_title_mode: authoritative.thread_title_mode,
            saving: false,
            message: String::new(),
        }
    }

    /// Returns a cloned authoritative snapshot for an adapter or test.
    #[must_use]
    pub fn authoritative(&self) -> ThreadTitleSettingsAuthoritativeState {
        ThreadTitleSettingsAuthoritativeState {
            available: self.available,
            thread_title_mode: self.thread_title_mode.clone(),
        }
    }

    /// Borrows the current mode without copying an unknown raw value.
    #[must_use]
    pub fn mode(&self) -> &ThreadTitleMode {
        &self.thread_title_mode
    }

    /// Returns the derived checked state of the summary switch.
    ///
    /// Only the exact current `summary` mode is checked. Unknown future modes
    /// deliberately remain unchecked rather than being normalized.
    #[must_use]
    pub const fn summarized(&self) -> bool {
        self.thread_title_mode.is_summary()
    }

    /// Alias for [`Self::summarized`] for callers using predicate naming.
    #[must_use]
    pub const fn is_summarized(&self) -> bool {
        self.summarized()
    }

    /// Returns the derived disabled state rendered by the switch.
    #[must_use]
    pub const fn switch_disabled(&self) -> bool {
        !self.available || self.saving
    }

    /// Alias for [`Self::switch_disabled`] matching the view's presentation
    /// vocabulary.
    #[must_use]
    pub const fn disabled(&self) -> bool {
        self.switch_disabled()
    }

    /// Returns whether the source handler's admission guard will accept a
    /// toggle.
    ///
    /// This intentionally ignores `available`: the legacy handler checks only
    /// `saving`; availability is a presentation concern owned by the switch.
    #[must_use]
    pub const fn can_admit_toggle(&self) -> bool {
        !self.saving
    }

    /// Admits a toggle and returns the exact mode requested by it.
    ///
    /// The `enabled` argument is the switch's next checked value. Admission
    /// clears the previous message and starts the local save flight, but does
    /// not change the authoritative mode. Availability is intentionally not a
    /// guard; see [`Self::can_admit_toggle`].
    ///
    /// # Errors
    ///
    /// Returns [`ThreadTitleSettingsSaveRejection::Saving`] when another save
    /// is already in flight. In that case all local state is unchanged.
    pub fn admit_toggle(
        &mut self,
        enabled: bool,
    ) -> Result<ThreadTitleSettingsPersistenceCommand, ThreadTitleSettingsSaveRejection> {
        if self.saving {
            return Err(ThreadTitleSettingsSaveRejection::Saving);
        }

        self.message.clear();
        self.saving = true;
        let mode = if enabled {
            ThreadTitleMode::Summary
        } else {
            ThreadTitleMode::LatestMessage
        };
        Ok(ThreadTitleSettingsPersistenceCommand::SetThreadTitleMode { mode })
    }

    /// Starts the same source toggle handler under save-oriented naming.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::admit_toggle`]'s concurrent-save rejection.
    pub fn start_toggle(
        &mut self,
        enabled: bool,
    ) -> Result<ThreadTitleSettingsPersistenceCommand, ThreadTitleSettingsSaveRejection> {
        self.admit_toggle(enabled)
    }

    /// Admits the inverse of the current rendered checked state.
    ///
    /// This is the no-argument form used by a switch adapter that has already
    /// read [`Self::summarized`].
    ///
    /// # Errors
    ///
    /// Propagates [`Self::admit_toggle`]'s concurrent-save rejection.
    pub fn toggle(
        &mut self,
    ) -> Result<ThreadTitleSettingsPersistenceCommand, ThreadTitleSettingsSaveRejection> {
        self.admit_toggle(!self.summarized())
    }

    /// Applies a complete streamed session-defaults replacement.
    ///
    /// The supplied availability and mode replace the authoritative fields
    /// exactly. Local `saving` and `message` state is preserved because stream
    /// delivery and save completion are separate observations in the legacy
    /// surface.
    pub fn apply_authoritative_update(&mut self, next: ThreadTitleSettingsAuthoritativeState) {
        self.available = next.available;
        self.thread_title_mode = next.thread_title_mode;
    }

    /// Settles a save and always clears the local in-flight bit.
    ///
    /// Success leaves the message as-is, matching the legacy effect after a
    /// newly admitted save has already cleared it. Failure replaces the
    /// message with [`THREAD_TITLE_SETTINGS_SAVE_FAILURE_MESSAGE`]. Neither
    /// outcome fabricates a new authoritative mode.
    pub fn finish_save(&mut self, outcome: ThreadTitleSettingsSaveOutcome) {
        self.saving = false;
        if matches!(outcome, ThreadTitleSettingsSaveOutcome::Failed) {
            THREAD_TITLE_SETTINGS_SAVE_FAILURE_MESSAGE.clone_into(&mut self.message);
        }
    }

    /// Settles a confirmed save.
    pub fn save_succeeded(&mut self) {
        self.finish_save(ThreadTitleSettingsSaveOutcome::Succeeded);
    }

    /// Settles a failed save with the exact reader-facing message.
    pub fn save_failed(&mut self) {
        self.finish_save(ThreadTitleSettingsSaveOutcome::Failed);
    }
}
