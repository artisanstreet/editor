//! Pure workspace tab and breadcrumb derivations.
//!
//! This is the native boundary for the behavior in
//! `lib/workspace/tab-model/derivations.ts`.  The input and output models keep
//! only the fields needed by those derivations instead of coupling this
//! boundary to the incomplete UI workspace state.

/// The small tab shape needed by the overflow derivation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceTab {
    /// Stable tab identity used for active-tab lookup and overflow
    /// membership.
    pub id: String,
}

/// The small workspace shape needed by the overflow derivation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceState {
    /// Tabs in their presentation order.
    pub tabs: Vec<WorkspaceTab>,
    /// The active tab identity, when one is selected.
    pub active_tab_id: Option<String>,
}

/// The owned result of deriving the visible and overflow tab lists.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TabOverflow {
    /// Tabs shown in the primary tab strip.
    pub visible: Vec<WorkspaceTab>,
    /// Tabs omitted from the primary strip, in their original state order.
    pub overflow: Vec<WorkspaceTab>,
}

/// The small file shape needed by breadcrumb derivation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceFileReference {
    /// Slash-delimited file path.
    pub path: String,
}

/// Errors for inputs that cannot be given a well-defined native limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabDerivationError {
    /// `max_visible` was `NaN`, positive infinity, or negative infinity.
    ///
    /// JavaScript's coercion of these values is not silently reproduced here:
    /// callers must provide a finite limit and handle this error explicitly.
    NonFiniteMaxVisible,
}

/// Derives the visible and overflow tabs.
///
/// For every finite `max_visible`, this mirrors the TypeScript expression
/// `Math.max(1, Math.trunc(max_visible))`.  A zero, negative, or positive
/// fraction therefore still permits one visible tab, while a positive
/// fraction is truncated before it determines the visible slice.  A limit
/// larger than the tab list simply makes the whole list visible.
///
/// If the active tab is outside the initial visible slice, the first tab with
/// that ID is promoted to the front and the last tab in that slice is
/// removed.  Overflow membership is ID-based, matching the TypeScript
/// `Set`: it preserves state ordering and excludes every state tab whose ID is
/// present in the final visible list.  A missing active ID causes no
/// promotion.
///
/// # Errors
///
/// Returns [`TabDerivationError::NonFiniteMaxVisible`] when `max_visible` is
/// `NaN` or infinite.
#[must_use = "handle the derived tab lists or limit error"]
pub fn derive_tab_overflow(
    state: &WorkspaceState,
    max_visible: f64,
) -> Result<TabOverflow, TabDerivationError> {
    let limit = visible_limit(max_visible, state.tabs.len())?;

    if state.tabs.len() <= limit {
        return Ok(TabOverflow {
            visible: state.tabs.clone(),
            overflow: Vec::new(),
        });
    }

    let mut visible: Vec<WorkspaceTab> = state.tabs.iter().take(limit).cloned().collect();

    if let Some(active_tab_id) = state.active_tab_id.as_deref() {
        let active_is_visible = visible.iter().any(|tab| tab.id == active_tab_id);
        if !active_is_visible
            && let Some(active_tab) = state.tabs.iter().find(|tab| tab.id == active_tab_id)
        {
            // `limit >= 1` and `state.tabs.len() > limit` guarantee that the
            // initial slice has a last element to replace.
            visible.pop();
            visible.insert(0, active_tab.clone());
        }
    }

    let overflow = state
        .tabs
        .iter()
        .filter(|tab| !visible.iter().any(|visible_tab| visible_tab.id == tab.id))
        .cloned()
        .collect();

    Ok(TabOverflow { visible, overflow })
}

/// Splits a file path into non-empty slash-delimited breadcrumb segments.
///
/// This intentionally preserves segment order and ignores empty segments
/// caused by leading, trailing, or repeated slashes.
#[must_use]
pub fn derive_breadcrumbs(file: &WorkspaceFileReference) -> Vec<String> {
    file.path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Applies JavaScript's truncation/minimum-one rule without narrowing a large
/// finite `f64` to `usize` before it is known to fit.
fn visible_limit(max_visible: f64, tab_count: usize) -> Result<usize, TabDerivationError> {
    if !max_visible.is_finite() {
        return Err(TabDerivationError::NonFiniteMaxVisible);
    }

    let normalized = max_visible.trunc().max(1.0);
    if tab_count == 0 {
        // Keep the logical minimum-one rule even though there is nothing to
        // slice.  This also avoids making an empty state a special invalid
        // limit case.
        return Ok(1);
    }
    #[allow(clippy::cast_precision_loss)]
    let tab_count_as_float = tab_count as f64;
    if normalized >= tab_count_as_float {
        return Ok(tab_count);
    }

    // `normalized < tab_count_as_float` above makes this conversion bounded
    // by the platform's addressable vector length.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(normalized as usize)
}
