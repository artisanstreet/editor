//! Pure presentation and recovery policy for the settings model favorites.
//!
//! This is the native counterpart of the favorites projection and `Unstar`
//! effect in `routes/components/settings/models.svelte`. The eventual
//! adapter supplies the already-derived model choices and the Forge
//! availability observation. This module only preserves the source order and
//! values, and returns typed intents for the adapter to execute. It does not
//! know about a catalog decoder, transport, Effect, streams, or rendering.

#![allow(clippy::module_name_repetitions)]

/// The already-derived model fields read by the settings favorites view.
///
/// `ModelsFromCatalog` has already joined catalog definitions with provider
/// labels before this boundary. The native adapter therefore supplies these
/// final values directly. Every string is borrowed from that supplied model
/// list so display text and the optional native variant identifier remain
/// byte-for-byte unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelChoiceView<'a> {
    /// The stable catalog model identifier used by favorites.
    pub id: &'a str,
    /// The exact catalog display name.
    pub name: &'a str,
    /// The exact provider/lab display label.
    pub lab: &'a str,
    /// The provider identifier used by a later provider-mark adapter.
    pub provider: &'a str,
    /// The optional provider-native selection variant, if one was cataloged.
    pub variant_id: Option<&'a str>,
}

impl<'a> ModelChoiceView<'a> {
    /// Creates a borrowed, already-derived model choice.
    #[must_use]
    pub const fn new(
        id: &'a str,
        name: &'a str,
        lab: &'a str,
        provider: &'a str,
        variant_id: Option<&'a str>,
    ) -> Self {
        Self {
            id,
            name,
            lab,
            provider,
            variant_id,
        }
    }

    /// Returns the native variant identifier without changing or formatting it.
    #[must_use]
    pub const fn native_variant_id(self) -> Option<&'a str> {
        self.variant_id
    }
}

/// The renderer-facing state for the model favorites section.
///
/// `favorites` is projected in favorite-id order, not catalog order, and may
/// contain the same borrowed model more than once when the input favorite IDs
/// contain duplicates. An empty vector is the exact source empty state;
/// [`Self::is_empty`] exposes it without requiring a renderer to infer a
/// separate sentinel. `forge_available` is retained because it is the source
/// of both the unstar button's disabled state and unstar admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelFavoritesPresentation<'a> {
    /// Models selected by the ordered favorite-id projection.
    pub favorites: Vec<ModelChoiceView<'a>>,
    /// Whether the Forge-owned favorite mutation path is available.
    pub forge_available: bool,
}

impl<'a> ModelFavoritesPresentation<'a> {
    /// Creates a presentation from an already-projected favorite list.
    #[must_use]
    pub const fn new(favorites: Vec<ModelChoiceView<'a>>, forge_available: bool) -> Self {
        Self {
            favorites,
            forge_available,
        }
    }

    /// Returns whether the source section should expose its empty state.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.favorites.is_empty()
    }

    /// Returns the disabled state of the unstar control.
    #[must_use]
    pub const fn unstar_disabled(&self) -> bool {
        !self.forge_available
    }
}

/// Projects ordered favorite IDs onto the supplied model choices.
///
/// This preserves the source expression's `map`/`find`/`filter` semantics:
/// each favorite ID is looked up independently in the model list, the first
/// matching model wins, missing IDs are discarded, and no deduplication or
/// catalog-order sorting occurs. The returned choices borrow the original
/// model fields.
#[must_use]
pub fn project_model_favorites<'a, I>(
    favorite_ids: I,
    models: &[ModelChoiceView<'a>],
) -> Vec<ModelChoiceView<'a>>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    favorite_ids
        .into_iter()
        .filter_map(|favorite_id| {
            let favorite_id = favorite_id.as_ref();
            models.iter().find(|model| model.id == favorite_id).copied()
        })
        .collect()
}

/// Builds the settings favorites presentation from its already-derived inputs.
///
/// No catalog transformation is performed here. `models` is the output of
/// the model-selection layer, and `favorite_ids` is consumed only for lookup;
/// the resulting rows borrow from `models`.
#[must_use]
pub fn present_model_favorites<'a, I>(
    models: &[ModelChoiceView<'a>],
    favorite_ids: I,
    forge_available: bool,
) -> ModelFavoritesPresentation<'a>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    ModelFavoritesPresentation::new(
        project_model_favorites(favorite_ids, models),
        forge_available,
    )
}

/// The pure action selected for an attempted unstar operation.
///
/// `NoOp` is returned when Forge is unavailable, matching both the source
/// admission guard and the disabled button. An admitted operation carries the
/// exact durable update shape and always uses `favorite: false`; the adapter
/// owns transport execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnstarAction<'a> {
    /// Forge is unavailable, so no request should be made.
    NoOp,
    /// Request the exact `(model_id, false)` favorite mutation.
    Request {
        /// The catalog model identifier to unstar.
        model_id: &'a str,
        /// The requested final favorite state, always `false` here.
        favorite: bool,
    },
}

impl<'a> UnstarAction<'a> {
    /// Returns whether this attempted unstar is intentionally a no-op.
    #[must_use]
    pub const fn is_no_op(self) -> bool {
        matches!(self, Self::NoOp)
    }

    /// Returns the exact request tuple for an admitted unstar.
    #[must_use]
    pub const fn request(self) -> Option<(&'a str, bool)> {
        match self {
            Self::NoOp => None,
            Self::Request { model_id, favorite } => Some((model_id, favorite)),
        }
    }
}

/// Selects whether the unstar button may admit a request.
///
/// An unavailable Forge produces [`UnstarAction::NoOp`]. An available Forge
/// produces one request whose model ID is preserved exactly and whose desired
/// favorite state is exactly `false`.
#[must_use]
pub const fn unstar_action(forge_available: bool, model_id: &str) -> UnstarAction<'_> {
    if forge_available {
        UnstarAction::Request {
            model_id,
            favorite: false,
        }
    } else {
        UnstarAction::NoOp
    }
}

/// Returns the unstar control's disabled policy for an availability value.
#[must_use]
pub const fn unstar_control_disabled(forge_available: bool) -> bool {
    !forge_available
}

/// The pure completion instruction for an admitted unstar request.
///
/// A successful mutation replaces the local defaults state with the exact
/// state returned by `SetFavorite`. A failed mutation requests a fresh
/// `Current` read; it never fabricates or retains a locally guessed state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnstarCompletion<S> {
    /// Replace the component's defaults state with the returned value.
    ApplyDefaults {
        /// The exact successful state returned by the mutation.
        state: S,
    },
    /// Read and apply a fresh `Current` defaults state.
    RefreshCurrent,
}

impl<S> UnstarCompletion<S> {
    /// Borrows the successful replacement state, if the request succeeded.
    #[must_use]
    pub fn applied_state(&self) -> Option<&S> {
        match self {
            Self::ApplyDefaults { state } => Some(state),
            Self::RefreshCurrent => None,
        }
    }

    /// Returns whether the adapter must perform a fresh `Current` read.
    #[must_use]
    pub const fn requests_current_refresh(&self) -> bool {
        matches!(self, Self::RefreshCurrent)
    }
}

/// Converts a mutation result into the adapter instruction from the source.
///
/// The error value is intentionally discarded because the source catches the
/// request failure and recovers by reading `Current`. The returned successful
/// state is not merged or transformed.
#[must_use]
pub fn resolve_unstar_completion<S, E>(result: Result<S, E>) -> UnstarCompletion<S> {
    match result {
        Ok(state) => UnstarCompletion::ApplyDefaults { state },
        Err(_) => UnstarCompletion::RefreshCurrent,
    }
}

/// Builds the successful replacement instruction for an unstar mutation.
#[must_use]
pub fn unstar_succeeded<S>(state: S) -> UnstarCompletion<S> {
    UnstarCompletion::ApplyDefaults { state }
}

/// Builds the failure instruction requiring a fresh `Current` replacement.
#[must_use]
pub const fn unstar_failed<S>() -> UnstarCompletion<S> {
    UnstarCompletion::RefreshCurrent
}
