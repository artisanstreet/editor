//! Static paint facts for the composer's accessory surfaces.
//!
//! This is the pure counterpart of three legacy composer leaves:
//!
//! - `routes/components/composer/attachment-tray.svelte` for the
//!   attachment-tray thumbnail row,
//! - `routes/components/composer/steering-lip.svelte` with
//!   `routes/components/composer/queued-steer.ts` for queued-steer rows,
//! - `routes/components/composer/controls.svelte` with the native send face
//!   in `native_composer.rs` for readiness gating.
//!
//! Every value here is fixture or static render data: ordered labels, icon
//! identities, layout tokens, and resolved [`artisan_ui`] recipes. There is
//! intentionally no behavior and no state: no callbacks, no submission gate,
//! no draft session, no clock, no timers, and no animation driver. A renderer
//! paints the returned facts and owns all interaction, timing, image bytes,
//! and preview references.
//!
//! Copy is byte-identical to the legacy strings. Attachment labels reuse
//! [`crate::attachment_tray_policy`] so the `View <name>` / `Remove <name>`
//! facts cannot drift, and fixture attachment keys are minted by a real
//! [`crate::scoped_attachment_queue::ScopedAttachmentQueue`] so they keep the
//! `attachment:<n>` token shape.
//!
//! Motion is described, never driven. Queued-steer rows carry the
//! `lip-row-grow` endpoints from `docs/ui/INVENTORY.md` §5.8
//! (`lib/styles/animations.css:292-296`, `lib/styles/utilities.css:522-531`):
//! each row owns a grid track that grows from `0fr` so the lip's height
//! tweens instead of jumping when a newer steer lands on top. Timing tokens
//! stay renderer-owned; under the reduced-motion authority they collapse to
//! an immediate reveal with no code change here.

use artisan_assets::AssetId;
use artisan_ui::{
    button::{AccessibleLabel, ButtonContent, ButtonSize, ButtonStyle, ButtonVariant},
    icon::{IconSize, IconStyle, IconTint},
    motion::MotionPolicy,
    progress::{ProgressFraction, ProgressStyle},
    theme::{ArtisanTheme, ThemeMode},
};

use crate::{
    attachment_tray_policy::{AttachmentFact, AttachmentTrayRow},
    scoped_attachment_queue::ScopedAttachmentQueue,
};

/// Accessible name of the attachment tray
/// (`attachment-tray.svelte:26`).
pub const ATTACHMENT_TRAY_LABEL: &str = "Attached images";

/// Fallback steer label when the queued text trims to nothing
/// (`thread-composer.svelte:619`).
pub const QUEUED_STEER_EMPTY_TEXT: &str = "Attached image";

/// Accessible name and title of the steer edit control
/// (`steering-lip.svelte:39-40`).
pub const EDIT_QUEUED_MESSAGE_LABEL: &str = "Edit queued message";

/// Accessible name and title of the steer discard control
/// (`steering-lip.svelte:49-50`).
pub const DISCARD_QUEUED_MESSAGE_LABEL: &str = "Discard queued message";

/// Accessible name of the send control while idle
/// (`controls.svelte:160`).
pub const SEND_MESSAGE_LABEL: &str = "Send message";

/// Accessible name of the send control while a run holds it
/// (`controls.svelte:160`).
pub const STOP_RUN_LABEL: &str = "Stop current run";

/// Label of the run-time escape-hatch action
/// (`controls.svelte:141`).
pub const START_NEW_THREAD_PROMPT_LABEL: &str = "Start a new thread with prompt";

/// Native send button text while idle (`native_composer.rs:580`).
pub const SEND_BUTTON_TEXT: &str = "Send";

/// Native send button text while a submission is in flight
/// (`native_composer.rs:580`).
pub const SENDING_BUTTON_TEXT: &str = "Sending…";

/// Blocked reason when no engine is reachable
/// (`lib/composer/send-readiness.ts:13`).
pub const FORGE_OFFLINE_REASON: &str = "Forge is offline — reconnect to send";

/// Blocked reason used when a composed message cannot leave yet
/// (`thread-composer.svelte:371`).
pub const PREPARING_TO_SEND_REASON: &str =
    "This surface is still preparing to send. Your message is kept in the composer.";

/// Title reported when a send is refused (`thread-composer.svelte:434`).
pub const SEND_FAILURE_TITLE: &str = "Could not send message";

/// Title reported when the new-thread escape hatch is refused
/// (`thread-composer.svelte:450-453`).
pub const NEW_THREAD_FAILURE_TITLE: &str = "Could not start a new thread";

/// Refusal used when no route can recall a queued steer
/// (`thread-composer.svelte:144`).
pub const RECALL_UNAVAILABLE_MESSAGE: &str = "This surface cannot recall a queued message.";

/// Title reported when recalling a queued steer is refused
/// (`composer/queued-steer.ts:34`).
pub const DISCARD_QUEUED_FAILURE_TITLE: &str = "Could not discard the queued message";

/// Live-region role of one queued-steer row (`steering-lip.svelte:30`).
pub const QUEUED_STEER_ROLE: &str = "status";

/// Keyframe a queued-steer row plays on mount
/// (`lib/styles/animations.css:292-296`).
pub const LIP_ROW_GROW_KEYFRAME: &str = "lip-row-grow";

/// Grid track a queued-steer row grows from (`animations.css:294`).
pub const LIP_ROW_GROW_FROM_TRACK: &str = "0fr";

/// Settled grid track of a queued-steer row (`utilities.css:524`).
pub const LIP_ROW_GROW_TRACK: &str = "1fr";

/// Duration token that paces `lip-row-grow` (`utilities.css:525`).
/// Renderer-owned; never constructed here.
pub const LIP_ROW_GROW_DURATION_TOKEN: &str = "--acc-expand";

/// Easing token that shapes `lip-row-grow` (`utilities.css:525`).
/// Renderer-owned; never constructed here.
pub const LIP_ROW_GROW_EASING_TOKEN: &str = "--acc-ease";

/// Horizontal gap between tray thumbnails, in spacing steps
/// (`attachment-tray.svelte:29`, `gap-2`).
pub const ATTACHMENT_TRAY_GAP_STEPS: f32 = 2.0;

/// Square edge of one tray thumbnail tile, in spacing steps
/// (`attachment-tray.svelte:32`, `size-18`).
pub const ATTACHMENT_THUMBNAIL_EDGE_STEPS: f32 = 18.0;

/// Returns the preview-only blocked reason for one engine label
/// (`lib/composer/send-readiness.ts:19`).
#[must_use]
pub fn preview_only_blocked_reason(engine_label: &str) -> String {
    format!("{engine_label} models are preview-only — this engine cannot run in Artisan yet")
}

/// Returns the queued-steer label for one submitted text.
///
/// Whitespace-only text renders [`QUEUED_STEER_EMPTY_TEXT`], matching the
/// legacy `submission.text.trim() || "Attached image"` expression.
#[must_use]
pub fn queued_steer_label(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        QUEUED_STEER_EMPTY_TEXT.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Layout axis of the attachment-tray thumbnail row.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TrayAxis {
    /// Thumbnails sit side by side (`attachment-tray.svelte:29`, `flex`).
    Horizontal,
}

/// Static geometry shared by every tray thumbnail row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrayLayout {
    /// The row direction; always horizontal for this surface.
    pub direction: TrayAxis,
    /// Horizontal gap between tiles, in spacing steps.
    pub gap_steps: f32,
    /// Square edge of one tile, in spacing steps.
    pub tile_edge_steps: f32,
}

/// The tray's one static layout: a horizontal row of `size-18` tiles at
/// `gap-2` (`attachment-tray.svelte:29-32`).
pub const ATTACHMENT_TRAY_LAYOUT: TrayLayout = TrayLayout {
    direction: TrayAxis::Horizontal,
    gap_steps: ATTACHMENT_TRAY_GAP_STEPS,
    tile_edge_steps: ATTACHMENT_THUMBNAIL_EDGE_STEPS,
};

/// Static paint facts for one attachment-tray thumbnail.
///
/// Image bytes and preview references stay renderer-owned: the tile carries a
/// static file glyph placeholder instead, while the file name doubles as the
/// image alt text exactly as the legacy `alt={attachment.name}` does.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AttachmentThumbnail {
    /// Stable identity used as the rendered row key.
    pub attachment_id: String,
    /// Original file name, also the image alt text.
    pub file_name: String,
    /// Exact `View <name>` accessible label.
    pub view_label: String,
    /// Exact `Remove <name>` accessible label.
    pub remove_label: String,
}

impl AttachmentThumbnail {
    /// Builds one thumbnail from one attachment fact, retaining every source
    /// value byte-for-byte through [`AttachmentTrayRow`].
    #[must_use]
    pub fn from_fact(fact: &AttachmentFact) -> Self {
        let row = AttachmentTrayRow::from_attachment(fact);
        Self {
            attachment_id: row.attachment_id().to_owned(),
            file_name: row.name().to_owned(),
            view_label: row.view_label().to_owned(),
            remove_label: row.remove_label().to_owned(),
        }
    }

    /// Returns the exact file name shown beside the tile.
    #[must_use]
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    /// Returns the exact image alt text, which is the original file name.
    #[must_use]
    pub fn alt_text(&self) -> &str {
        &self.file_name
    }

    /// Static file-glyph placeholder for the tile. The catalog holds no
    /// image-specific glyph, so the tile names the file kind generically;
    /// the file name beside it carries the exact identity.
    #[must_use]
    pub const fn tile_icon() -> AssetId {
        AssetId::TABLER_FILE_TEXT
    }

    /// Glyph edge for the tile placeholder: the control-content default.
    #[must_use]
    pub const fn tile_icon_size() -> IconSize {
        IconSize::Default
    }

    /// Remove glyph (`attachment-tray.svelte:48`, `X` at `size-3.5`).
    #[must_use]
    pub const fn remove_icon() -> AssetId {
        AssetId::TABLER_X
    }

    /// Remove-glyph edge for the tighter tray chrome.
    #[must_use]
    pub const fn remove_icon_size() -> IconSize {
        IconSize::Compact
    }

    /// Remove-control face (`attachment-tray.svelte:41-44`, secondary
    /// icon button).
    #[must_use]
    pub const fn remove_variant() -> ButtonVariant {
        ButtonVariant::Secondary
    }

    /// Remove-control size paired with the icon-only content.
    #[must_use]
    pub const fn remove_size() -> ButtonSize {
        ButtonSize::IconSmall
    }

    /// Resolves the tile placeholder recipe from shared theme tokens.
    #[must_use]
    pub fn tile_icon_style(theme: ArtisanTheme) -> IconStyle {
        IconStyle::resolve(
            theme,
            Self::tile_icon(),
            Self::tile_icon_size(),
            IconTint::Inherit,
        )
    }

    /// Resolves the remove-glyph recipe from shared theme tokens.
    #[must_use]
    pub fn remove_icon_style(theme: ArtisanTheme) -> IconStyle {
        IconStyle::resolve(
            theme,
            Self::remove_icon(),
            Self::remove_icon_size(),
            IconTint::Inherit,
        )
    }

    /// Resolves the remove-control recipe from shared theme tokens.
    #[must_use]
    pub fn remove_button_style(theme: ArtisanTheme) -> ButtonStyle {
        ButtonStyle::resolve(
            theme,
            Self::remove_variant(),
            Self::remove_size(),
            Self::motion(),
        )
    }

    /// Motion policy shared by the tray controls: reduced, matching the
    /// native composer send face.
    #[must_use]
    pub const fn motion() -> MotionPolicy {
        MotionPolicy::Reduced
    }

    /// Returns the icon-only remove control content, or `None` when the
    /// retained label has no usable accessible name.
    #[must_use]
    pub fn remove_content(&self) -> Option<ButtonContent> {
        AccessibleLabel::new(self.remove_label.clone())
            .ok()
            .map(|label| ButtonContent::icon_only(Self::remove_icon(), label))
    }
}

/// Static paint facts for the whole attachment-tray row.
///
/// An empty tray carries no rows and paints nothing: renderers must skip the
/// surface entirely when [`Self::is_visible`] is false, matching the legacy
/// tray whose open state derives solely from a nonempty attachment view.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AttachmentTrayStill {
    /// Accessible name of the tray surface.
    pub label: &'static str,
    /// Thumbnails in the exact order supplied by the attachment projection.
    pub rows: Vec<AttachmentThumbnail>,
}

impl AttachmentTrayStill {
    /// Projects ordered attachment facts into ordered thumbnail facts.
    #[must_use]
    pub fn project(attachments: &[AttachmentFact]) -> Self {
        Self {
            label: ATTACHMENT_TRAY_LABEL,
            rows: attachments
                .iter()
                .map(AttachmentThumbnail::from_fact)
                .collect(),
        }
    }

    /// Returns the tray's static layout.
    #[must_use]
    pub const fn layout() -> TrayLayout {
        ATTACHMENT_TRAY_LAYOUT
    }

    /// Returns whether the tray paints at all: exactly when rows exist.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        !self.rows.is_empty()
    }

    /// Returns whether no thumbnail exists.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Returns the number of thumbnails in the row.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Returns the rows to paint, or `None` when the empty tray renders
    /// nothing.
    #[must_use]
    pub fn rows_or_nothing(&self) -> Option<&[AttachmentThumbnail]> {
        if self.rows.is_empty() {
            None
        } else {
            Some(&self.rows)
        }
    }
}

/// Static marker dot for one queued-steer row.
///
/// The legacy row announces through `role="status"` (see
/// [`QUEUED_STEER_ROLE`]); the dot is the native still's static pending
/// marker and owns no legacy string counterpart.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StatusDot {
    /// Native paint token for the marker. No behavior reads this value.
    pub key: &'static str,
}

impl StatusDot {
    /// The one status a queued steer can hold: waiting to be taken up.
    pub const QUEUED: Self = Self { key: "queued" };
}

/// Static `lip-row-grow` endpoints for one queued-steer row.
///
/// This is a presentation description, not an animation driver: it stores no
/// duration, easing callback, timer, or renderer state. Timing tokens stay
/// renderer-owned and collapse under the reduced-motion authority with no
/// code change here.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LipRowMotion {
    /// Static display mode of the row (`utilities.css:523`).
    pub display: &'static str,
    /// Settled grid track of the row (`utilities.css:524`).
    pub track: &'static str,
    /// Grid track the row grows from (`animations.css:294`).
    pub from_track: &'static str,
    /// Keyframe the row plays on mount (`utilities.css:525`).
    pub keyframe: &'static str,
    /// Renderer-owned duration token (`utilities.css:525`).
    pub duration_token: &'static str,
    /// Renderer-owned easing token (`utilities.css:525`).
    pub easing_token: &'static str,
    /// Whether the inner wrapper clips while the track opens
    /// (`utilities.css:527-530`).
    pub inner_clips_overflow: bool,
}

/// The fixed `lip-row-grow` endpoints shared by every queued-steer row.
pub const LIP_ROW_GROW: LipRowMotion = LipRowMotion {
    display: "grid",
    track: LIP_ROW_GROW_TRACK,
    from_track: LIP_ROW_GROW_FROM_TRACK,
    keyframe: LIP_ROW_GROW_KEYFRAME,
    duration_token: LIP_ROW_GROW_DURATION_TOKEN,
    easing_token: LIP_ROW_GROW_EASING_TOKEN,
    inner_clips_overflow: true,
};

/// Static paint facts for one queued-steer row: a label plus a status dot.
///
/// `editable` mirrors whether the composer can still recall the steer
/// (`editable={onwithdraw !== undefined}` in `thread-composer.svelte:616`):
/// without a recall route the row only informs and shows no actions.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QueuedSteerRow {
    /// Generation owning this row in the pending-lip stack.
    pub generation: u64,
    /// Single-line label: the trimmed submission text, or
    /// [`QUEUED_STEER_EMPTY_TEXT`] when nothing remains.
    pub label: String,
    /// Whether the edit/discard actions are shown.
    pub editable: bool,
    /// Static pending marker dot.
    pub status_dot: StatusDot,
    /// Static `lip-row-grow` endpoints.
    pub motion: LipRowMotion,
}

impl QueuedSteerRow {
    /// Builds one row from its generation, submitted text, and recall
    /// availability. The text is retained trimmed-or-fallback only; no
    /// submission payload, effect, or route is stored.
    #[must_use]
    pub fn new(generation: u64, text: &str, editable: bool) -> Self {
        Self {
            generation,
            label: queued_steer_label(text),
            editable,
            status_dot: StatusDot::QUEUED,
            motion: LIP_ROW_GROW,
        }
    }

    /// Live-region role of the row.
    #[must_use]
    pub const fn role() -> &'static str {
        QUEUED_STEER_ROLE
    }

    /// Accessible name and title of the edit control.
    #[must_use]
    pub const fn edit_label() -> &'static str {
        EDIT_QUEUED_MESSAGE_LABEL
    }

    /// Accessible name and title of the discard control.
    #[must_use]
    pub const fn discard_label() -> &'static str {
        DISCARD_QUEUED_MESSAGE_LABEL
    }

    /// Edit glyph (`steering-lip.svelte:43`, pencil at `size-4`).
    #[must_use]
    pub const fn edit_icon() -> AssetId {
        AssetId::TABLER_PENCIL
    }

    /// Discard glyph (`steering-lip.svelte:53`, trash at `size-4`).
    #[must_use]
    pub const fn discard_icon() -> AssetId {
        AssetId::TABLER_TRASH
    }

    /// Glyph edge for the row actions: the control-content default.
    #[must_use]
    pub const fn action_icon_size() -> IconSize {
        IconSize::Default
    }

    /// Resolves the edit-glyph recipe from shared theme tokens. The actions
    /// wear the muted treatment (`steering-lip.svelte:38,48`).
    #[must_use]
    pub fn edit_icon_style(theme: ArtisanTheme) -> IconStyle {
        IconStyle::resolve(
            theme,
            Self::edit_icon(),
            Self::action_icon_size(),
            IconTint::Muted,
        )
    }

    /// Resolves the discard-glyph recipe from shared theme tokens.
    #[must_use]
    pub fn discard_icon_style(theme: ArtisanTheme) -> IconStyle {
        IconStyle::resolve(
            theme,
            Self::discard_icon(),
            Self::action_icon_size(),
            IconTint::Muted,
        )
    }
}

/// Which send-button presentation a readiness gate paints.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SendGate {
    /// Armed: the composed message can leave.
    Ready,
    /// Disarmed with a tooltip reason: the message stays in the composer.
    Blocked,
    /// A submission is in flight: the button names it and holds a progress
    /// track.
    Sending,
}

/// Static paint facts for one send-button presentation.
///
/// The face follows the native composer send button (`native_composer.rs`):
/// a ghost small text button under reduced motion whose content is `Send`
/// while idle and `Sending…` in flight, disabled whenever it cannot act.
/// The `controls.svelte` icon-swap face is carried alongside as the
/// [`Self::control_icon`] / [`Self::control_label`] companion facts.
#[derive(Clone, Debug, PartialEq)]
pub struct SendButtonStill {
    /// Which presentation this still holds.
    pub gate: SendGate,
    /// Visible button text: `Send` or `Sending…`.
    pub text: &'static str,
    /// Whether the control suppresses interaction.
    pub disabled: bool,
    /// Tooltip reason shown while blocked; absent otherwise.
    pub blocked_reason: Option<String>,
    /// Progress-track fill; present only while sending. The legacy product
    /// has no determinate send fraction, so the still holds the empty track
    /// and the renderer repaints it away when the flight settles. There is
    /// deliberately no indeterminate rendering: [`artisan_ui::progress`]
    /// exposes none, and this module adds none.
    pub progress_fill: Option<ProgressFraction>,
}

impl SendButtonStill {
    /// Armed presentation: `Send`, enabled, no reason, no track.
    #[must_use]
    pub const fn ready() -> Self {
        Self {
            gate: SendGate::Ready,
            text: SEND_BUTTON_TEXT,
            disabled: false,
            blocked_reason: None,
            progress_fill: None,
        }
    }

    /// Disarmed presentation: `Send`, disabled, with the exact tooltip
    /// reason the composer surface explains itself with.
    #[must_use]
    pub fn blocked(reason: impl Into<String>) -> Self {
        Self {
            gate: SendGate::Blocked,
            text: SEND_BUTTON_TEXT,
            disabled: true,
            blocked_reason: Some(reason.into()),
            progress_fill: None,
        }
    }

    /// In-flight presentation: `Sending…`, disabled, with the empty
    /// progress track visible.
    #[must_use]
    pub const fn sending() -> Self {
        Self {
            gate: SendGate::Sending,
            text: SENDING_BUTTON_TEXT,
            disabled: true,
            blocked_reason: None,
            progress_fill: Some(ProgressFraction::EMPTY),
        }
    }

    /// Button face shared by all three presentations.
    #[must_use]
    pub const fn variant() -> ButtonVariant {
        ButtonVariant::Ghost
    }

    /// Button size shared by all three presentations.
    #[must_use]
    pub const fn size() -> ButtonSize {
        ButtonSize::Small
    }

    /// Motion policy shared by all three presentations.
    #[must_use]
    pub const fn motion() -> MotionPolicy {
        MotionPolicy::Reduced
    }

    /// Returns the typed button content; the visible text supplies the
    /// accessible name.
    #[must_use]
    pub fn content(&self) -> ButtonContent {
        ButtonContent::text(self.text)
    }

    /// Resolves the button recipe from shared theme tokens.
    #[must_use]
    pub fn button_style(&self, theme: ArtisanTheme) -> ButtonStyle {
        ButtonStyle::resolve(theme, Self::variant(), Self::size(), Self::motion())
    }

    /// Resolves the progress-track recipe from shared theme tokens.
    #[must_use]
    pub fn progress_style(theme: ArtisanTheme) -> ProgressStyle {
        ProgressStyle::resolve(theme)
    }

    /// Icon-swap companion face from `controls.svelte:165-174`: the arrow
    /// while idle, the filled stop glyph while a run holds the button.
    #[must_use]
    pub const fn control_icon(run_active: bool) -> AssetId {
        if run_active {
            AssetId::TABLER_PLAYER_STOP_FILLED
        } else {
            AssetId::TABLER_ARROW_UP
        }
    }

    /// Accessible-name companion face from `controls.svelte:160`.
    #[must_use]
    pub const fn control_label(run_active: bool) -> &'static str {
        if run_active {
            STOP_RUN_LABEL
        } else {
            SEND_MESSAGE_LABEL
        }
    }
}

/// Theme used by the fixture stills: the dark mode the native composer
/// paints with (`native_composer.rs:497`).
#[must_use]
pub fn fixture_theme() -> ArtisanTheme {
    ArtisanTheme::for_mode(ThemeMode::Dark)
}

/// Fixture tray with two attachments keyed by a real queue, so the keys keep
/// the `attachment:<n>` token shape without hand-writing it.
#[must_use]
pub fn fixture_attachment_tray() -> AttachmentTrayStill {
    let mut queue = ScopedAttachmentQueue::<()>::new();
    let first = queue.attach(());
    let second = queue.attach(());
    AttachmentTrayStill::project(&[
        AttachmentFact::new(first, "screenshot.png", "fixture-preview-0"),
        AttachmentFact::new(second, "error-photo.webp", "fixture-preview-1"),
    ])
}

/// Fixture steer rows: one recallable row with text and one informing-only
/// row whose blank text renders the image fallback.
#[must_use]
pub fn fixture_queued_steers() -> Vec<QueuedSteerRow> {
    vec![
        QueuedSteerRow::new(2, "  Keep the lip open while streaming  ", true),
        QueuedSteerRow::new(1, "   ", false),
    ]
}

/// Fixture send stills in gate order: armed, disarmed, in flight.
#[must_use]
pub fn fixture_send_buttons() -> [SendButtonStill; 3] {
    [
        SendButtonStill::ready(),
        SendButtonStill::blocked(FORGE_OFFLINE_REASON),
        SendButtonStill::sending(),
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        ATTACHMENT_THUMBNAIL_EDGE_STEPS, ATTACHMENT_TRAY_GAP_STEPS, ATTACHMENT_TRAY_LABEL,
        AttachmentFact, AttachmentThumbnail, AttachmentTrayStill, ButtonContent, ButtonSize,
        ButtonVariant, DISCARD_QUEUED_MESSAGE_LABEL, EDIT_QUEUED_MESSAGE_LABEL,
        FORGE_OFFLINE_REASON, IconSize, LIP_ROW_GROW, MotionPolicy, NEW_THREAD_FAILURE_TITLE,
        PREPARING_TO_SEND_REASON, QUEUED_STEER_EMPTY_TEXT, QUEUED_STEER_ROLE, QueuedSteerRow,
        RECALL_UNAVAILABLE_MESSAGE, SEND_BUTTON_TEXT, SEND_FAILURE_TITLE, SEND_MESSAGE_LABEL,
        SENDING_BUTTON_TEXT, START_NEW_THREAD_PROMPT_LABEL, STOP_RUN_LABEL, SendButtonStill,
        SendGate, StatusDot, TrayAxis, fixture_attachment_tray, fixture_queued_steers,
        fixture_send_buttons, fixture_theme, preview_only_blocked_reason, queued_steer_label,
    };
    use artisan_assets::AssetId;
    use artisan_ui::progress::ProgressFraction;

    fn fact(id: &str, name: &str) -> AttachmentFact {
        AttachmentFact::new(id, name, "fixture-preview")
    }

    #[test]
    fn thumbnail_labels_are_byte_identical_to_the_legacy_tray() {
        let thumbnail = AttachmentThumbnail::from_fact(&fact("attachment:0", "screenshot.png"));
        assert_eq!(thumbnail.attachment_id, "attachment:0");
        assert_eq!(thumbnail.file_name(), "screenshot.png");
        assert_eq!(thumbnail.alt_text(), "screenshot.png");
        assert_eq!(thumbnail.view_label, "View screenshot.png");
        assert_eq!(thumbnail.remove_label, "Remove screenshot.png");
    }

    #[test]
    fn tray_projects_rows_in_order_and_names_its_surface() {
        let tray = AttachmentTrayStill::project(&[
            fact("attachment:0", "a.png"),
            fact("attachment:1", "b.webp"),
        ]);
        assert_eq!(tray.label, ATTACHMENT_TRAY_LABEL);
        assert_eq!(tray.label, "Attached images");
        assert!(tray.is_visible());
        assert!(!tray.is_empty());
        assert_eq!(tray.len(), 2);
        let rows = tray.rows_or_nothing().expect("nonempty tray paints");
        assert_eq!(rows[0].file_name(), "a.png");
        assert_eq!(rows[1].file_name(), "b.webp");
    }

    #[test]
    fn empty_tray_renders_nothing() {
        let tray = AttachmentTrayStill::project(&[]);
        assert!(!tray.is_visible());
        assert!(tray.is_empty());
        assert_eq!(tray.len(), 0);
        assert!(tray.rows_or_nothing().is_none());
    }

    #[test]
    fn tray_layout_is_a_horizontal_row_of_legacy_tiles() {
        let layout = AttachmentTrayStill::layout();
        assert_eq!(layout.direction, TrayAxis::Horizontal);
        assert!(layout.gap_steps > 0.0);
        assert!((layout.gap_steps - ATTACHMENT_TRAY_GAP_STEPS).abs() < f32::EPSILON);
        assert!((layout.tile_edge_steps - ATTACHMENT_THUMBNAIL_EDGE_STEPS).abs() < f32::EPSILON);
        assert!(layout.tile_edge_steps > layout.gap_steps);
    }

    #[test]
    fn tray_controls_reuse_the_secondary_icon_recipe_and_remove_glyph() {
        assert_eq!(
            AttachmentThumbnail::remove_variant(),
            ButtonVariant::Secondary
        );
        assert_eq!(AttachmentThumbnail::remove_size(), ButtonSize::IconSmall);
        assert_eq!(AttachmentThumbnail::remove_icon(), AssetId::TABLER_X);
        assert_eq!(AttachmentThumbnail::remove_icon_size(), IconSize::Compact);
        let thumbnail = AttachmentThumbnail::from_fact(&fact("attachment:0", "a.png"));
        let content = thumbnail
            .remove_content()
            .expect("a Remove label is never blank");
        assert_eq!(content.accessible_label(), "Remove a.png");
        assert!(matches!(content, ButtonContent::IconOnly { .. }));
    }

    #[test]
    fn steer_label_trims_text_and_falls_back_to_the_image_copy() {
        assert_eq!(queued_steer_label("  hello  "), "hello");
        assert_eq!(queued_steer_label("   "), QUEUED_STEER_EMPTY_TEXT);
        assert_eq!(queued_steer_label(""), "Attached image");
    }

    #[test]
    fn steer_rows_carry_status_facts_and_recall_availability() {
        let editable = QueuedSteerRow::new(7, "  take me back  ", true);
        assert_eq!(editable.generation, 7);
        assert_eq!(editable.label, "take me back");
        assert!(editable.editable);
        assert_eq!(editable.status_dot, StatusDot::QUEUED);
        assert_eq!(QueuedSteerRow::role(), QUEUED_STEER_ROLE);
        assert_eq!(QueuedSteerRow::role(), "status");
        assert_eq!(QueuedSteerRow::edit_label(), EDIT_QUEUED_MESSAGE_LABEL);
        assert_eq!(QueuedSteerRow::edit_label(), "Edit queued message");
        assert_eq!(
            QueuedSteerRow::discard_label(),
            DISCARD_QUEUED_MESSAGE_LABEL
        );
        assert_eq!(QueuedSteerRow::discard_label(), "Discard queued message");

        let informing = QueuedSteerRow::new(6, "\n\t ", false);
        assert_eq!(informing.label, "Attached image");
        assert!(!informing.editable);
    }

    #[test]
    fn steer_motion_is_the_static_lip_row_grow_endpoints() {
        let row = QueuedSteerRow::new(1, "x", true);
        assert_eq!(row.motion, LIP_ROW_GROW);
        assert_eq!(row.motion.display, "grid");
        assert_eq!(row.motion.track, "1fr");
        assert_eq!(row.motion.from_track, "0fr");
        assert_eq!(row.motion.keyframe, "lip-row-grow");
        assert_eq!(row.motion.duration_token, "--acc-expand");
        assert_eq!(row.motion.easing_token, "--acc-ease");
        assert!(row.motion.inner_clips_overflow);
    }

    #[test]
    fn steer_glyphs_match_the_legacy_edit_and_discard_controls() {
        assert_eq!(QueuedSteerRow::edit_icon(), AssetId::TABLER_PENCIL);
        assert_eq!(QueuedSteerRow::discard_icon(), AssetId::TABLER_TRASH);
        assert_eq!(QueuedSteerRow::action_icon_size(), IconSize::Default);
        let theme = fixture_theme();
        let edit = QueuedSteerRow::edit_icon_style(theme);
        assert_eq!(edit.asset_id(), AssetId::TABLER_PENCIL);
        let discard = QueuedSteerRow::discard_icon_style(theme);
        assert_eq!(discard.asset_id(), AssetId::TABLER_TRASH);
    }

    #[test]
    fn send_stills_cover_ready_blocked_and_sending() {
        let ready = SendButtonStill::ready();
        assert_eq!(ready.gate, SendGate::Ready);
        assert_eq!(ready.text, SEND_BUTTON_TEXT);
        assert_eq!(ready.text, "Send");
        assert!(!ready.disabled);
        assert!(ready.blocked_reason.is_none());
        assert!(ready.progress_fill.is_none());
        assert_eq!(ready.content().accessible_label(), "Send");

        let blocked = SendButtonStill::blocked(FORGE_OFFLINE_REASON);
        assert_eq!(blocked.gate, SendGate::Blocked);
        assert_eq!(blocked.text, "Send");
        assert!(blocked.disabled);
        assert_eq!(
            blocked.blocked_reason.as_deref(),
            Some("Forge is offline — reconnect to send")
        );
        assert!(blocked.progress_fill.is_none());

        let sending = SendButtonStill::sending();
        assert_eq!(sending.gate, SendGate::Sending);
        assert_eq!(sending.text, SENDING_BUTTON_TEXT);
        assert_eq!(sending.text, "Sending…");
        assert!(sending.disabled);
        assert!(sending.blocked_reason.is_none());
        assert_eq!(sending.progress_fill, Some(ProgressFraction::EMPTY));
    }

    #[test]
    fn send_stills_reuse_the_ghost_button_recipe_without_motion() {
        let theme = fixture_theme();
        for still in fixture_send_buttons() {
            assert_eq!(SendButtonStill::variant(), ButtonVariant::Ghost);
            assert_eq!(SendButtonStill::size(), ButtonSize::Small);
            assert_eq!(SendButtonStill::motion(), MotionPolicy::Reduced);
            let style = still.button_style(theme);
            assert!(style.pressed_offset_y.is_none());
        }
        let track = SendButtonStill::progress_style(theme);
        assert_eq!(track, SendButtonStill::progress_style(theme));
    }

    #[test]
    fn control_companion_facts_match_the_legacy_icon_swap_face() {
        assert_eq!(
            SendButtonStill::control_icon(false),
            AssetId::TABLER_ARROW_UP
        );
        assert_eq!(
            SendButtonStill::control_icon(true),
            AssetId::TABLER_PLAYER_STOP_FILLED
        );
        assert_eq!(SendButtonStill::control_label(false), SEND_MESSAGE_LABEL);
        assert_eq!(SendButtonStill::control_label(false), "Send message");
        assert_eq!(SendButtonStill::control_label(true), STOP_RUN_LABEL);
        assert_eq!(SendButtonStill::control_label(true), "Stop current run");
    }

    #[test]
    fn blocked_copy_is_byte_identical_to_the_legacy_surfaces() {
        assert_eq!(
            preview_only_blocked_reason("Nebula"),
            "Nebula models are preview-only — this engine cannot run in Artisan yet"
        );
        assert_eq!(
            PREPARING_TO_SEND_REASON,
            "This surface is still preparing to send. Your message is kept in the composer."
        );
        assert_eq!(SEND_FAILURE_TITLE, "Could not send message");
        assert_eq!(NEW_THREAD_FAILURE_TITLE, "Could not start a new thread");
        assert_eq!(
            START_NEW_THREAD_PROMPT_LABEL,
            "Start a new thread with prompt"
        );
        assert_eq!(
            RECALL_UNAVAILABLE_MESSAGE,
            "This surface cannot recall a queued message."
        );
    }

    #[test]
    fn fixtures_hold_together_for_screenshots() {
        let tray = fixture_attachment_tray();
        assert_eq!(tray.len(), 2);
        assert!(tray.is_visible());
        let rows = tray.rows_or_nothing().expect("fixture tray paints");
        assert_eq!(rows[0].attachment_id, "attachment:0");
        assert_eq!(rows[1].attachment_id, "attachment:1");

        let steers = fixture_queued_steers();
        assert_eq!(steers.len(), 2);
        assert!(steers.iter().any(|row| row.editable));
        assert!(steers.iter().any(|row| !row.editable));

        let buttons = fixture_send_buttons();
        assert_eq!(
            [buttons[0].gate, buttons[1].gate, buttons[2].gate],
            [SendGate::Ready, SendGate::Blocked, SendGate::Sending]
        );
    }
}
