import { Context, Deferred, Effect, Layer, Ref, Semaphore, Stream, SubscriptionRef } from "effect";

import {
	DefaultThreadTitleMode,
	inherited_compaction_model,
	SessionPolicyPermission,
	type AgentNameDataset,
	type RuntimeCatalog,
	type SessionDefaults,
	type SessionDefaultsUpdateInput,
	type SessionModelDefaultsUpdate,
	type ThreadSessionPolicy,
	type ThreadTitleMode,
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

type SessionDefaultsRefreshClaim =
	| {
			readonly _tag: "Follower";
			readonly deferred: Deferred.Deferred<SessionDefaultsState, ArtisanClientError>;
	  }
	| {
			readonly _tag: "Leader";
			readonly deferred: Deferred.Deferred<SessionDefaultsState, ArtisanClientError>;
	  };

interface ReconciliationState {
	readonly requested_generation: number;
	readonly requested_revision: number;
	readonly running: boolean;
}

const EmptyDefaults: SessionDefaults = {
	agent_name_dataset: "norwegian",
	auto_continue_usage_limits: true,
	models: [],
	permission: "supervised",
	thread_title_mode: DefaultThreadTitleMode,
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
		readonly SetAgentNameDataset: (
			agent_name_dataset: AgentNameDataset,
		) => Effect.Effect<SessionDefaultsState, ArtisanClientError>;
		/** Default captured by newly created provider-usage interruptions. */
		readonly SetAutoContinueUsageLimits: (
			auto_continue_usage_limits: boolean,
		) => Effect.Effect<SessionDefaultsState, ArtisanClientError>;
		/** How thread rows are titled: harness summary or latest user message. */
		readonly SetThreadTitleMode: (
			thread_title_mode: ThreadTitleMode,
		) => Effect.Effect<SessionDefaultsState, ArtisanClientError>;
	}
>()("Artisan/SessionDefaultsController") {}

export const SessionDefaultsControllerLive = Layer.effect(
	SessionDefaultsController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const state = yield* SubscriptionRef.make<SessionDefaultsState>({
			available: false,
			catalog: OfflineRuntimeCatalog,
			defaults: EmptyDefaults,
			favorite_ids: [],
		});
		const mutation_lock = yield* Semaphore.make(1);
		/** Accepted receipts advance their domain's generation before publishing a local patch. */
		const defaults_generation = yield* Ref.make(0);
		const favorites_generation = yield* Ref.make(0);
		/** A failed receipt schedules one app-owned corrective read per domain. */
		const defaults_reconciliation = yield* Ref.make<ReconciliationState>({
			requested_generation: -1,
			requested_revision: 0,
			running: false,
		});
		const favorites_reconciliation = yield* Ref.make<ReconciliationState>({
			requested_generation: -1,
			requested_revision: 0,
			running: false,
		});
		/** One cold/recovery read is shared by every mounting surface. */
		const refresh_inflight = yield* Ref.make<
			Deferred.Deferred<SessionDefaultsState, ArtisanClientError> | undefined
		>(undefined);

		const Current = Effect.gen(function* () {
			return yield* SubscriptionRef.get(state);
		});

		/** Applies the durable command's patch without waiting for its journal event to arrive. */
		const PatchDefaults = (defaults: SessionDefaults, update: SessionDefaultsUpdateInput) => {
			const disabled_engine_ids = new Set(defaults.disabled_engines ?? []);
			if (update.engine !== undefined) {
				if (update.engine.enabled) disabled_engine_ids.delete(update.engine.engine_id);
				else disabled_engine_ids.add(update.engine.engine_id);
			}

			const without_compaction = (() => {
				const { compaction_model: _compaction_model, ...remaining } = defaults;
				return remaining;
			})();
			const shared =
				update.compaction_model === undefined
					? defaults
					: update.compaction_model === null
						? without_compaction
						: { ...without_compaction, compaction_model: update.compaction_model };
			const models =
				update.model === undefined
					? shared.models
					: (() => {
							const existing = shared.models.find(
								(candidate) => candidate.model_id === update.model?.model_id,
							);
							const model = update.model;
							if (model === undefined) return shared.models;
							const next = {
								model_id: model.model_id,
								...(model.context_window === undefined
									? existing?.context_window === undefined
										? {}
										: { context_window: existing.context_window }
									: model.context_window === null
										? {}
										: { context_window: model.context_window }),
								...(model.reasoning_effort === undefined
									? existing?.reasoning_effort === undefined
										? {}
										: { reasoning_effort: existing.reasoning_effort }
									: model.reasoning_effort === null
										? {}
										: { reasoning_effort: model.reasoning_effort }),
								...(model.service_tier === undefined
									? existing?.service_tier === undefined
										? {}
										: { service_tier: existing.service_tier }
									: model.service_tier === null
										? {}
										: { service_tier: model.service_tier }),
							};
							return [
								...shared.models.filter(
									(candidate) => candidate.model_id !== model.model_id,
								),
								next,
							].toSorted((left, right) =>
								left.model_id.localeCompare(right.model_id),
							);
						})();
			const { disabled_engines: _disabled_engines, ...shared_without_disabled_engines } =
				shared;

			return {
				...shared_without_disabled_engines,
				...(update.agent_name_dataset === undefined
					? {}
					: { agent_name_dataset: update.agent_name_dataset }),
				...(update.auto_continue_usage_limits === undefined
					? {}
					: { auto_continue_usage_limits: update.auto_continue_usage_limits }),
				...(disabled_engine_ids.size === 0
					? {}
					: { disabled_engines: [...disabled_engine_ids].toSorted() }),
				...(update.last_model_id === undefined
					? {}
					: { last_model_id: update.last_model_id }),
				models,
				...(update.permission === undefined ? {} : { permission: update.permission }),
				...(update.thread_title_mode === undefined
					? {}
					: { thread_title_mode: update.thread_title_mode }),
			} satisfies SessionDefaults;
		};

		const ScheduleDefaultsReconciliation = Effect.uninterruptibleMask((restore) =>
			Effect.gen(function* () {
				const generation = yield* Ref.get(defaults_generation);
				const start = yield* Ref.modify(
					defaults_reconciliation,
					(current) =>
						[
							!current.running,
							{
								requested_generation: generation,
								requested_revision: current.requested_revision + 1,
								running: true,
							},
						] as const,
				);
				if (!start) return;

				const Reconcile = Effect.gen(function* () {
					while (true) {
						const requested = yield* Ref.get(defaults_reconciliation);
						const defaults = yield* client.GetSessionDefaults.pipe(Effect.option);
						if (defaults._tag === "Some") {
							yield* mutation_lock.withPermit(
								Effect.gen(function* () {
									const generation = yield* Ref.get(defaults_generation);
									const current = yield* Ref.get(defaults_reconciliation);
									if (
										generation !== requested.requested_generation ||
										current.requested_revision !== requested.requested_revision
									)
										return;
									yield* SubscriptionRef.update(state, (current) => ({
										...current,
										defaults: defaults.value,
									}));
								}),
							);
						}
						const continue_reconciling = yield* Ref.modify(
							defaults_reconciliation,
							(current) =>
								current.requested_revision === requested.requested_revision
									? [false, { ...current, running: false }]
									: [true, current],
						);
						if (!continue_reconciling) return;
					}
				});
				yield* Effect.forkIn(restore(Reconcile), controller_scope);
			}),
		);

		const ScheduleFavoritesReconciliation = Effect.uninterruptibleMask((restore) =>
			Effect.gen(function* () {
				const generation = yield* Ref.get(favorites_generation);
				const start = yield* Ref.modify(
					favorites_reconciliation,
					(current) =>
						[
							!current.running,
							{
								requested_generation: generation,
								requested_revision: current.requested_revision + 1,
								running: true,
							},
						] as const,
				);
				if (!start) return;

				const Reconcile = Effect.gen(function* () {
					while (true) {
						const requested = yield* Ref.get(favorites_reconciliation);
						const favorites = yield* client.GetModelFavorites.pipe(Effect.option);
						if (favorites._tag === "Some") {
							yield* mutation_lock.withPermit(
								Effect.gen(function* () {
									const generation = yield* Ref.get(favorites_generation);
									const current = yield* Ref.get(favorites_reconciliation);
									if (
										generation !== requested.requested_generation ||
										current.requested_revision !== requested.requested_revision
									)
										return;
									yield* SubscriptionRef.update(state, (current) => ({
										...current,
										favorite_ids: favorites.value.model_ids,
									}));
								}),
							);
						}
						const continue_reconciling = yield* Ref.modify(
							favorites_reconciliation,
							(current) =>
								current.requested_revision === requested.requested_revision
									? [false, { ...current, running: false }]
									: [true, current],
						);
						if (!continue_reconciling) return;
					}
				});
				yield* Effect.forkIn(restore(Reconcile), controller_scope);
			}),
		);

		const RefreshUnlocked = Effect.gen(function* () {
			const [catalog, defaults, favorites] = yield* Effect.all(
				[
					WithOfflineRuntimeCatalog(client.GetRuntimeCatalog),
					client.GetSessionDefaults,
					client.GetModelFavorites,
				],
				{ concurrency: "unbounded" },
			);
			const next = {
				available: !IsOfflineRuntimeCatalog(catalog),
				catalog,
				defaults,
				favorite_ids: favorites.model_ids,
			} satisfies SessionDefaultsState;
			yield* SubscriptionRef.set(state, next);
			return next;
		});

		/**
		 * Reads and mutations share one publication order; stale hydration cannot win.
		 * Concurrent readers await the same deferred rather than queueing redundant
		 * catalog/default/favorite RPC triples behind one another.
		 */
		const Refresh = Effect.uninterruptibleMask((restore) =>
			Effect.gen(function* () {
				const candidate = yield* Deferred.make<SessionDefaultsState, ArtisanClientError>();
				const claim = yield* Ref.modify<
					Deferred.Deferred<SessionDefaultsState, ArtisanClientError> | undefined,
					SessionDefaultsRefreshClaim
				>(
					refresh_inflight,
					(
						current,
					): readonly [
						SessionDefaultsRefreshClaim,
						Deferred.Deferred<SessionDefaultsState, ArtisanClientError> | undefined,
					] =>
						current === undefined
							? [{ _tag: "Leader", deferred: candidate }, candidate]
							: [{ _tag: "Follower", deferred: current }, current],
				);
				if (claim._tag === "Follower")
					return yield* restore(Deferred.await(claim.deferred));

				yield* Effect.forkIn(
					mutation_lock.withPermit(RefreshUnlocked).pipe(
						Effect.exit,
						Effect.flatMap((exit) =>
							Ref.update(refresh_inflight, (current) =>
								current === claim.deferred ? undefined : current,
							).pipe(Effect.andThen(Deferred.done(claim.deferred, exit))),
						),
						Effect.asVoid,
					),
					controller_scope,
				);
				return yield* restore(Deferred.await(claim.deferred));
			}),
		);

		const SaveDefaultsUnlocked = (update: SessionDefaultsUpdateInput) =>
			Effect.gen(function* () {
				yield* client.UpdateSessionDefaults(update).pipe(
					Effect.catch((error) =>
						Effect.gen(function* () {
							yield* ScheduleDefaultsReconciliation;
							return yield* Effect.fail(error);
						}),
					),
				);
				yield* Ref.update(defaults_generation, (generation) => generation + 1);
				return yield* SubscriptionRef.updateAndGet(state, (current) => ({
					...current,
					defaults: PatchDefaults(current.defaults, update),
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
						yield* client.UpdateModelFavorite({ favorite, model_id }).pipe(
							Effect.catch((error) =>
								Effect.gen(function* () {
									yield* ScheduleFavoritesReconciliation;
									return yield* Effect.fail(error);
								}),
							),
						);
						yield* Ref.update(favorites_generation, (generation) => generation + 1);
						return yield* SubscriptionRef.updateAndGet(state, (current) => ({
							...current,
							favorite_ids: favorite
								? current.favorite_ids.includes(model_id)
									? current.favorite_ids
									: [...current.favorite_ids, model_id]
								: current.favorite_ids.filter(
										(candidate) => candidate !== model_id,
									),
						}));
					}),
				);
			});

		const SetAgentNameDataset = (agent_name_dataset: AgentNameDataset) =>
			SaveDefaults({ agent_name_dataset });

		const SetAutoContinueUsageLimits = (auto_continue_usage_limits: boolean) =>
			SaveDefaults({ auto_continue_usage_limits });

		const SetThreadTitleMode = (thread_title_mode: ThreadTitleMode) =>
			SaveDefaults({ thread_title_mode });

		const RememberPolicyDefaults = (policy: ThreadSessionPolicy) =>
			Effect.gen(function* () {
				const current = yield* Current;
				const model_id = current.catalog.manifest.models.find(
					(candidate) =>
						candidate.harness === policy.engine_id &&
						candidate.native_model_id === policy.model,
				)?.id;
				return yield* SaveDefaults({
					...(model_id === undefined
						? {}
						: {
								last_model_id: model_id,
								model: {
									...(policy.context_window === undefined
										? {}
										: { context_window: policy.context_window }),
									model_id,
									reasoning_effort: policy.reasoning_effort,
									service_tier: policy.service_tier,
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
			SetAgentNameDataset,
			SetAutoContinueUsageLimits,
			SetFavorite,
			SetThreadTitleMode,
		});
	}),
);
