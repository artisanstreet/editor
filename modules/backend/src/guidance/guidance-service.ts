import { basename, extname } from "node:path";

import { Context, Data, Effect, Layer, Option, Semaphore } from "effect";

import type { EngineGlobalGuidance } from "@artisan/engines";
import { global_guidance_maximum_bytes } from "@artisan/protocol";
import { SnowflakeId } from "@artisan/protocol";
import type {
	GlobalGuidanceCandidate,
	GlobalGuidanceDriftResolutionRequest,
	GlobalGuidanceProvider,
	GlobalGuidanceProviderMetadata,
	GlobalGuidanceRetryRequest,
	GlobalGuidanceSelectionRequest,
	GlobalGuidanceSnapshot,
} from "@artisan/protocol";

import {
	GlobalGuidanceRepository,
	type GlobalGuidanceAcceptance,
	type GlobalGuidanceCommandInput,
} from "./guidance-repository";
import {
	guidance_hash,
	normalize_guidance_content,
	GuidanceProviderRegistry,
	type GuidanceDiscovery,
	type NativeGuidanceProviderAdapter,
} from "./provider-mirrors";
import { GuidanceFileStore, GuidanceFileStoreFailure } from "./file-store";
import type { JournalStoreError } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

/** Configures the one canonical file and its recoverable backup directory. */
export interface GlobalGuidanceServiceOptions {
	readonly backups_directory: string;
	readonly canonical_path: string;
}

/** Supplies trace identity for a durable user-initiated guidance operation. */
export interface GlobalGuidanceMutationTrace {
	readonly message_id: string;
	readonly origin: "frontend";
	readonly sent_at: string;
}

/** Returns the durable operation result together with the refreshed projection. */
export interface GlobalGuidanceMutationResult {
	readonly acceptance: GlobalGuidanceAcceptance;
	readonly snapshot: GlobalGuidanceSnapshot;
}

/** Rejects a stale selection or drift action after provider files changed. */
export class GlobalGuidanceConflict extends Data.TaggedError("GlobalGuidanceConflict")<{
	readonly provider: GlobalGuidanceProvider;
	readonly reason:
		| "candidate_changed"
		| "drift_changed"
		| "provider_unavailable"
		| "selection_not_required";
}> {}

/** Reports a canonical-file invariant or a failed post-write verification. */
export class GlobalGuidanceInvariantError extends Data.TaggedError("GlobalGuidanceInvariantError")<{
	readonly operation: string;
}> {}

export type GlobalGuidanceServiceError =
	| GlobalGuidanceConflict
	| GlobalGuidanceInvariantError
	| GuidanceFileStoreFailure
	| JournalStoreError;

interface CanonicalContent {
	readonly byte_count: number;
	readonly content: string;
	readonly content_hash: string;
}

interface PresentProvider {
	readonly adapter: NativeGuidanceProviderAdapter;
	readonly discovery: Extract<GuidanceDiscovery, { readonly _tag: "Present" }>;
}

type ExpectedProviderState =
	| { readonly _tag: "Absent" }
	| { readonly _tag: "Present"; readonly hash: string };

type ProviderExpectations = ReadonlyMap<GlobalGuidanceProvider, ExpectedProviderState>;

interface PreparedProviderMutation {
	readonly acceptance: Option.Option<GlobalGuidanceAcceptance>;
	readonly canonical: CanonicalContent;
	readonly request_fingerprint: string;
}

/** Owns first-run import, canonical writes, provider sync, drift, and runtime handoff. */
export class GlobalGuidanceService extends Context.Service<
	GlobalGuidanceService,
	{
		readonly Get: Effect.Effect<GlobalGuidanceSnapshot, GlobalGuidanceServiceError>;
		readonly Initialize: Effect.Effect<GlobalGuidanceSnapshot, GlobalGuidanceServiceError>;
		readonly ResolveDrift: (
			input: GlobalGuidanceDriftResolutionRequest & GlobalGuidanceMutationTrace,
		) => Effect.Effect<GlobalGuidanceMutationResult, GlobalGuidanceServiceError>;
		readonly ResolveForEngine: (
			engine_id: string,
		) => Effect.Effect<Option.Option<EngineGlobalGuidance>, GlobalGuidanceServiceError>;
		readonly RetrySync: (
			input: GlobalGuidanceRetryRequest & GlobalGuidanceMutationTrace,
		) => Effect.Effect<GlobalGuidanceMutationResult, GlobalGuidanceServiceError>;
		readonly Select: (
			input: GlobalGuidanceSelectionRequest & GlobalGuidanceMutationTrace,
		) => Effect.Effect<GlobalGuidanceMutationResult, GlobalGuidanceServiceError>;
		readonly Update: (
			input: GlobalGuidanceMutationTrace & { readonly content: string },
		) => Effect.Effect<GlobalGuidanceMutationResult, GlobalGuidanceServiceError>;
	}
>()("Artisan/GlobalGuidanceService") {}

function content_metadata(content: string) {
	return Effect.gen(function* () {
		const normalized = normalize_guidance_content(content);
		const byte_count = new TextEncoder().encode(normalized).byteLength;

		if (byte_count > global_guidance_maximum_bytes) {
			return yield* new GlobalGuidanceInvariantError({
				operation: "guidance_too_large",
			});
		}

		return {
			byte_count,
			content: normalized,
			content_hash: guidance_hash(normalized),
		} satisfies CanonicalContent;
	});
}

function provider_metadata(
	providers: ReadonlyArray<GlobalGuidanceProviderMetadata>,
	provider: GlobalGuidanceProvider,
) {
	return providers.find((entry) => entry.provider === provider);
}

function reconciliation_operation_id(
	base: string,
	provider: GlobalGuidanceProvider,
	input: Readonly<Record<string, unknown>>,
) {
	const fingerprint = guidance_hash(JSON.stringify(input)).slice(0, 24);

	return `${base}_${provider}_${fingerprint}`;
}

function provider_mutation_fingerprint(input: Readonly<Record<string, unknown>>) {
	return guidance_hash(JSON.stringify(input));
}

function make_provider_expectations(
	discoveries: ReadonlyArray<{
		readonly adapter: NativeGuidanceProviderAdapter;
		readonly discovery: GuidanceDiscovery;
	}>,
) {
	return new Map(
		discoveries.flatMap(({ adapter, discovery }) =>
			discovery._tag === "ReadFailed"
				? []
				: [
						[
							adapter.provider,
							discovery._tag === "Present"
								? { _tag: "Present" as const, hash: discovery.hash }
								: { _tag: "Absent" as const },
						] as const,
					],
		),
	) satisfies Map<GlobalGuidanceProvider, ExpectedProviderState>;
}

function provider_state_matches(
	discovery: Exclude<GuidanceDiscovery, { readonly _tag: "ReadFailed" }>,
	expected: ExpectedProviderState,
) {
	return expected._tag === "Absent"
		? discovery._tag === "Absent"
		: discovery._tag === "Present" && discovery.hash === expected.hash;
}

function backup_name(label: string, path: string, id: string) {
	const extension = extname(path);
	const stem = basename(path, extension).replace(/[^a-zA-Z0-9._-]/g, "_");

	return `${label}-${stem}-${id}${extension || ".md"}`;
}

function guidance_file_error_code(error: GuidanceFileStoreFailure) {
	const code = (error.cause as NodeJS.ErrnoException).code;
	const is_access_error = code === "EACCES" || code === "EBUSY" || code === "EPERM";

	if (error.operation === "restore") {
		return is_access_error ? "guidance_restore_access_denied" : "guidance_restore_failed";
	}

	if (is_access_error) {
		return "guidance_access_denied";
	}

	if (code === "EXDEV") {
		return "guidance_cross_device";
	}

	if (code === "EMLINK" || code === "ENOTSUP") {
		return "guidance_link_unsupported";
	}

	return `guidance_${error.operation}_failed`;
}

/** Creates the serialized global guidance workflow over explicit provider adapters. */
export function make_global_guidance_service_layer(options: GlobalGuidanceServiceOptions) {
	return Layer.effect(
		GlobalGuidanceService,
		Effect.gen(function* () {
			const files = yield* GuidanceFileStore;
			const metadata = yield* RuntimeMetadata;
			const providers = yield* GuidanceProviderRegistry;
			const repository = yield* GlobalGuidanceRepository;
			const lock = yield* Semaphore.make(1);
			const snowflake_id = yield* SnowflakeId;

			const ReadCanonical = files.Read(options.canonical_path).pipe(
				Effect.flatMap(
					Option.match({
						onNone: () => Effect.succeed(Option.none<CanonicalContent>()),
						onSome: (file) =>
							content_metadata(file.content).pipe(Effect.map(Option.some)),
					}),
				),
			);

			const PrepareProviderMutation = (
				operation_id: string,
				make_request: (canonical_hash: string) => Readonly<Record<string, unknown>>,
			) =>
				Effect.gen(function* () {
					const canonical = yield* ReadCanonical;

					if (Option.isNone(canonical)) {
						return yield* new GlobalGuidanceInvariantError({
							operation: "canonical_file_missing",
						});
					}

					const request_fingerprint = provider_mutation_fingerprint(
						make_request(canonical.value.content_hash),
					);
					const acceptance = yield* repository.PreflightProviderMutation({
						operation_id,
						request_fingerprint,
					});

					return {
						acceptance,
						canonical: canonical.value,
						request_fingerprint,
					} satisfies PreparedProviderMutation;
				});

			const VerifyFile = (path: string, expected: CanonicalContent) =>
				files.Read(path).pipe(
					Effect.flatMap(
						Option.match({
							onNone: () =>
								Effect.fail(
									new GlobalGuidanceInvariantError({
										operation: "post_write_file_missing",
									}),
								),
							onSome: (file) =>
								content_metadata(file.content).pipe(
									Effect.flatMap((actual) =>
										actual.content_hash === expected.content_hash
											? Effect.succeed(actual)
											: Effect.fail(
													new GlobalGuidanceInvariantError({
														operation: "post_write_hash_mismatch",
													}),
												),
									),
								),
						}),
					),
				);

			const WriteCanonical = (next: CanonicalContent, label: string) =>
				Effect.gen(function* () {
					const current = yield* files.Read(options.canonical_path);
					const observed = yield* Option.match(current, {
						onNone: () => Effect.succeed(Option.none<CanonicalContent>()),
						onSome: (file) =>
							content_metadata(file.content).pipe(Effect.map(Option.some)),
					});

					if (
						Option.isSome(observed) &&
						observed.value.content_hash === next.content_hash
					) {
						return yield* VerifyFile(options.canonical_path, next);
					}

					const replacement = yield* files.ReplaceAtomic({
						backup_name: backup_name(
							label,
							options.canonical_path,
							yield* snowflake_id.Make("backup"),
						),
						backups_directory: options.backups_directory,
						content: next.content,
						...(Option.isSome(observed)
							? { expected_hash: observed.value.content_hash }
							: {}),
						path: options.canonical_path,
					});

					if (replacement._tag === "Changed") {
						return yield* new GlobalGuidanceInvariantError({
							operation: "canonical_changed_during_write",
						});
					}

					return yield* VerifyFile(options.canonical_path, next);
				});

			const RecordProvider = (
				base: string,
				input: Omit<
					Parameters<typeof repository.RecordProviderReconciliation>[0],
					"operation_id"
				>,
				exact_operation_id = false,
			) =>
				repository.RecordProviderReconciliation({
					...input,
					operation_id: exact_operation_id
						? base
						: reconciliation_operation_id(base, input.provider, input),
				});

			const RecordDiscoveryFailure = (
				base: string,
				adapter: NativeGuidanceProviderAdapter,
				discovery: Extract<GuidanceDiscovery, { readonly _tag: "ReadFailed" }>,
				exact_operation_id = false,
				request_fingerprint?: string,
			) =>
				RecordProvider(
					base,
					{
						last_error_code: "guidance_read_failed",
						path: discovery.path,
						provider: adapter.provider,
						...(request_fingerprint === undefined ? {} : { request_fingerprint }),
						status: "sync_failed",
					},
					exact_operation_id,
				);

			const DiscoverNative = (adapter: NativeGuidanceProviderAdapter) =>
				adapter.Discover.pipe(Effect.map((discovery) => ({ adapter, discovery })));

			const DiscoverAllNative = Effect.forEach(
				providers.Providers.filter(
					(adapter): adapter is NativeGuidanceProviderAdapter =>
						adapter.mode === "native_file",
				),
				DiscoverNative,
				{ concurrency: "unbounded" },
			);

			const RecordNonNative = (base: string) =>
				Effect.forEach(
					providers.Providers.filter((adapter) => adapter.mode !== "native_file"),
					(adapter) =>
						RecordProvider(base, {
							provider: adapter.provider,
							status:
								adapter.mode === "runtime" ? "applied_at_run_time" : "unsupported",
						}),
					{ concurrency: "unbounded", discard: true },
				);

			const SyncNative = (
				adapter: NativeGuidanceProviderAdapter,
				canonical: CanonicalContent,
				base: string,
				force: boolean,
				exact_operation_id = false,
				request_fingerprint?: string,
				expected_observed_hash?: string,
				expected_state?: ExpectedProviderState,
			) => {
				let backup = Option.none<string>();
				let path: string | undefined;

				return Effect.gen(function* () {
					const current_metadata = provider_metadata(
						(yield* repository.Read).providers,
						adapter.provider,
					);
					const discovery = yield* adapter.Discover;
					path = discovery.path;

					if (discovery._tag === "ReadFailed") {
						return yield* RecordDiscoveryFailure(
							base,
							adapter,
							discovery,
							exact_operation_id,
							request_fingerprint,
						);
					}

					if (
						expected_observed_hash !== undefined &&
						(discovery._tag !== "Present" || discovery.hash !== expected_observed_hash)
					) {
						return yield* new GlobalGuidanceConflict({
							provider: adapter.provider,
							reason: "drift_changed",
						});
					}

					if (
						expected_state !== undefined &&
						!provider_state_matches(discovery, expected_state)
					) {
						if (discovery._tag === "Absent") {
							return yield* RecordProvider(
								base,
								{
									last_error_code: "guidance_candidate_changed",
									path: discovery.path,
									provider: adapter.provider,
									...(request_fingerprint === undefined
										? {}
										: { request_fingerprint }),
									status: "sync_failed",
								},
								exact_operation_id,
							);
						}

						const observed = yield* content_metadata(discovery.content);

						return yield* RecordProvider(
							base,
							{
								modified_at: discovery.modified_at,
								observed_byte_count: observed.byte_count,
								observed_hash: observed.content_hash,
								path: discovery.path,
								provider: adapter.provider,
								...(request_fingerprint === undefined
									? {}
									: { request_fingerprint }),
								status: "drift_detected",
							},
							exact_operation_id,
						);
					}

					if (
						discovery._tag === "Present" &&
						discovery.hash !== canonical.content_hash &&
						!force &&
						current_metadata?.applied_hash !== undefined &&
						discovery.hash !== current_metadata.applied_hash
					) {
						return yield* RecordProvider(
							base,
							{
								...(current_metadata.ignored_drift_hash === discovery.hash
									? { ignored_drift_hash: discovery.hash }
									: {}),
								modified_at: discovery.modified_at,
								observed_byte_count: new TextEncoder().encode(discovery.content)
									.byteLength,
								observed_hash: discovery.hash,
								path: discovery.path,
								provider: adapter.provider,
								...(request_fingerprint === undefined
									? {}
									: { request_fingerprint }),
								status: "drift_detected",
							},
							exact_operation_id,
						);
					}

					if (discovery._tag === "Absent" || discovery.hash !== canonical.content_hash) {
						const replacement = yield* files.ReplaceAtomic({
							backup_name: backup_name(
								adapter.provider,
								discovery.path,
								yield* snowflake_id.Make("backup"),
							),
							backups_directory: options.backups_directory,
							content: canonical.content,
							...(discovery._tag === "Present"
								? { expected_hash: discovery.hash }
								: {}),
							path: discovery.path,
						});

						if (replacement.backup_path !== undefined) {
							backup = Option.some(replacement.backup_path);
						}

						if (replacement._tag === "Changed") {
							if (expected_observed_hash !== undefined) {
								return yield* new GlobalGuidanceConflict({
									provider: adapter.provider,
									reason: "drift_changed",
								});
							}

							const changed = yield* adapter.Discover;

							if (changed._tag !== "Present") {
								return yield* RecordProvider(
									base,
									{
										...(Option.isSome(backup)
											? { backup_path: backup.value }
											: {}),
										last_error_code: "guidance_sync_raced",
										path: changed.path,
										provider: adapter.provider,
										...(request_fingerprint === undefined
											? {}
											: { request_fingerprint }),
										status: "sync_failed",
									},
									exact_operation_id,
								);
							}

							const observed = yield* content_metadata(changed.content);

							return yield* RecordProvider(
								base,
								{
									...(Option.isSome(backup) ? { backup_path: backup.value } : {}),
									modified_at: changed.modified_at,
									observed_byte_count: observed.byte_count,
									observed_hash: observed.content_hash,
									path: changed.path,
									provider: adapter.provider,
									...(request_fingerprint === undefined
										? {}
										: { request_fingerprint }),
									status: "drift_detected",
								},
								exact_operation_id,
							);
						}
					}

					const verified = yield* VerifyFile(discovery.path, canonical);
					const refreshed = yield* adapter.Discover;

					if (refreshed._tag !== "Present") {
						return yield* RecordProvider(
							base,
							{
								...(Option.isSome(backup) ? { backup_path: backup.value } : {}),
								last_error_code:
									refreshed._tag === "ReadFailed"
										? "guidance_read_failed"
										: "guidance_sync_failed",
								path: refreshed.path,
								provider: adapter.provider,
								...(request_fingerprint === undefined
									? {}
									: { request_fingerprint }),
								status: "sync_failed",
							},
							exact_operation_id,
						);
					}

					const observed = yield* content_metadata(refreshed.content);

					if (observed.content_hash !== canonical.content_hash) {
						return yield* RecordProvider(
							base,
							{
								applied_byte_count: verified.byte_count,
								applied_hash: verified.content_hash,
								...(Option.isSome(backup) ? { backup_path: backup.value } : {}),
								modified_at: refreshed.modified_at,
								observed_byte_count: observed.byte_count,
								observed_hash: observed.content_hash,
								path: refreshed.path,
								provider: adapter.provider,
								...(request_fingerprint === undefined
									? {}
									: { request_fingerprint }),
								status: "drift_detected",
							},
							exact_operation_id,
						);
					}

					return yield* RecordProvider(
						base,
						{
							applied_byte_count: verified.byte_count,
							applied_hash: verified.content_hash,
							...(Option.isSome(backup) ? { backup_path: backup.value } : {}),
							modified_at: refreshed.modified_at,
							observed_byte_count: observed.byte_count,
							observed_hash: observed.content_hash,
							path: discovery.path,
							provider: adapter.provider,
							...(request_fingerprint === undefined ? {} : { request_fingerprint }),
							status: "synced",
						},
						exact_operation_id,
					);
				}).pipe(
					Effect.catchIf(
						(cause): cause is GuidanceFileStoreFailure | GlobalGuidanceInvariantError =>
							cause instanceof GuidanceFileStoreFailure ||
							cause instanceof GlobalGuidanceInvariantError,
						(cause) => {
							const failure_backup = Option.isSome(backup)
								? backup.value
								: cause instanceof GuidanceFileStoreFailure
									? cause.backup_path
									: undefined;

							return RecordProvider(
								base,
								{
									...(failure_backup === undefined
										? {}
										: { backup_path: failure_backup }),
									last_error_code:
										cause instanceof GuidanceFileStoreFailure
											? guidance_file_error_code(cause)
											: "guidance_sync_failed",
									...(path === undefined ? {} : { path }),
									provider: adapter.provider,
									...(request_fingerprint === undefined
										? {}
										: { request_fingerprint }),
									status: "sync_failed",
								},
								exact_operation_id,
							);
						},
					),
				);
			};

			const SyncAll = (
				canonical: CanonicalContent,
				base: string,
				force: boolean,
				expectations?: ProviderExpectations,
			) =>
				Effect.gen(function* () {
					yield* RecordNonNative(base);
					const native = providers.Providers.filter(
						(adapter): adapter is NativeGuidanceProviderAdapter =>
							adapter.mode === "native_file",
					);

					yield* Effect.forEach(
						native,
						(adapter) =>
							SyncNative(
								adapter,
								canonical,
								base,
								force,
								false,
								undefined,
								undefined,
								expectations?.get(adapter.provider),
							),
						{ concurrency: "unbounded", discard: true },
					);
				});

			const MakeCandidates = (
				discoveries: ReadonlyArray<{
					readonly adapter: NativeGuidanceProviderAdapter;
					readonly discovery: GuidanceDiscovery;
				}>,
			) =>
				Effect.forEach(
					discoveries,
					({ adapter, discovery }) =>
						discovery._tag === "Present"
							? content_metadata(discovery.content).pipe(
									Effect.map((content) =>
										Option.some({
											byte_count: content.byte_count,
											content_hash: content.content_hash,
											modified_at: discovery.modified_at,
											path: discovery.path,
											preview: content.content,
											provider: adapter.provider,
										} satisfies GlobalGuidanceCandidate),
									),
								)
							: Effect.succeed(Option.none<GlobalGuidanceCandidate>()),
					{ concurrency: "unbounded" },
				).pipe(
					Effect.map((candidates) =>
						candidates.filter(Option.isSome).map(Option.getOrThrow),
					),
				);

			const Snapshot = Effect.gen(function* () {
				const stored = yield* repository.Read;
				const canonical = yield* ReadCanonical;
				const candidates =
					stored.canonical.status === "selection_required"
						? yield* DiscoverAllNative.pipe(Effect.flatMap(MakeCandidates))
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

					if (Option.isSome(existing)) {
						return existing.value;
					}

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

					yield* SyncAll(written, command.message_id, force_provider_sync, expectations);

					return acceptance;
				});

			const InitializeInternal = Effect.gen(function* () {
				const stored = yield* repository.Read;
				const canonical = yield* ReadCanonical;

				if (stored.canonical.status === "ready" && Option.isSome(canonical)) {
					if (stored.canonical.content_hash !== canonical.value.content_hash) {
						const now = yield* metadata.Now;
						const operation_id = `guidance_recovery_${canonical.value.content_hash.slice(0, 24)}`;

						yield* CommitCanonical(
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
					} else {
						yield* SyncAll(canonical.value, "guidance_refresh", false);
					}

					return yield* Snapshot;
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

					const recovered = yield* content_metadata(recovery_source.discovery.content);
					const written = yield* WriteCanonical(recovered, "canonical-recovery");

					yield* SyncAll(written, "guidance_recovery", false);

					return yield* Snapshot;
				}

				const candidates = yield* MakeCandidates(discoveries);
				const failures = discoveries.filter((item) => item.discovery._tag === "ReadFailed");
				const present_by_hash = Map.groupBy(
					candidates,
					(candidate) => candidate.content_hash,
				);

				for (const failure of failures) {
					if (failure.discovery._tag === "ReadFailed") {
						yield* RecordDiscoveryFailure(
							"guidance_initialization",
							failure.adapter,
							failure.discovery,
						);
					}
				}

				if (failures.length > 0 || present_by_hash.size > 1) {
					if (candidates.length > 0) {
						const now = yield* metadata.Now;
						const hashes = [...present_by_hash.keys()].sort();
						const operation_id = `guidance_selection_${guidance_hash(hashes.join("\n")).slice(0, 24)}`;

						yield* repository.Accept({
							intent: {
								candidate_hashes: hashes as [string, ...Array<string>],
								type: "guidance.selection.require",
							},
							message_id: operation_id,
							origin: "backend",
							sent_at: now,
						});

						for (const candidate of candidates) {
							yield* RecordProvider("guidance_selection", {
								modified_at: candidate.modified_at,
								observed_byte_count: candidate.byte_count,
								observed_hash: candidate.content_hash,
								path: candidate.path,
								provider: candidate.provider,
								status: "awaiting_selection",
							});
						}
					}

					return yield* Snapshot;
				}

				const selected = candidates[0];
				const initial = yield* content_metadata(selected?.preview ?? "");
				const now = yield* metadata.Now;
				const operation_id = `guidance_initial_${initial.content_hash.slice(0, 24)}`;

				yield* CommitCanonical(
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

				return yield* Snapshot;
			});

			const Get = Semaphore.withPermit(lock)(
				InitializeInternal.pipe(Effect.andThen(Snapshot)),
			);
			const Initialize = Semaphore.withPermit(lock)(InitializeInternal);
			const Update = (input: GlobalGuidanceMutationTrace & { readonly content: string }) =>
				Semaphore.withPermit(lock)(
					Effect.gen(function* () {
						const canonical = yield* content_metadata(input.content);
						const acceptance = yield* CommitCanonical(
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
				);
			const Select = (input: GlobalGuidanceSelectionRequest & GlobalGuidanceMutationTrace) =>
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
									(current) => current.adapter.provider === candidate.provider,
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

						const canonical = yield* content_metadata(selected.discovery.content);
						const acceptance = yield* CommitCanonical(
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
				);
			const ResolveDrift = (
				input: GlobalGuidanceDriftResolutionRequest & GlobalGuidanceMutationTrace,
			) =>
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
								return { acceptance: duplicate.value, snapshot: yield* Snapshot };
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

						if (Option.isSome(mutation) && Option.isSome(mutation.value.acceptance)) {
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

						const observed = yield* content_metadata(discovery.content);

						if (input.action === "import") {
							const request_fingerprint = import_request_fingerprint!;
							const acceptance = yield* CommitCanonical(
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

							const acceptance = yield* SyncNative(
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
				);
			const RetrySync = (input: GlobalGuidanceRetryRequest & GlobalGuidanceMutationTrace) =>
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
							acceptance = yield* SyncNative(
								adapter,
								canonical.value,
								input.message_id,
								false,
								true,
								request_fingerprint,
							);
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
				);
			const ResolveForEngine = (engine_id: string) =>
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
				);

			yield* InitializeInternal;

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
	);
}
