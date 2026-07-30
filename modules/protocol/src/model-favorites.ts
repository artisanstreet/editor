import { Schema } from "effect";

/**
 * Names one catalog model the user has starred. Favorites are keyed by catalog
 * model id rather than native provider id: a provider may rename or re-point a
 * native id, but the catalog entry is the thing the picker actually lists.
 */
export const ModelFavoriteId = Schema.NonEmptyString;

export type ModelFavoriteId = typeof ModelFavoriteId.Type;

/**
 * The complete set of starred models. Favorites are Forge-owned so every
 * client — desktop, browser, or remote — opens the picker to the same order,
 * and the set stays whole rather than arriving as per-model fragments.
 */
export const ModelFavoritesSnapshot = Schema.Struct({
	model_ids: Schema.Array(ModelFavoriteId),
});

export type ModelFavoritesSnapshot = typeof ModelFavoritesSnapshot.Type;

/**
 * Stars or unstars one model through the durable command path. The command
 * carries the intended end state rather than a toggle so a retried or
 * duplicated delivery cannot flip the star back.
 */
export const ModelFavoriteUpdateCommand = Schema.Struct({
	favorite: Schema.Boolean,
	model_id: ModelFavoriteId,
	type: Schema.Literal("model.favorite.update"),
});

export type ModelFavoriteUpdateCommand = typeof ModelFavoriteUpdateCommand.Type;

/** Records a durable change to the starred model set. */
export const ModelFavoritesUpdatedEvent = Schema.Struct({
	favorites: ModelFavoritesSnapshot,
	type: Schema.Literal("model.favorites.updated"),
});

export type ModelFavoritesUpdatedEvent = typeof ModelFavoritesUpdatedEvent.Type;
