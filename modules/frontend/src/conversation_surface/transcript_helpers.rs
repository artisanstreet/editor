//! Pure transcript helper policies: navigator markers, shell command
//! presentation, activity-chain copy, and shared row painters.

use super::*;

/// One loaded user-message control in the turn navigator rail.
pub(super) struct NavigatorMarker {
    /// Visible policy label; never crosses the action boundary.
    pub(super) label: String,
    /// Exact scroll target; identity only, never body text.
    pub(super) target: ConversationSurfaceTarget,
    /// Index of the owning turn in the accepted scene order.
    ///
    /// Carried so painted-geometry lookups touch marker-bearing turns only
    /// instead of walking every turn and block each frame.
    pub(super) turn_index: usize,
}

/// Window-local rail geometry for the turn navigator.
///
/// The rail centers vertically in the card from live measurement: both
/// windows sharing one surface converge independently, exactly like the
/// transcript end-space height.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct TurnNavigatorMetrics {
    /// Rail top offset in px within the surface root.
    pub(super) top_px: f32,
    /// Viewport height in px at the last measurement.
    pub(super) viewport_px: f32,
}

/// Samples the reference dropdown easing for navigator motion clocks.
///
/// `cubic-bezier(0.22, 1, 0.36, 1)` (`theme.css:140`), shared with the model
/// picker hover flights rather than re-solved per surface.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the shared easing curve samples in f64 and feeds the f32 animation clock; the narrowing is the intended precision"
)]
pub(super) fn navigator_smooth_out(progress: f32) -> f32 {
    MotionCurve::SmoothOut.sample(f64::from(progress)) as f32
}

/// One recognised shell wrapper: how its executable is named and how it takes
/// a command. Ported from the reference `shell-command.ts`.
struct ShellWrapper {
    flags: &'static [&'static str],
    names: &'static [&'static str],
}

/// The wrappers a reader means, in the reference's exact order.
const SHELL_WRAPPERS: &[ShellWrapper] = &[
    ShellWrapper {
        flags: &["-command", "-c"],
        names: &["pwsh", "powershell"],
    },
    ShellWrapper {
        flags: &["-c", "-lc", "-ic", "-lic"],
        names: &["bash", "sh", "zsh", "dash"],
    },
    ShellWrapper {
        flags: &["/c", "/k"],
        names: &["cmd"],
    },
];

/// Splits one leading argument, honouring the quotes a path with spaces needs.
///
/// Returns `(rest, value)` exactly like the reference: the rest keeps its
/// leading whitespace (the next split trims it) and an unterminated quote
/// falls back to the whitespace split.
pub(super) fn take_shell_argument(input: &str) -> (&str, &str) {
    let text = input.trim_start();
    let quote = if text.starts_with('"') {
        Some('"')
    } else if text.starts_with('\'') {
        Some('\'')
    } else {
        None
    };

    if let Some(quote) = quote
        && let Some(offset) = text[1..].find(quote)
    {
        let close = 1 + offset;
        return (&text[close + 1..], &text[1..close]);
    }

    match text.find(char::is_whitespace) {
        Some(break_at) => (&text[break_at..], &text[..break_at]),
        None => ("", text),
    }
}

/// The executable's own name, without its directory or extension.
pub(super) fn shell_executable_name(path: &str) -> String {
    let file = match path.rfind(['/', '\\']) {
        Some(separator) => &path[separator + 1..],
        None => path,
    };
    let lowered = file.to_lowercase();
    lowered.strip_suffix(".exe").unwrap_or(&lowered).to_owned()
}

/// Drops one matching pair of surrounding quotes, which a shell would have
/// eaten.
pub(super) fn unquote_shell_argument(text: &str) -> &str {
    let first = text.chars().next();
    match first {
        Some(first @ ('"' | '\'')) if text.len() > 1 && text.ends_with(first) => {
            &text[1..text.len() - 1]
        }
        _ => text,
    }
}

/// Collapses every whitespace run to one space and trims, exactly like the
/// reference `replaceAll(/\s+/g, " ").trim()`.
pub(super) fn collapse_whitespace(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        pending_space = false;
        collapsed.push(character);
    }
    collapsed
}

/// The command a reader means, recovered from the invocation an engine
/// reports. Ported from the reference `PresentShellCommand`.
///
/// A run arrives as the full argv it was spawned with, which on Windows
/// begins with an absolute path into `WindowsApps` before `-Command` and the
/// actual work. In a truncated row every visible character would be that
/// path, so the line would say less than the word it replaced. Anything not
/// recognised is returned as it came, because a command this does not
/// understand is still more useful whole than guessed at.
#[must_use]
pub fn present_shell_command(command: &str) -> String {
    let collapsed = collapse_whitespace(command);
    let (rest, executable) = take_shell_argument(&collapsed);
    let name = shell_executable_name(executable);
    let Some(wrapper) = SHELL_WRAPPERS
        .iter()
        .find(|candidate| candidate.names.contains(&name.as_str()))
    else {
        return collapsed;
    };

    let (body, flag) = take_shell_argument(rest);
    if !wrapper.flags.contains(&flag.to_lowercase().as_str()) {
        return collapsed;
    }

    let body = unquote_shell_argument(body.trim());

    if body.is_empty() {
        collapsed
    } else {
        body.to_owned()
    }
}

/// Whether one lifecycle is a failure the trace must explain.
pub(super) const fn lifecycle_is_failed(lifecycle: ConversationLifecycle) -> bool {
    matches!(
        lifecycle,
        ConversationLifecycle::Failed
            | ConversationLifecycle::Cancelled
            | ConversationLifecycle::Interrupted
    )
}

/// Whether one lifecycle is live work; the set the scene build treats as
/// potentially still arriving.
pub(super) const fn lifecycle_is_live(lifecycle: ConversationLifecycle) -> bool {
    matches!(
        lifecycle,
        ConversationLifecycle::Pending
            | ConversationLifecycle::Streaming
            | ConversationLifecycle::Active
            | ConversationLifecycle::Waiting
    )
}

/// The muted row text for one activity: the normalized shell command for
/// terminal details, the raw detail otherwise, else the presentation label.
///
/// A terminal detail whose normalization collapses to nothing falls back to
/// the label, so the row never paints an empty second column.
#[must_use]
pub(super) fn activity_detail_text(
    kind: &str,
    detail: Option<&str>,
    lifecycle: Option<ConversationLifecycle>,
) -> String {
    match detail {
        Some(detail) if kind == "terminal_activity" => {
            let presented = present_shell_command(detail);
            if presented.is_empty() {
                activity_presentation_label(kind, lifecycle).unwrap_or_default()
            } else {
                presented
            }
        }
        Some(detail) => detail.to_owned(),
        None => activity_presentation_label(kind, lifecycle).unwrap_or_default(),
    }
}

/// Builds one chain header's clause sentence from its activity kinds.
///
/// Composition is counted in first-appearance order so the header describes
/// the work rather than measuring it: "Ran 1 command, read 2 files". The
/// first clause is capitalized, later ones join with a comma, exactly like
/// the reference `GroupClauses`.
#[must_use]
pub(super) fn activity_chain_clause<'a>(kinds: impl Iterator<Item = &'a str>) -> String {
    let mut composition: Vec<(ActivityCategory, usize)> = Vec::new();
    for kind in kinds {
        let category = activity_category(kind);
        match composition.iter_mut().find(|(known, _)| *known == category) {
            Some((_, count)) => *count += 1,
            None => composition.push((category, 1)),
        }
    }

    let mut clause_text = String::new();
    for (index, (category, count)) in composition.iter().enumerate() {
        let clause = category.count_label(*count);
        if index == 0 {
            let mut characters = clause.chars();
            if let Some(first) = characters.next() {
                clause_text.extend(first.to_uppercase());
                clause_text.push_str(characters.as_str());
            }
        } else {
            clause_text.push_str(", ");
            clause_text.push_str(&clause);
        }
    }
    clause_text
}

/// The Tabler glyph naming one activity category, matching the reference
/// `CategoryIcon`.
pub(super) const fn activity_category_icon(category: ActivityCategory) -> AssetId {
    match category {
        ActivityCategory::Command | ActivityCategory::Test | ActivityCategory::Typecheck => {
            AssetId::TABLER_TERMINAL_2
        }
        ActivityCategory::FileRead => AssetId::TABLER_FILE_TEXT,
        ActivityCategory::FileEdit => AssetId::TABLER_FILE_PENCIL,
        ActivityCategory::FileDelete => AssetId::TABLER_FILE_X,
        ActivityCategory::FileSearch => AssetId::TABLER_FILE_SEARCH,
        ActivityCategory::WebSearch => AssetId::TABLER_WORLD_SEARCH,
        _ => AssetId::TABLER_TOOL,
    }
}

/// The chain header's glyph: a homogeneous chain is represented by its tool
/// category, while mixed work uses the group glyph because no single category
/// can honestly name its contents (reference `GroupIcon`).
#[must_use]
pub(super) fn activity_chain_icon<'a>(kinds: impl Iterator<Item = &'a str>) -> AssetId {
    let mut categories: Vec<ActivityCategory> = Vec::new();
    for kind in kinds {
        let category = activity_category(kind);
        if !categories.contains(&category) {
            categories.push(category);
        }
    }
    match categories.as_slice() {
        [only] => activity_category_icon(*only),
        _ => AssetId::TABLER_LIST_DETAILS,
    }
}

/// Resolves one activity chain's default disclosure state.
///
/// The reference defaults an activity group closed unless the chain failed or
/// is live. Native signals are the member lifecycles plus the owning turn: a
/// group that owns the turn's live Thinking/Working line counts as live while
/// the turn is not terminal (the closest native analogue of the reference
/// `work_active` flag), and a failed, cancelled, or interrupted turn opens
/// its chain so the failure can explain itself. Unknown liveness never counts
/// as live.
#[must_use]
pub(super) fn activity_chain_disclosure_state(
    rows: &[(u64, DetailRow<'_>)],
    live_header: bool,
    turn_lifecycle: ConversationLifecycle,
) -> (bool, bool) {
    let mut live = live_header && !turn_lifecycle.is_terminal();
    let mut failed = lifecycle_is_failed(turn_lifecycle);
    for (_, row) in rows {
        if let DetailRow::Activity {
            lifecycle: Some(lifecycle),
            ..
        } = row
        {
            live |= lifecycle_is_live(*lifecycle);
            failed |= lifecycle_is_failed(*lifecycle);
        }
    }
    (live, failed)
}

/// Returns whether one work group paints actual visible trace content.
///
/// This is the native `has_visible_details` trace predicate
/// (`work_session_disclosure`): session-title markers and empty bodies are
/// signals, not content, so a `Thought for …` group with no rows paints no
/// disclosure control at all instead of a blank collapsible. A kinded
/// activity counts as content even with an empty body, because its category
/// label and fallback presentation always paint.
pub(super) fn work_group_has_visible_details(block: &WorkGroupBlock) -> bool {
    ordered_detail_rows(block).iter().any(|(_, row)| match row {
        DetailRow::Assistant { body, .. } => !body.trim().is_empty(),
        DetailRow::Activity {
            body, kind, detail, ..
        } => {
            kind.is_some()
                || !body.trim().is_empty()
                || detail.is_some_and(|detail| !detail.trim().is_empty())
        }
        DetailRow::Compaction { summary, .. } => !summary.trim().is_empty(),
        DetailRow::NativeFact { text, .. } => !text.trim().is_empty(),
        DetailRow::SessionTitle { .. } => false,
    })
}

/// Returns the stable focus-map key for one navigator target
/// (`item:<id>` or `scene:<id>`).
pub(super) fn navigator_focus_key(target: &ConversationSurfaceTarget) -> String {
    match target {
        ConversationSurfaceTarget::Item(id) => format!("item:{}", id.as_str()),
        ConversationSurfaceTarget::Scene(id) => format!("scene:{}", id.as_str()),
    }
}

/// Returns the raw scene or item identity carried by a navigator target.
pub(super) fn navigator_target_slug(target: &ConversationSurfaceTarget) -> &str {
    match target {
        ConversationSurfaceTarget::Item(id) => id.as_str(),
        ConversationSurfaceTarget::Scene(id) => id.as_str(),
    }
}

/// Derives the loaded-turn navigator markers from the current scene only.
///
/// The existing `conversation_turn_markers` policy supplies ordering,
/// labels, and the two-marker minimum. Durable user-message identities
/// become exact `Item` targets; anything else that survives the policy
/// keeps its exact render-only `Scene` identity. Window markers are never
/// supplied: this surface renders only loaded turns.
#[must_use]
pub(super) fn loaded_turn_navigator_markers(scene: &ConversationScene) -> Vec<NavigatorMarker> {
    let mut turns = Vec::new();
    let mut items = Vec::new();
    let mut item_turn_indices: HashMap<&str, usize> = HashMap::new();
    let mut ordinal: u64 = 0;
    for (turn_index, turn_scene) in scene.turn_scenes().iter().enumerate() {
        turns.push(ConversationTurnInput::new(
            turn_scene.turn_id.as_str(),
            turn_scene.ordinal,
        ));
        for block in turn_scene.blocks() {
            if let TurnBlock::UserMessage(message) = block {
                items.push(LoadedConversationItemInput::user_message(
                    message.id.as_str(),
                    turn_scene.turn_id.as_str(),
                    ordinal,
                    message.body.clone(),
                ));
                item_turn_indices.insert(message.id.as_str(), turn_index);
                ordinal = ordinal.saturating_add(1);
            }
        }
    }
    let snapshot = ConversationSnapshotInput::new(turns, items, None);
    conversation_turn_markers(&snapshot)
        .into_iter()
        .filter_map(|marker| {
            let target = ItemId::parse(marker.id.as_str())
                .ok()
                .map(ConversationSurfaceTarget::Item)
                .or_else(|| {
                    SceneId::parse(marker.id.as_str())
                        .ok()
                        .map(ConversationSurfaceTarget::Scene)
                })?;
            // Every marker in a loaded snapshot is a loaded user message, so
            // its owning turn index is present; an unresolvable identity keeps
            // index zero and only affects which bound it measures against.
            let turn_index = item_turn_indices
                .get(marker.id.as_str())
                .copied()
                .unwrap_or(0);
            Some(NavigatorMarker {
                label: marker.label,
                target,
                turn_index,
            })
        })
        .collect()
}

pub(super) fn item_id_for_scene_id(id: &SceneId) -> Option<ItemId> {
    ItemId::parse(id.as_str()).ok()
}

/// Returns the block identities of every question row in the scene, in
/// render order, for per-row input focus retention.
pub(super) fn question_block_ids(scene: &ConversationScene) -> Vec<String> {
    let mut ids = Vec::new();
    for turn_scene in scene.turn_scenes() {
        for block in turn_scene.blocks() {
            if let TurnBlock::Question(question) = block {
                ids.push(question.id.as_str().to_owned());
            }
        }
    }
    ids
}

/// Returns the scene identity that owns the transcript position of one work
/// group card. This is the single source for the group-card anchor identity
/// shared by rendering and scroll-target resolution.
pub(super) fn work_group_anchor_id(turn_id: &TurnId, block: &WorkGroupBlock) -> Option<SceneId> {
    block
        .items
        .first()
        .map(work_item_id)
        .cloned()
        .or_else(|| SceneId::parse(turn_id.as_str()).ok())
}

pub(super) fn text_block_scroll_identity(id: &SceneId) -> (Option<SceneId>, Option<ItemId>) {
    (Some(id.clone()), item_id_for_scene_id(id))
}

/// Returns the anchor identity pair painted for one transcript child.
///
/// The pair mirrors the exact arguments passed to
/// [`ScrollAnchorRegistry::attach`] for that child, so prepaint listeners can
/// resolve executed scroll targets against measured child bounds. Unanchored
/// rows report no identity and never match.
pub(super) fn block_scroll_identity(
    turn_id: &TurnId,
    block: &TurnBlock,
) -> (Option<SceneId>, Option<ItemId>) {
    match block {
        TurnBlock::UserMessage(block) => text_block_scroll_identity(&block.id),
        TurnBlock::AssistantMessage(block) => text_block_scroll_identity(&block.id),
        TurnBlock::WorkGroup(block) => (
            block
                .session
                .clone()
                .or_else(|| work_group_anchor_id(turn_id, block)),
            None,
        ),
        TurnBlock::Compaction(block) => text_block_scroll_identity(&block.id),
        TurnBlock::ChangeSet(block) => text_block_scroll_identity(&block.id),
        TurnBlock::Plan(block) => text_block_scroll_identity(&block.id),
        TurnBlock::Approval(block) => text_block_scroll_identity(&block.id),
        TurnBlock::Question(block) => text_block_scroll_identity(&block.id),
        TurnBlock::Error(block) => text_block_scroll_identity(&block.id),
        TurnBlock::UsageInterruption(block) => text_block_scroll_identity(&block.id),
        TurnBlock::ModelTransition(block) => text_block_scroll_identity(&block.id),
        TurnBlock::NativeFact(block) => text_block_scroll_identity(&block.id),
        TurnBlock::SteeringLabel(block) => (Some(block.id.clone()), None),
        TurnBlock::TurnStatus(_) | TurnBlock::TurnFooter(_) => (None, None),
    }
}

/// Returns whether a queued scroll target addresses one measured identity.
///
/// This mirrors [`RenderedScrollAnchor::matches`] for the transient
/// render-to-prepaint handoff, which carries plain identity pairs instead of
/// anchor objects.
pub(super) fn scroll_target_matches_identity(
    target: &ConversationSurfaceTarget,
    identity: &(Option<SceneId>, Option<ItemId>),
) -> bool {
    match target {
        ConversationSurfaceTarget::Scene(scene_id) => identity.0.as_ref() == Some(scene_id),
        ConversationSurfaceTarget::Item(item_id) => identity.1.as_ref() == Some(item_id),
    }
}
pub(super) fn card_heading(title: impl Into<SharedString>, theme: &ArtisanTheme) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(theme.typography.control_text)
        .font_weight(FontWeight::MEDIUM)
        .child(title.into())
}

pub(super) fn body_text(text: &str, theme: &ArtisanTheme) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(theme.typography.editor_text_desktop)
        .line_height(theme.spacing.steps(6.0))
        .whitespace_normal()
        .child(text.to_owned())
}

pub(super) fn changed_file_row(
    card_id: &SceneId,
    index: usize,
    file: &SceneFileChange,
    theme: &ArtisanTheme,
) -> AnyElement {
    let selector = changed_file_selector(card_id, index);
    let mut row = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(theme.spacing.steps(2.0));
    row = row.debug_selector(move || selector.clone());
    row = row
        .child(outline_badge(
            BadgeStyle::resolve(*theme),
            file_change_status_label(file.status),
        ))
        .child(body_text(&file.path, theme));
    row.into_any_element()
}

pub(super) fn status_color(theme: &ArtisanTheme, narration: TurnNarration) -> gpui::Hsla {
    match narration {
        TurnNarration::Failed | TurnNarration::Interrupted | TurnNarration::Cancelled => {
            theme.colors.destructive.to_paint()
        }
        _ => theme.colors.muted_foreground.to_paint(),
    }
}
