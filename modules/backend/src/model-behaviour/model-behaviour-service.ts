import { createHash } from "node:crypto";

import { Context, Data, Effect, Layer, Option, Schema, Semaphore } from "effect";

import {
	ModelBehaviourSnapshot,
	type ModelBehaviourDriftResolutionRequest,
	type ModelBehaviourProviderState,
	type ModelBehaviourRetryRequest,
	type ModelBehaviourSettingId,
	type ModelBehaviourUpdateRequest,
	type ModelBehaviourValue,
} from "@artisan/protocol";

import { RuntimeMetadata } from "../runtime/runtime-metadata";
import {
	ModelBehaviourRepository,
	type ModelBehaviourCommitResult,
	type ModelBehaviourEvent,
	type ModelBehaviourOperation,
	type ModelBehaviourProviderCommit,
	type ModelBehaviourReadResult,
	type ModelBehaviourRepositoryError,
} from "./model-behaviour-repository";
import {
	ModelBehaviourProviderRegistry,
	type ModelBehaviourProviderAdapter,
	type ModelBehaviourProviderApplyResult,
	type ModelBehaviourProviderError,
	type ModelBehaviourProviderObservation,
} from "./model-behaviour-provider";
import { hash_model_behaviour_value } from "./model-behaviour-value";

/** Supplies durable operation identity for a Model Behaviour mutation. */
export interface ModelBehaviourMutationTrace {
	readonly message_id: string;
	readonly origin: "backend" | "frontend";
	readonly sent_at: string;
}

/** Reports a rejected Model Behaviour intent without changing provider files. */
export class ModelBehaviourConflict extends Data.TaggedError("ModelBehaviourConflict")<{
	readonly code:
		| "no_supported_provider"
		| "provider_not_found"
		| "provider_not_supported"
		| "provider_unavailable"
		| "stale_observation";
	readonly message: string;
}> {}

/** Reports an impossible canonical snapshot or registry state. */
export class ModelBehaviourInvariantError extends Data.TaggedError("ModelBehaviourInvariantError")<{
	readonly message: string;
}> {}

/** Returns the exact durable events and current snapshot for one mutation. */
export interface ModelBehaviourMutationResult {
	readonly events: ReadonlyArray<ModelBehaviourEvent>;
	readonly snapshot: typeof ModelBehaviourSnapshot.Type;
	readonly status: "accepted" | "duplicate";
}

/** Represents every recoverable Model Behaviour service failure. */
export type ModelBehaviourServiceError =
	| ModelBehaviourConflict
	| ModelBehaviourInvariantError
	| ModelBehaviourRepositoryError;

/** Owns canonical Model Behaviour values and provider reconciliation policy. */
export class ModelBehaviourService extends Context.Service<
	ModelBehaviourService,
	{
		readonly Get: Effect.Effect<typeof ModelBehaviourSnapshot.Type, ModelBehaviourServiceError>;
		readonly ResolveDrift: (
			input: ModelBehaviourDriftResolutionRequest & ModelBehaviourMutationTrace,
		) => Effect.Effect<ModelBehaviourMutationResult, ModelBehaviourServiceError>;
		readonly RetrySync: (
			input: ModelBehaviourRetryRequest & ModelBehaviourMutationTrace,
		) => Effect.Effect<ModelBehaviourMutationResult, ModelBehaviourServiceError>;
		readonly Update: (
			input: ModelBehaviourUpdateRequest & ModelBehaviourMutationTrace,
		) => Effect.Effect<ModelBehaviourMutationResult, ModelBehaviourServiceError>;
	}
>()("Artisan/ModelBehaviourService") {}

type ProviderState = ModelBehaviourProviderCommit;

function request_fingerprint(kind: string, payload: unknown) {
	return createHash("sha256")
		.update(JSON.stringify({ kind, payload, version: 1 }))
		.digest("hex");
}

function operation(
	trace: ModelBehaviourMutationTrace,
	kind: string,
	payload: unknown,
): ModelBehaviourOperation {
	return {
		message_id: trace.message_id,
		origin: trace.origin,
		request_fingerprint: request_fingerprint(kind, payload),
		sent_at: trace.sent_at,
	};
}

function is_writeable(provider: ModelBehaviourProviderAdapter) {
	return provider.mapping.state === "supported" || provider.mapping.state === "experimental";
}

function find_setting(read: ModelBehaviourReadResult, setting_id: ModelBehaviourSettingId) {
	return Option.fromUndefinedOr(
		read.settings.find((setting) => setting.setting_id === setting_id),
	);
}

function find_provider_state(
	read: ModelBehaviourReadResult,
	provider: ModelBehaviourProviderAdapter,
) {
	return read.provider_states.find(
		(state) =>
			state.provider_id === provider.mapping.provider_id &&
			state.setting_id === provider.mapping.setting_id,
	);
}

function compact_state(state: ModelBehaviourProviderState): ProviderState {
	return {
		...(state.applied_hash === undefined ? {} : { applied_hash: state.applied_hash }),
		...(state.backup_path === undefined ? {} : { backup_path: state.backup_path }),
		...(state.ignored_drift_hash === undefined
			? {}
			: { ignored_drift_hash: state.ignored_drift_hash }),
		...(state.last_error_code === undefined ? {} : { last_error_code: state.last_error_code }),
		...(state.native_key === undefined ? {} : { native_key: state.native_key }),
		...(state.observed_hash === undefined ? {} : { observed_hash: state.observed_hash }),
		provider_id: state.provider_id,
		setting_id: state.setting_id,
		status: state.status,
		...(state.target_path === undefined ? {} : { target_path: state.target_path }),
	};
}

function state_changed(previous: ModelBehaviourProviderState | undefined, next: ProviderState) {
	return (
		previous === undefined || JSON.stringify(compact_state(previous)) !== JSON.stringify(next)
	);
}

function static_state(
	provider: ModelBehaviourProviderAdapter,
	previous?: ModelBehaviourProviderState,
): ProviderState {
	const status =
		provider.mapping.state === "runtime_only"
			? ("runtime_only" as const)
			: provider.mapping.state === "unsupported"
				? ("unsupported" as const)
				: ("version_unavailable" as const);

	return {
		...(provider.mapping.native_key === undefined
			? {}
			: { native_key: provider.mapping.native_key }),
		provider_id: provider.mapping.provider_id,
		setting_id: provider.mapping.setting_id,
		status,
		...(previous?.target_path === undefined ? {} : { target_path: previous.target_path }),
	};
}

function failed_state(
	provider: ModelBehaviourProviderAdapter,
	error: ModelBehaviourProviderError,
	previous?: ModelBehaviourProviderState,
	observation?: ModelBehaviourProviderObservation,
): ProviderState {
	return {
		...(previous?.applied_hash === undefined ? {} : { applied_hash: previous.applied_hash }),
		...(previous?.backup_path === undefined ? {} : { backup_path: previous.backup_path }),
		last_error_code: error.code,
		...(provider.mapping.native_key === undefined
			? {}
			: { native_key: provider.mapping.native_key }),
		...(observation?.observed_hash === undefined
			? previous?.observed_hash === undefined
				? {}
				: { observed_hash: previous.observed_hash }
			: { observed_hash: observation.observed_hash }),
		provider_id: provider.mapping.provider_id,
		setting_id: provider.mapping.setting_id,
		status: "sync_failed",
		...(observation?.target_path === undefined
			? previous?.target_path === undefined
				? {}
				: { target_path: previous.target_path }
			: { target_path: observation.target_path }),
	};
}

function synchronized_state(
	provider: ModelBehaviourProviderAdapter,
	desired: ModelBehaviourValue,
	result: ModelBehaviourProviderApplyResult,
	previous?: ModelBehaviourProviderState,
): ProviderState {
	const observation = result.observation;
	const backup_path = result._tag === "Written" ? result.backup_path : previous?.backup_path;

	if (result._tag === "Changed") {
		return {
			...(previous?.applied_hash === undefined
				? {}
				: { applied_hash: previous.applied_hash }),
			...(previous?.backup_path === undefined ? {} : { backup_path: previous.backup_path }),
			...(provider.mapping.native_key === undefined
				? {}
				: { native_key: provider.mapping.native_key }),
			observed_hash: observation.observed_hash,
			provider_id: provider.mapping.provider_id,
			setting_id: provider.mapping.setting_id,
			status: "drift_detected",
			target_path: observation.target_path,
		};
	}

	return {
		applied_hash: observation.observed_hash,
		...(backup_path === undefined ? {} : { backup_path }),
		...(provider.mapping.native_key === undefined
			? {}
			: { native_key: provider.mapping.native_key }),
		observed_hash: observation.observed_hash,
		provider_id: provider.mapping.provider_id,
		setting_id: provider.mapping.setting_id,
		status: desired.type === "provider_default" ? "provider_default" : "synced",
		target_path: observation.target_path,
	};
}

function observed_state(
	provider: ModelBehaviourProviderAdapter,
	desired: ModelBehaviourValue,
	observation: ModelBehaviourProviderObservation,
	previous?: ModelBehaviourProviderState,
): ProviderState {
	const desired_hash = hash_model_behaviour_value(desired);

	if (observation.observed_hash === desired_hash) {
		return {
			applied_hash: desired_hash,
			...(previous?.backup_path === undefined ? {} : { backup_path: previous.backup_path }),
			...(provider.mapping.native_key === undefined
				? {}
				: { native_key: provider.mapping.native_key }),
			observed_hash: observation.observed_hash,
			provider_id: provider.mapping.provider_id,
			setting_id: provider.mapping.setting_id,
			status: desired.type === "provider_default" ? "provider_default" : "synced",
			target_path: observation.target_path,
		};
	}

	const ignored = previous?.ignored_drift_hash === observation.observed_hash;

	return {
		...(previous?.applied_hash === undefined ? {} : { applied_hash: previous.applied_hash }),
		...(previous?.backup_path === undefined ? {} : { backup_path: previous.backup_path }),
		...(ignored ? { ignored_drift_hash: observation.observed_hash } : {}),
		...(provider.mapping.native_key === undefined
			? {}
			: { native_key: provider.mapping.native_key }),
		observed_hash: observation.observed_hash,
		provider_id: provider.mapping.provider_id,
		setting_id: provider.mapping.setting_id,
		status: ignored ? "drift_ignored" : "drift_detected",
		target_path: observation.target_path,
	};
}

function InspectProvider(provider: ModelBehaviourProviderAdapter) {
	return provider.Inspect.pipe(
		Effect.map((observation) => ({ _tag: "Observed" as const, observation })),
		Effect.catch((error) => Effect.succeed({ _tag: "Failed" as const, error })),
	);
}

function ApplyProvider(
	provider: ModelBehaviourProviderAdapter,
	desired: ModelBehaviourValue,
	operation_id: string,
	previous?: ModelBehaviourProviderState,
) {
	return Effect.gen(function* () {
		if (!is_writeable(provider)) {
			return static_state(provider, previous);
		}

		const inspected = yield* InspectProvider(provider);

		if (inspected._tag === "Failed") {
			return failed_state(provider, inspected.error, previous);
		}

		const observation = inspected.observation;
		const applied = yield* provider
			.Apply({
				...(observation.document_hash === undefined
					? {}
					: { expected_document_hash: observation.document_hash }),
				expected_observed_hash: observation.observed_hash,
				operation_id,
				value: desired,
			})
			.pipe(
				Effect.map((result) => ({ _tag: "Applied" as const, result })),
				Effect.catch((error) => Effect.succeed({ _tag: "Failed" as const, error })),
			);

		return applied._tag === "Failed"
			? failed_state(provider, applied.error, previous, observation)
			: synchronized_state(provider, desired, applied.result, previous);
	});
}

function ObserveProvider(
	provider: ModelBehaviourProviderAdapter,
	desired: ModelBehaviourValue,
	previous?: ModelBehaviourProviderState,
) {
	if (!is_writeable(provider)) {
		return Effect.succeed(static_state(provider, previous));
	}

	return InspectProvider(provider).pipe(
		Effect.map((result) =>
			result._tag === "Failed"
				? failed_state(provider, result.error, previous)
				: observed_state(provider, desired, result.observation, previous),
		),
	);
}

function same_value(left: ModelBehaviourValue, right: ModelBehaviourValue) {
	return hash_model_behaviour_value(left) === hash_model_behaviour_value(right);
}

/** Builds the canonical Model Behaviour service over repositories and provider adapters. */
export const ModelBehaviourServiceLive = Layer.effect(
	ModelBehaviourService,
	Effect.gen(function* () {
		const repository = yield* ModelBehaviourRepository;
		const providers = yield* ModelBehaviourProviderRegistry;
		const metadata = yield* RuntimeMetadata;
		const lock = yield* Semaphore.make(1);

		const Snapshot = (read: ModelBehaviourReadResult) =>
			Schema.decodeUnknownEffect(ModelBehaviourSnapshot, {
				onExcessProperty: "error",
			})({
				capabilities: providers.Capabilities,
				providers: read.provider_states,
				registry_version: 1,
				settings: read.settings,
			}).pipe(
				Effect.mapError(
					() =>
						new ModelBehaviourInvariantError({
							message: "The Model Behaviour snapshot is invalid",
						}),
				),
			);

		const ReadMutationResult = (
			commit: Pick<ModelBehaviourCommitResult, "events" | "status">,
		) =>
			Effect.gen(function* () {
				return {
					events: commit.events,
					snapshot: yield* Snapshot(yield* repository.Read),
					status: commit.status,
				};
			});

		const Duplicate = (events: ReadonlyArray<ModelBehaviourEvent>) =>
			ReadMutationResult({ events, status: "duplicate" });

		const ReconcileRead = Effect.gen(function* () {
			const read = yield* repository.Read;
			const next_states = yield* Effect.forEach(providers.Providers, (provider) => {
				const setting = find_setting(read, provider.mapping.setting_id);

				if (setting._tag === "None") {
					return Effect.fail(
						new ModelBehaviourInvariantError({
							message: `Canonical setting ${provider.mapping.setting_id} is missing`,
						}),
					);
				}

				return ObserveProvider(
					provider,
					setting.value.value,
					find_provider_state(read, provider),
				);
			});
			const changed = next_states.filter((state) => {
				const previous = read.provider_states.find(
					(candidate) =>
						candidate.provider_id === state.provider_id &&
						candidate.setting_id === state.setting_id,
				);

				return state_changed(previous, state);
			});

			if (changed.length > 0) {
				const message_id = yield* metadata.MakeId("message");
				const sent_at = yield* metadata.Now;

				yield* repository.Commit({
					operation: operation(
						{ message_id, origin: "backend", sent_at },
						"model_behaviour.query.reconcile",
						changed,
					),
					provider_states: changed,
				});
			}

			return yield* Snapshot(yield* repository.Read);
		});

		const UpdateUnlocked = (input: ModelBehaviourUpdateRequest & ModelBehaviourMutationTrace) =>
			Effect.gen(function* () {
				const op = operation(input, "model_behaviour.update", {
					setting_id: input.setting_id,
					value: input.value,
				});
				const preflight = yield* repository.Preflight(op);

				if (preflight._tag === "Duplicate") {
					return yield* Duplicate(preflight.events);
				}

				const capable = providers.Providers.filter(
					(provider) =>
						provider.mapping.setting_id === input.setting_id && is_writeable(provider),
				);

				if (capable.length === 0) {
					return yield* new ModelBehaviourConflict({
						code: "no_supported_provider",
						message: "No installed provider supports this Model Behaviour control",
					});
				}

				const read = yield* repository.Read;
				const setting = find_setting(read, input.setting_id);

				if (setting._tag === "None") {
					return yield* new ModelBehaviourInvariantError({
						message: `Canonical setting ${input.setting_id} is missing`,
					});
				}

				const provider_states = yield* Effect.forEach(
					providers.Providers.filter(
						(provider) => provider.mapping.setting_id === input.setting_id,
					),
					(provider) =>
						ApplyProvider(
							provider,
							input.value,
							input.message_id,
							find_provider_state(read, provider),
						),
				);
				const committed = yield* repository.Commit({
					operation: op,
					provider_states,
					...(same_value(setting.value.value, input.value)
						? {}
						: {
								setting_update: {
									setting_id: input.setting_id,
									value: input.value,
								},
							}),
				});

				return yield* ReadMutationResult(committed);
			});

		const ResolveDriftUnlocked = (
			input: ModelBehaviourDriftResolutionRequest & ModelBehaviourMutationTrace,
		) =>
			Effect.gen(function* () {
				const payload: ModelBehaviourDriftResolutionRequest = {
					action: input.action,
					observed_hash: input.observed_hash,
					provider_id: input.provider_id,
					setting_id: input.setting_id,
				};
				const op = operation(input, "model_behaviour.drift.resolve", payload);
				const preflight = yield* repository.Preflight(op);

				if (preflight._tag === "Duplicate") {
					return yield* Duplicate(preflight.events);
				}

				const provider_option = providers.Find(input.provider_id, input.setting_id);

				if (provider_option._tag === "None") {
					return yield* new ModelBehaviourConflict({
						code: "provider_not_found",
						message: `Provider ${input.provider_id} does not own this control`,
					});
				}

				const provider = provider_option.value;

				if (!is_writeable(provider)) {
					return yield* new ModelBehaviourConflict({
						code: "provider_not_supported",
						message: `Provider ${input.provider_id} cannot reconcile this control`,
					});
				}

				const read = yield* repository.Read;
				const setting = find_setting(read, input.setting_id);

				if (setting._tag === "None") {
					return yield* new ModelBehaviourInvariantError({
						message: `Canonical setting ${input.setting_id} is missing`,
					});
				}

				const inspected = yield* InspectProvider(provider);

				if (inspected._tag === "Failed") {
					return yield* new ModelBehaviourConflict({
						code: "provider_unavailable",
						message: `Provider ${input.provider_id} could not be inspected`,
					});
				}

				const observation = inspected.observation;

				if (observation.observed_hash !== input.observed_hash) {
					return yield* new ModelBehaviourConflict({
						code: "stale_observation",
						message: `Provider ${input.provider_id} changed after drift was shown`,
					});
				}

				const previous = find_provider_state(read, provider);

				if (input.action === "ignore") {
					const committed = yield* repository.Commit({
						operation: op,
						provider_states: [
							{
								...(previous?.applied_hash === undefined
									? {}
									: { applied_hash: previous.applied_hash }),
								...(previous?.backup_path === undefined
									? {}
									: { backup_path: previous.backup_path }),
								ignored_drift_hash: observation.observed_hash,
								...(provider.mapping.native_key === undefined
									? {}
									: { native_key: provider.mapping.native_key }),
								observed_hash: observation.observed_hash,
								provider_id: provider.mapping.provider_id,
								setting_id: provider.mapping.setting_id,
								status: "drift_ignored",
								target_path: observation.target_path,
							},
						],
					});

					return yield* ReadMutationResult(committed);
				}

				const desired = input.action === "import" ? observation.value : setting.value.value;
				const source_result = yield* provider
					.Apply({
						...(observation.document_hash === undefined
							? {}
							: { expected_document_hash: observation.document_hash }),
						expected_observed_hash: observation.observed_hash,
						operation_id: input.message_id,
						value: desired,
					})
					.pipe(
						Effect.map((result) => ({ _tag: "Applied" as const, result })),
						Effect.catch((error) => Effect.succeed({ _tag: "Failed" as const, error })),
					);

				if (source_result._tag === "Applied" && source_result.result._tag === "Changed") {
					return yield* new ModelBehaviourConflict({
						code: "stale_observation",
						message: `Provider ${input.provider_id} changed during reconciliation`,
					});
				}

				if (input.action === "import" && source_result._tag === "Failed") {
					return yield* new ModelBehaviourConflict({
						code: "provider_unavailable",
						message: `Provider ${input.provider_id} could not be verified for import`,
					});
				}

				const source_state =
					source_result._tag === "Failed"
						? failed_state(provider, source_result.error, previous, observation)
						: synchronized_state(provider, desired, source_result.result, previous);
				const other_states =
					input.action === "import"
						? yield* Effect.forEach(
								providers.Providers.filter(
									(candidate) =>
										candidate !== provider &&
										candidate.mapping.setting_id === input.setting_id,
								),
								(candidate) =>
									ApplyProvider(
										candidate,
										desired,
										input.message_id,
										find_provider_state(read, candidate),
									),
							)
						: [];
				const committed = yield* repository.Commit({
					operation: op,
					provider_states: [source_state, ...other_states],
					...(input.action === "import" && !same_value(setting.value.value, desired)
						? {
								setting_update: {
									setting_id: input.setting_id,
									value: desired,
								},
							}
						: {}),
				});

				return yield* ReadMutationResult(committed);
			});

		const RetrySyncUnlocked = (
			input: ModelBehaviourRetryRequest & ModelBehaviourMutationTrace,
		) =>
			Effect.gen(function* () {
				const payload: ModelBehaviourRetryRequest = {
					provider_id: input.provider_id,
					setting_id: input.setting_id,
				};
				const op = operation(input, "model_behaviour.sync.retry", payload);
				const preflight = yield* repository.Preflight(op);

				if (preflight._tag === "Duplicate") {
					return yield* Duplicate(preflight.events);
				}

				const provider_option = providers.Find(input.provider_id, input.setting_id);

				if (provider_option._tag === "None") {
					return yield* new ModelBehaviourConflict({
						code: "provider_not_found",
						message: `Provider ${input.provider_id} does not own this control`,
					});
				}

				const provider = provider_option.value;

				if (!is_writeable(provider)) {
					return yield* new ModelBehaviourConflict({
						code: "provider_not_supported",
						message: `Provider ${input.provider_id} cannot reconcile this control`,
					});
				}

				const read = yield* repository.Read;
				const setting = find_setting(read, input.setting_id);

				if (setting._tag === "None") {
					return yield* new ModelBehaviourInvariantError({
						message: `Canonical setting ${input.setting_id} is missing`,
					});
				}

				const committed = yield* repository.Commit({
					operation: op,
					provider_states: [
						yield* ApplyProvider(
							provider,
							setting.value.value,
							input.message_id,
							find_provider_state(read, provider),
						),
					],
				});

				return yield* ReadMutationResult(committed);
			});

		return {
			Get: Semaphore.withPermit(lock)(ReconcileRead),
			ResolveDrift: (input) => Semaphore.withPermit(lock)(ResolveDriftUnlocked(input)),
			RetrySync: (input) => Semaphore.withPermit(lock)(RetrySyncUnlocked(input)),
			Update: (input) => Semaphore.withPermit(lock)(UpdateUnlocked(input)),
		};
	}),
);
