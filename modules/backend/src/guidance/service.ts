import { Deferred, Effect, Exit, Layer, Option, Ref, Scope, Semaphore } from "effect";

import type { EngineGlobalGuidance } from "@artisan/engines";
import type {
	GlobalGuidanceDriftResolutionRequest,
	GlobalGuidanceRetryRequest,
	GlobalGuidanceSelectionRequest,
	GlobalGuidanceSnapshot,
	GlobalGuidanceMetadata,
} from "@artisan/protocol";

import {
	GlobalGuidanceRepository,
	type GlobalGuidanceAcceptance,
	type GlobalGuidanceCommandInput,
} from "./repository";
import {
	guidance_hash,
	GuidanceProviderRegistry,
	type NativeGuidanceProviderAdapter,
} from "./provider-mirrors";
import { RuntimeMetadata } from "../runtime/metadata";
import {
	type CanonicalContent,
	ContentMetadata,
	MakeGuidanceCandidates,
	make_provider_expectations,
	type PresentProvider,
	provider_metadata,
	provider_mutation_fingerprint,
	type ProviderExpectations,
	type PreparedProviderMutation,
} from "./content";
import {
	GlobalGuidanceConflict,
	GlobalGuidanceInvariantError,
	GlobalGuidanceService,
	type GlobalGuidanceServiceError,
	type GlobalGuidanceMutationTrace,
	type GlobalGuidanceServiceOptions,
} from "./contracts";
import { GuidanceCanonical, make_guidance_canonical_layer } from "./canonical";
import {
	GuidanceProviderSync,
	type GuidanceProviderSyncOutcome,
	make_guidance_provider_sync_layer,
} from "./provider-sync";

export * from "./contracts";

interface GlobalGuidanceServiceLayerOptions extends GlobalGuidanceServiceOptions {
	/**
	 * Optional deterministic observation at the exact initialization-flight boundary.
	 * Production composition leaves this absent; lifecycle coverage uses it to hold an
	 * owner before it can touch provider, file, or persistence boundaries.
	 */
	readonly OnInitializeStart?: Effect.Effect<void, GlobalGuidanceServiceError>;
	/** Internal deterministic observation after a flight is admitted to this service scope. */
	readonly OnInitializationClaimed?: Effect.Effect<void>;
	/** Internal deterministic observation immediately before an exact flight is settled. */
	readonly OnInitializationPublishing?: Effect.Effect<void>;
	/** Internal deterministic observation after a caller saw failed startup and before retry admission. */
	readonly OnRetryObserved?: Effect.Effect<void>;
}

/** Creates the serialized global guidance workflow over explicit provider adapters. */
export function make_global_guidance_service_layer(options: GlobalGuidanceServiceLayerOptions) {
	return Layer.effect(
		GlobalGuidanceService,
		Effect.gen(function* () {
			const metadata = yield* RuntimeMetadata;
			const providers = yield* GuidanceProviderRegistry;
			const repository = yield* GlobalGuidanceRepository;
			const scope = yield* Scope.Scope;
			const lock = yield* Semaphore.make(1);
			const canonical_service = yield* GuidanceCanonical;
			const provider_sync = yield* GuidanceProviderSync;
			const ReadCanonical = canonical_service.Read;
			const PrepareProviderMutation = canonical_service.PrepareProviderMutation;
			const WriteCanonical = canonical_service.Write;
			const DiscoverAllNative = provider_sync.DiscoverAllNative;
			const RecordDiscoveryFailure = provider_sync.RecordDiscoveryFailure;
			const RecordProvider = provider_sync.RecordProvider;
			const SyncAll = provider_sync.SyncAll;
			const SyncNative = provider_sync.SyncNative;
			const MergeProviderOutcomes = (
				metadata: GlobalGuidanceMetadata,
				outcomes: ReadonlyArray<GuidanceProviderSyncOutcome>,
			) => {
				const providers = new Map(
					metadata.providers.map((provider) => [provider.provider, provider]),
				);
				for (const outcome of outcomes)
					providers.set(outcome.metadata.provider, outcome.metadata);

				return {
					...metadata,
					providers: [...providers.values()].sort((left, right) =>
						left.provider.localeCompare(right.provider),
					),
				} satisfies GlobalGuidanceMetadata;
			};
			const SnapshotFrom = (
				metadata: GlobalGuidanceMetadata,
				canonical: Option.Option<CanonicalContent>,
				candidates: GlobalGuidanceSnapshot["candidates"] = [],
			) =>
				Effect.succeed({
					candidates,
					content: Option.match(canonical, {
						onNone: () => "",
						onSome: (value) => value.content,
					}),
					metadata,
				} satisfies GlobalGuidanceSnapshot);

			const Snapshot = Effect.gen(function* () {
				const stored = yield* repository.Read;
				const canonical = yield* ReadCanonical;
				const candidates =
					stored.canonical.status === "selection_required"
						? yield* DiscoverAllNative.pipe(Effect.flatMap(MakeGuidanceCandidates))
						: [];

				return {
					candidates,
					content: Option.match(canonical, {
						onNone: () => "",
						onSome: (value) => value.content,
					}),
					metadata: stored,
				} satisfies GlobalGuidanceSnapshot;
			});

			const CommitCanonical = (
				canonical: CanonicalContent,
				command: GlobalGuidanceCommandInput,
				force_provider_sync: boolean,
				expectations?: ProviderExpectations,
			) =>
				Effect.gen(function* () {
					const existing = yield* repository.PreflightAccept(command);

					if (Option.isSome(existing))
						return { acceptance: existing.value, canonical, outcomes: [] };

					const written = yield* WriteCanonical(canonical, "canonical");
					const acceptance = yield* repository.Accept({
						...command,
						intent: {
							...command.intent,
							...(command.intent.type === "guidance.canonical.commit"
								? {
										byte_count: written.byte_count,
										content_hash: written.content_hash,
									}
								: {}),
						},
					});

					const outcomes = yield* SyncAll(
						written,
						command.message_id,
						force_provider_sync,
						expectations,
					);

					return { acceptance, canonical: written, outcomes };
				});

			const InitializeInternal = Effect.gen(function* () {
				yield* options.OnInitializeStart ?? Effect.void;
				const stored = yield* repository.Read;
				const canonical = yield* ReadCanonical;

				if (stored.canonical.status === "ready" && Option.isSome(canonical)) {
					if (stored.canonical.content_hash !== canonical.value.content_hash) {
						const now = yield* metadata.Now;
						const operation_id = `guidance_recovery_${canonical.value.content_hash.slice(0, 24)}`;

						const committed = yield* CommitCanonical(
							canonical.value,
							{
								intent: {
									byte_count: canonical.value.byte_count,
									content_hash: canonical.value.content_hash,
									reason: "recovery",
									type: "guidance.canonical.commit",
								},
								message_id: operation_id,
								origin: "backend",
								sent_at: now,
							},
							false,
						);
						return yield* SnapshotFrom(
							MergeProviderOutcomes(
								{
									...stored,
									canonical: {
										...stored.canonical,
										byte_count: committed.canonical.byte_count,
										content_hash: committed.canonical.content_hash,
										status: "ready",
										updated_at: committed.acceptance.event.sent_at,
									},
								},
								committed.outcomes,
							),
							Option.some(committed.canonical),
						);
					} else {
						const outcomes = yield* SyncAll(canonical.value, "guidance_refresh", false);
						return yield* SnapshotFrom(
							MergeProviderOutcomes(stored, outcomes),
							canonical,
						);
					}
				}

				const discoveries = yield* DiscoverAllNative;

				if (stored.canonical.status === "ready") {
					const recovery_source = discoveries.find(
						(item): item is PresentProvider =>
							item.discovery._tag === "Present" &&
							item.discovery.hash === stored.canonical.content_hash,
					);

					if (!recovery_source) {
						return yield* new GlobalGuidanceInvariantError({
							operation: "canonical_file_missing",
						});
					}

					const recovered = yield* ContentMetadata(recovery_source.discovery.content);
					const written = yield* WriteCanonical(recovered, "canonical-recovery");

					const outcomes = yield* SyncAll(written, "guidance_recovery", false);

					return yield* SnapshotFrom(
						MergeProviderOutcomes(stored, outcomes),
						Option.some(written),
					);
				}

				const candidates = yield* MakeGuidanceCandidates(discoveries);
				const failures = discoveries.filter((item) => item.discovery._tag === "ReadFailed");
				const present_by_hash = Map.groupBy(
					candidates,
					(candidate) => candidate.content_hash,
				);

				const failure_outcomes: Array<GuidanceProviderSyncOutcome> = [];
				for (const failure of failures) {
					if (failure.discovery._tag === "ReadFailed") {
						failure_outcomes.push(
							yield* RecordDiscoveryFailure(
								"guidance_initialization",
								failure.adapter,
								failure.discovery,
							),
						);
					}
				}

				if (failures.length > 0 || present_by_hash.size > 1) {
					if (candidates.length > 0) {
						const now = yield* metadata.Now;
						const hashes = [...present_by_hash.keys()].sort();
						const operation_id = `guidance_selection_${guidance_hash(hashes.join("\n")).slice(0, 24)}`;

						const acceptance = yield* repository.Accept({
							intent: {
								candidate_hashes: hashes as [string, ...Array<string>],
								type: "guidance.selection.require",
							},
							message_id: operation_id,
							origin: "backend",
							sent_at: now,
						});

						const selection_outcomes = yield* Effect.forEach(candidates, (candidate) =>
							RecordProvider("guidance_selection", {
								modified_at: candidate.modified_at,
								observed_byte_count: candidate.byte_count,
								observed_hash: candidate.content_hash,
								path: candidate.path,
								provider: candidate.provider,
								status: "awaiting_selection",
							}),
						);
						return yield* SnapshotFrom(
							MergeProviderOutcomes(
								{
									...stored,
									canonical: {
										status: "selection_required",
										updated_at: acceptance.event.sent_at,
									},
								},
								[...failure_outcomes, ...selection_outcomes],
							),
							canonical,
							candidates,
						);
					}

					return yield* SnapshotFrom(
						MergeProviderOutcomes(stored, failure_outcomes),
						canonical,
						candidates,
					);
				}

				const selected = candidates[0];
				const initial = yield* ContentMetadata(selected?.preview ?? "");
				const now = yield* metadata.Now;
				const operation_id = `guidance_initial_${initial.content_hash.slice(0, 24)}`;

				const committed = yield* CommitCanonical(
					initial,
					{
						intent: {
							byte_count: initial.byte_count,
							content_hash: initial.content_hash,
							reason: "first_run",
							...(selected === undefined
								? {}
								: { selected_provider: selected.provider }),
							type: "guidance.canonical.commit",
						},
						message_id: operation_id,
						origin: "backend",
						sent_at: now,
					},
					true,
					make_provider_expectations(discoveries),
				);

				return yield* SnapshotFrom(
					MergeProviderOutcomes(
						{
							...stored,
							canonical: {
								byte_count: committed.canonical.byte_count,
								content_hash: committed.canonical.content_hash,
								...(selected === undefined
									? {}
									: { selected_provider: selected.provider }),
								status: "ready",
								updated_at: committed.acceptance.event.sent_at,
							},
						},
						committed.outcomes,
					),
					Option.some(committed.canonical),
				);
			});

			type GuidanceFlight = {
				readonly phase: "refresh" | "startup";
				readonly result: Deferred.Deferred<
					Exit.Exit<GlobalGuidanceSnapshot, GlobalGuidanceServiceError>
				>;
			};
			type StartupState =
				| { readonly _tag: "Failed"; readonly flight: GuidanceFlight }
				| { readonly _tag: "Ready"; readonly flight: GuidanceFlight }
				| { readonly _tag: "Running"; readonly flight: GuidanceFlight };
			type LifecycleState =
				| { readonly _tag: "Closed" }
				| {
						readonly _tag: "Open";
						readonly refresh: Option.Option<GuidanceFlight>;
						readonly startup: StartupState;
				  };
			type FlightAdmission =
				| { readonly _tag: "Claimed"; readonly flight: GuidanceFlight }
				| { readonly _tag: "Closed" }
				| { readonly _tag: "Existing"; readonly flight: GuidanceFlight }
				| { readonly _tag: "Refresh" };

			const initial =
				yield* Deferred.make<
					Exit.Exit<GlobalGuidanceSnapshot, GlobalGuidanceServiceError>
				>();
			const initial_flight: GuidanceFlight = { phase: "startup", result: initial };
			const lifecycle = yield* Ref.make<LifecycleState>({
				_tag: "Open",
				refresh: Option.none(),
				startup: { _tag: "Running", flight: initial_flight },
			});
			const Owns = (flight: GuidanceFlight) =>
				Ref.get(lifecycle).pipe(
					Effect.map(
						(current) =>
							current._tag === "Open" &&
							(current.startup._tag === "Running" && current.startup.flight === flight
								? true
								: Option.isSome(current.refresh) &&
									current.refresh.value === flight),
					),
				);
			/** Deferred publication and exact state release are one non-interruptible ownership transition. */
			const Complete = (
				flight: GuidanceFlight,
				exit: Exit.Exit<GlobalGuidanceSnapshot, GlobalGuidanceServiceError>,
			) =>
				(options.OnInitializationPublishing ?? Effect.void).pipe(
					Effect.andThen(
						Effect.gen(function* () {
							yield* Deferred.succeed(flight.result, exit);
							yield* Ref.update(lifecycle, (current) => {
								if (current._tag === "Closed") return current;
								if (
									current.startup._tag === "Running" &&
									current.startup.flight === flight
								) {
									return {
										...current,
										startup: Exit.isSuccess(exit)
											? ({ _tag: "Ready", flight } satisfies StartupState)
											: ({ _tag: "Failed", flight } satisfies StartupState),
									};
								}
								return Option.isSome(current.refresh) &&
									current.refresh.value === flight
									? { ...current, refresh: Option.none() }
									: current;
							});
						}).pipe(Effect.uninterruptible),
					),
				);
			const Drive = (flight: GuidanceFlight) =>
				Effect.gen(function* () {
					if (!(yield* Owns(flight))) return;
					const exit = yield* (options.OnInitializationClaimed ?? Effect.void).pipe(
						Effect.andThen(Semaphore.withPermit(lock)(InitializeInternal)),
						Effect.exit,
					);
					yield* Complete(flight, exit);
				});
			const StartOwner = (flight: GuidanceFlight) =>
				Effect.uninterruptibleMask(() =>
					Owns(flight).pipe(
						Effect.flatMap((owned) =>
							owned
								? Effect.forkIn(Effect.interruptible(Drive(flight)), scope, {
										startImmediately: true,
									}).pipe(Effect.asVoid)
								: Effect.void,
						),
					),
				);
			const Close = Effect.uninterruptible(
				Effect.gen(function* () {
					const flights = yield* Ref.modify(lifecycle, (current) => {
						if (current._tag === "Closed") return [[], current] as const;
						return [
							[
								...(current.startup._tag === "Running"
									? [current.startup.flight]
									: []),
								...(Option.isSome(current.refresh) ? [current.refresh.value] : []),
							],
							{ _tag: "Closed" } satisfies LifecycleState,
						] as const;
					});
					const interrupted = yield* Effect.exit(Effect.interrupt);
					for (const flight of flights)
						yield* Deferred.succeed(flight.result, interrupted);
				}),
			);
			yield* Effect.addFinalizer(() => Close);

			const AwaitFlight = (flight: GuidanceFlight) =>
				Deferred.await(flight.result).pipe(
					Effect.flatMap((result) =>
						Exit.isSuccess(result)
							? Effect.succeed(result.value)
							: Effect.failCause(result.cause),
					),
				);
			const Claim = (
				phase: GuidanceFlight["phase"],
				retry_observation?: GuidanceFlight,
			): Effect.Effect<GlobalGuidanceSnapshot, GlobalGuidanceServiceError> =>
				Effect.uninterruptibleMask((restore) =>
					Effect.gen(function* () {
						const candidate: GuidanceFlight = {
							phase,
							result: yield* Deferred.make<
								Exit.Exit<GlobalGuidanceSnapshot, GlobalGuidanceServiceError>
							>(),
						};
						const admission = yield* Ref.modify<LifecycleState, FlightAdmission>(
							lifecycle,
							(current) => {
								if (current._tag === "Closed")
									return [{ _tag: "Closed" }, current] as const;
								if (phase === "startup") {
									if (current.startup._tag === "Running")
										return [
											{ _tag: "Existing", flight: current.startup.flight },
											current,
										] as const;
									if (current.startup._tag === "Ready")
										if (retry_observation !== undefined)
											return [
												{
													_tag: "Existing",
													flight: current.startup.flight,
												},
												current,
											] as const;
									if (current.startup._tag === "Ready")
										return [{ _tag: "Refresh" }, current] as const;
									if (
										retry_observation !== undefined &&
										current.startup.flight !== retry_observation
									)
										return [
											{ _tag: "Existing", flight: current.startup.flight },
											current,
										] as const;
									return [
										{ _tag: "Claimed", flight: candidate },
										{
											...current,
											startup: { _tag: "Running", flight: candidate },
										},
									] as const;
								}
								if (current.startup._tag !== "Ready")
									return [{ _tag: "Closed" }, current] as const;
								if (Option.isSome(current.refresh))
									return [
										{ _tag: "Existing", flight: current.refresh.value },
										current,
									] as const;
								return [
									{ _tag: "Claimed", flight: candidate },
									{ ...current, refresh: Option.some(candidate) },
								] as const;
							},
						);
						if (admission._tag === "Closed") return yield* restore(Effect.interrupt);
						if (admission._tag === "Refresh") return yield* restore(Claim("refresh"));
						if (admission._tag === "Claimed") yield* StartOwner(admission.flight);
						return yield* restore(AwaitFlight(admission.flight));
					}),
				);
			const AwaitStartup = Effect.gen(function* () {
				const current = yield* Ref.get(lifecycle);
				if (current._tag === "Closed") return yield* Effect.interrupt;
				if (current.startup._tag === "Ready") return;
				yield* AwaitFlight(current.startup.flight);
			});
			const AfterInitialization = <A, E, R>(effect: Effect.Effect<A, E, R>) =>
				AwaitStartup.pipe(Effect.andThen(effect));

			yield* StartOwner(initial_flight);

			const Get = Effect.gen(function* () {
				const current = yield* Ref.get(lifecycle);
				if (current._tag === "Closed") return yield* Effect.interrupt;
				return current.startup._tag === "Ready"
					? yield* Claim("refresh")
					: yield* AwaitFlight(current.startup.flight);
			});
			const Initialize = Effect.gen(function* () {
				const current = yield* Ref.get(lifecycle);
				if (current._tag === "Closed") return yield* Effect.interrupt;
				if (current.startup._tag === "Failed") {
					yield* options.OnRetryObserved ?? Effect.void;
					return yield* Claim("startup", current.startup.flight);
				}
				return current.startup._tag === "Ready"
					? yield* Claim("refresh")
					: yield* AwaitFlight(current.startup.flight);
			});
			const Update = (input: GlobalGuidanceMutationTrace & { readonly content: string }) =>
				AfterInitialization(
					Semaphore.withPermit(lock)(
						Effect.gen(function* () {
							const canonical = yield* ContentMetadata(input.content);
							const { acceptance } = yield* CommitCanonical(
								canonical,
								{
									intent: {
										byte_count: canonical.byte_count,
										content_hash: canonical.content_hash,
										reason: "user_update",
										type: "guidance.canonical.commit",
									},
									message_id: input.message_id,
									origin: input.origin,
									sent_at: input.sent_at,
								},
								false,
							);

							return { acceptance, snapshot: yield* Snapshot };
						}),
					),
				);
			const Select = (input: GlobalGuidanceSelectionRequest & GlobalGuidanceMutationTrace) =>
				AfterInitialization(
					Semaphore.withPermit(lock)(
						Effect.gen(function* () {
							const request_fingerprint = provider_mutation_fingerprint({
								content_hash: input.content_hash,
								provider: input.provider,
								type: "guidance.selection",
							});
							const duplicate = yield* repository.PreflightRequest({
								message_id: input.message_id,
								origin: input.origin,
								request_fingerprint,
							});

							if (Option.isSome(duplicate)) {
								return { acceptance: duplicate.value, snapshot: yield* Snapshot };
							}

							const stored = yield* repository.Read;

							if (stored.canonical.status !== "selection_required") {
								return yield* new GlobalGuidanceConflict({
									provider: input.provider,
									reason: "selection_not_required",
								});
							}

							const recorded_candidate = provider_metadata(
								stored.providers,
								input.provider,
							);

							if (
								recorded_candidate?.status !== "awaiting_selection" ||
								recorded_candidate.observed_hash !== input.content_hash
							) {
								return yield* new GlobalGuidanceConflict({
									provider: input.provider,
									reason: "candidate_changed",
								});
							}

							const current_discoveries = yield* DiscoverAllNative;
							const unavailable_candidate = stored.providers.find(
								(candidate) =>
									candidate.status === "awaiting_selection" &&
									!current_discoveries.some(
										(current) =>
											current.adapter.provider === candidate.provider,
									),
							);

							if (unavailable_candidate) {
								return yield* new GlobalGuidanceConflict({
									provider: unavailable_candidate.provider,
									reason: "provider_unavailable",
								});
							}

							const changed_candidate = current_discoveries.find(
								({ adapter, discovery }) => {
									const recorded = provider_metadata(
										stored.providers,
										adapter.provider,
									);

									return recorded?.status === "awaiting_selection"
										? discovery._tag !== "Present" ||
												discovery.hash !== recorded.observed_hash
										: discovery._tag !== "Absent";
								},
							);

							if (changed_candidate) {
								return yield* new GlobalGuidanceConflict({
									provider: changed_candidate.adapter.provider,
									reason: "candidate_changed",
								});
							}

							const selected = current_discoveries.find(
								({ adapter }) => adapter.provider === input.provider,
							);

							if (selected?.discovery._tag !== "Present") {
								return yield* new GlobalGuidanceConflict({
									provider: input.provider,
									reason: "provider_unavailable",
								});
							}

							const canonical = yield* ContentMetadata(selected.discovery.content);
							const { acceptance } = yield* CommitCanonical(
								canonical,
								{
									intent: {
										byte_count: canonical.byte_count,
										content_hash: canonical.content_hash,
										reason: "selection",
										selected_provider: input.provider,
										type: "guidance.canonical.commit",
									},
									message_id: input.message_id,
									origin: input.origin,
									request_fingerprint,
									sent_at: input.sent_at,
								},
								true,
								make_provider_expectations(current_discoveries),
							);

							return { acceptance, snapshot: yield* Snapshot };
						}),
					),
				);
			const ResolveDrift = (
				input: GlobalGuidanceDriftResolutionRequest & GlobalGuidanceMutationTrace,
			) =>
				AfterInitialization(
					Semaphore.withPermit(lock)(
						Effect.gen(function* () {
							const import_request_fingerprint =
								input.action === "import"
									? provider_mutation_fingerprint({
											action: input.action,
											observed_hash: input.observed_hash,
											provider: input.provider,
											type: "guidance.drift.resolve",
										})
									: undefined;

							if (import_request_fingerprint !== undefined) {
								const duplicate = yield* repository.PreflightRequest({
									message_id: input.message_id,
									origin: input.origin,
									request_fingerprint: import_request_fingerprint,
								});

								if (Option.isSome(duplicate)) {
									return {
										acceptance: duplicate.value,
										snapshot: yield* Snapshot,
									};
								}
							}

							const adapter = providers.Providers.find(
								(candidate): candidate is NativeGuidanceProviderAdapter =>
									candidate.provider === input.provider &&
									candidate.mode === "native_file",
							);

							if (!adapter) {
								return yield* new GlobalGuidanceConflict({
									provider: input.provider,
									reason: "provider_unavailable",
								});
							}

							const mutation =
								input.action === "import"
									? Option.none<PreparedProviderMutation>()
									: Option.some(
											yield* PrepareProviderMutation(
												input.message_id,
												(canonical_hash) => ({
													action: input.action,
													canonical_hash,
													observed_hash: input.observed_hash,
													provider: input.provider,
													type: "guidance.drift.resolve",
												}),
											),
										);

							if (
								Option.isSome(mutation) &&
								Option.isSome(mutation.value.acceptance)
							) {
								return {
									acceptance: mutation.value.acceptance.value,
									snapshot: yield* Snapshot,
								};
							}

							const recorded_provider = provider_metadata(
								(yield* repository.Read).providers,
								input.provider,
							);

							if (
								recorded_provider?.status !== "drift_detected" ||
								recorded_provider.observed_hash !== input.observed_hash
							) {
								return yield* new GlobalGuidanceConflict({
									provider: input.provider,
									reason: "drift_changed",
								});
							}

							const discovery = yield* adapter.Discover;

							if (
								discovery._tag !== "Present" ||
								discovery.hash !== input.observed_hash
							) {
								return yield* new GlobalGuidanceConflict({
									provider: input.provider,
									reason: "drift_changed",
								});
							}

							const observed = yield* ContentMetadata(discovery.content);

							if (input.action === "import") {
								if (import_request_fingerprint === undefined)
									return yield* new GlobalGuidanceConflict({
										provider: input.provider,
										reason: "drift_changed",
									});
								const request_fingerprint = import_request_fingerprint;
								const { acceptance } = yield* CommitCanonical(
									observed,
									{
										intent: {
											byte_count: observed.byte_count,
											content_hash: observed.content_hash,
											reason: "drift_import",
											selected_provider: input.provider,
											type: "guidance.canonical.commit",
										},
										message_id: input.message_id,
										origin: input.origin,
										request_fingerprint,
										sent_at: input.sent_at,
									},
									false,
								);

								return { acceptance, snapshot: yield* Snapshot };
							}

							if (input.action === "overwrite") {
								const prepared = Option.getOrThrow(mutation);

								const { acceptance } = yield* SyncNative(
									adapter,
									prepared.canonical,
									input.message_id,
									true,
									true,
									prepared.request_fingerprint,
									input.observed_hash,
								);

								return { acceptance, snapshot: yield* Snapshot };
							}

							const prepared = Option.getOrThrow(mutation);
							const acceptance = yield* repository.RecordProviderReconciliation({
								ignored_drift_hash: observed.content_hash,
								modified_at: discovery.modified_at,
								observed_byte_count: observed.byte_count,
								observed_hash: observed.content_hash,
								operation_id: input.message_id,
								path: discovery.path,
								provider: input.provider,
								request_fingerprint: prepared.request_fingerprint,
								status: "drift_detected",
							});

							return { acceptance, snapshot: yield* Snapshot };
						}),
					),
				);
			const RetrySync = (input: GlobalGuidanceRetryRequest & GlobalGuidanceMutationTrace) =>
				AfterInitialization(
					Semaphore.withPermit(lock)(
						Effect.gen(function* () {
							const request_fingerprint = provider_mutation_fingerprint({
								provider: input.provider,
								type: "guidance.sync.retry",
							});
							const duplicate = yield* repository.PreflightProviderMutation({
								operation_id: input.message_id,
								request_fingerprint,
							});

							if (Option.isSome(duplicate)) {
								return { acceptance: duplicate.value, snapshot: yield* Snapshot };
							}

							const adapter = providers.Providers.find(
								(candidate) => candidate.provider === input.provider,
							);

							if (!adapter) {
								return yield* new GlobalGuidanceConflict({
									provider: input.provider,
									reason: "provider_unavailable",
								});
							}

							const canonical = yield* ReadCanonical;

							if (Option.isNone(canonical)) {
								return yield* new GlobalGuidanceInvariantError({
									operation: "canonical_file_missing",
								});
							}

							let acceptance: GlobalGuidanceAcceptance;

							if (adapter.mode === "native_file") {
								acceptance = (yield* SyncNative(
									adapter,
									canonical.value,
									input.message_id,
									false,
									true,
									request_fingerprint,
								)).acceptance;
							} else {
								acceptance = yield* repository.RecordProviderReconciliation({
									operation_id: input.message_id,
									provider: adapter.provider,
									request_fingerprint,
									status:
										adapter.mode === "runtime"
											? "applied_at_run_time"
											: "unsupported",
								});
							}

							return { acceptance, snapshot: yield* Snapshot };
						}),
					),
				);
			const ResolveForEngine = (engine_id: string) =>
				AfterInitialization(
					Semaphore.withPermit(lock)(
						Effect.gen(function* () {
							const adapter = providers.Providers.find(
								(candidate) => candidate.provider === engine_id,
							);

							if (adapter?.mode !== "runtime") {
								return Option.none<EngineGlobalGuidance>();
							}

							const canonical = yield* ReadCanonical;

							if (Option.isNone(canonical)) {
								return yield* new GlobalGuidanceInvariantError({
									operation: "canonical_file_missing",
								});
							}

							return Option.some({
								content: canonical.value.content,
								source_file: options.canonical_path,
							});
						}),
					),
				);

			return {
				Get,
				Initialize,
				ResolveDrift,
				ResolveForEngine,
				RetrySync,
				Select,
				Update,
			};
		}),
	).pipe(
		Layer.provide(make_guidance_provider_sync_layer(options)),
		Layer.provide(make_guidance_canonical_layer(options)),
	);
}
