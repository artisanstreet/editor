import { Context, Effect, Layer, Semaphore, Stream, SubscriptionRef } from "effect";

import {
	inherited_compaction_model,
	SessionPolicyPermission,
	type RuntimeCatalog,
	type SessionDefaults,
	type SessionDefaultsUpdateInput,
	type SessionModelDefaultsUpdate,
	type ThreadSessionPolicy,
} from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";
import {
	IsOfflineRuntimeCatalog,
	OfflineRuntimeCatalog,
	WithOfflineRuntimeCatalog,
} from "../runtime/offline-catalog";

export type CompactionSelection =
	| { readonly _tag: "Curated" }
	| { readonly _tag: "Inherited" }
	| { readonly _tag: "Explicit"; readonly model_id: string };

export interface SessionDefaultsState {
	readonly available: boolean;
	readonly catalog: RuntimeCatalog;
	readonly defaults: SessionDefaults;
	readonly favorite_ids: ReadonlyArray<string>;
}

export interface SaveCompactionDefaultsInput {
	readonly model?: SessionModelDefaultsUpdate;
	readonly permission?: string;
	readonly selection: CompactionSelection;
}

const EmptyDefaults: SessionDefaults = {
	auto_continue_usage_limits: true,
	models: [],
	permission: "supervised",
};

export const CompactionSelectionFromDefaults = (defaults: SessionDefaults): CompactionSelection =>
	defaults.compaction_model === undefined
		? { _tag: "Curated" }
		: defaults.compaction_model === inherited_compaction_model
			? { _tag: "Inherited" }
			: { _tag: "Explicit", model_id: defaults.compaction_model };

const CompactionModelValue = (selection: CompactionSelection): string | null =>
	selection._tag === "Curated"
		? null
		: selection._tag === "Inherited"
			? inherited_compaction_model
			: selection.model_id;

export class SessionDefaultsController extends Context.Service<
	SessionDefaultsController,
	{
		readonly Changes: Stream.Stream<SessionDefaultsState>;
		readonly Current: Effect.Effect<SessionDefaultsState>;
		readonly Refresh: Effect.Effect<SessionDefaultsState, ArtisanClientError>;
		readonly RememberPolicyDefaults: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<SessionDefaultsState, ArtisanClientError>;
		readonly SaveCompactionDefaults: (
			input: SaveCompactionDefaultsInput,
		) => Effect.Effect<SessionDefaultsState, ArtisanClientError>;
		/**
		 * Switches one engine's availability. Disabled engines are represented
		 * nowhere — selectors, usage reads, and settings treat them as absent.
		 */
		readonly SetEngineEnabled: (
			engine_id: string,
			enabled: boolean,
		) => Effect.Effect<SessionDefaultsState, ArtisanClientError>;
		readonly SetFavorite: (
			model_id: string,
			favorite: boolean,
		) => Effect.Effect<SessionDefaultsState, ArtisanClientError>;
		/** Default captured by newly created provider-usage interruptions. */
		readonly SetAutoContinueUsageLimits: (
			auto_continue_usage_limits: boolean,
		) => Effect.Effect<SessionDefaultsState, ArtisanClientError>;
	}
>()("Artisan/SessionDefaultsController") {}

export const SessionDefaultsControllerLive = Layer.effect(
	SessionDefaultsController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const state = yield* SubscriptionRef.make<SessionDefaultsState>({
			available: false,
			catalog: OfflineRuntimeCatalog,
			defaults: EmptyDefaults,
			favorite_ids: [],
		});
		const mutation_lock = yield* Semaphore.make(1);

		const Current = Effect.gen(function* () {
			return yield* SubscriptionRef.get(state);
		});

		const RefreshUnlocked = Effect.gen(function* () {
			const catalog = yield* WithOfflineRuntimeCatalog(client.GetRuntimeCatalog);
			const defaults = yield* client.GetSessionDefaults;
			const favorites = yield* client.GetModelFavorites;
			const next = {
				available: !IsOfflineRuntimeCatalog(catalog),
				catalog,
				defaults,
				favorite_ids: favorites.model_ids,
			} satisfies SessionDefaultsState;
			yield* SubscriptionRef.set(state, next);
			return next;
		});

		/** Reads and mutations share one publication order; stale hydration cannot win. */
		const Refresh = Effect.gen(function* () {
			return yield* mutation_lock.withPermit(RefreshUnlocked);
		});

		const SaveDefaultsUnlocked = (update: SessionDefaultsUpdateInput) =>
			Effect.gen(function* () {
				yield* client.UpdateSessionDefaults(update);
				const defaults = yield* client.GetSessionDefaults;
				return yield* SubscriptionRef.updateAndGet(state, (current) => ({
					...current,
					defaults,
				}));
			});

		const SaveDefaults = (update: SessionDefaultsUpdateInput) =>
			Effect.gen(function* () {
				return yield* mutation_lock.withPermit(SaveDefaultsUnlocked(update));
			});

		const SetEngineEnabled = (engine_id: string, enabled: boolean) =>
			Effect.gen(function* () {
				return yield* SaveDefaults({ engine: { enabled, engine_id } });
			});

		const SetFavorite = (model_id: string, favorite: boolean) =>
			Effect.gen(function* () {
				return yield* mutation_lock.withPermit(
					Effect.gen(function* () {
						yield* client.UpdateModelFavorite({ favorite, model_id });
						const favorites = yield* client.GetModelFavorites;
						return yield* SubscriptionRef.updateAndGet(state, (current) => ({
							...current,
							favorite_ids: favorites.model_ids,
						}));
					}),
				);
			});

		const SetAutoContinueUsageLimits = (auto_continue_usage_limits: boolean) =>
			SaveDefaults({ auto_continue_usage_limits });

		const RememberPolicyDefaults = (policy: ThreadSessionPolicy) =>
			Effect.gen(function* () {
				const current = yield* Current;
				const model_id = current.catalog.manifest.models.find(
					(candidate) => candidate.native_model_id === policy.model,
				)?.id;
				return yield* SaveDefaults({
					...(policy.model === undefined ? {} : { last_model_id: policy.model }),
					...(model_id === undefined
						? {}
						: {
								model: {
									...(policy.context_window === undefined
										? {}
										: { context_window: policy.context_window }),
									model_id,
									reasoning_effort: policy.reasoning_effort,
								},
							}),
					permission: SessionPolicyPermission(policy),
				});
			});

		const SaveCompactionDefaults = (input: SaveCompactionDefaultsInput) =>
			Effect.gen(function* () {
				return yield* SaveDefaults({
					compaction_model: CompactionModelValue(input.selection),
					...(input.model === undefined ? {} : { model: input.model }),
					...(input.permission === undefined ? {} : { permission: input.permission }),
				});
			});

		return SessionDefaultsController.of({
			Changes: SubscriptionRef.changes(state),
			Current,
			Refresh,
			RememberPolicyDefaults,
			SaveCompactionDefaults,
			SetEngineEnabled,
			SetAutoContinueUsageLimits,
			SetFavorite,
		});
	}),
);
