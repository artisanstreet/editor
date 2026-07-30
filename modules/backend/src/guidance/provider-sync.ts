import { Context, Effect, Layer, Option } from "effect";

import { SnowflakeId } from "@artisan/protocol";

import {
	backup_name,
	type CanonicalContent,
	ContentMetadata,
	type ExpectedProviderState,
	guidance_file_error_code,
	provider_metadata,
	type ProviderExpectations,
	provider_state_matches,
	reconciliation_operation_id,
} from "./content";
import {
	GlobalGuidanceConflict,
	GlobalGuidanceInvariantError,
	type GlobalGuidanceServiceError,
	type GlobalGuidanceServiceOptions,
} from "./contracts";
import { GuidanceCanonical } from "./canonical";
import { GuidanceFileStore, GuidanceFileStoreFailure } from "./file-store";
import {
	type GuidanceDiscovery,
	GuidanceProviderRegistry,
	type NativeGuidanceProviderAdapter,
} from "./provider-mirrors";
import {
	GlobalGuidanceRepository,
	type GlobalGuidanceAcceptance,
	type GlobalGuidanceReconciliationInput,
} from "./repository";

type ProviderReconciliation = Omit<GlobalGuidanceReconciliationInput, "operation_id">;

export class GuidanceProviderSync extends Context.Service<
	GuidanceProviderSync,
	{
		readonly DiscoverAllNative: Effect.Effect<
			ReadonlyArray<{
				readonly adapter: NativeGuidanceProviderAdapter;
				readonly discovery: GuidanceDiscovery;
			}>,
			GlobalGuidanceServiceError
		>;
		readonly RecordDiscoveryFailure: (
			base: string,
			adapter: NativeGuidanceProviderAdapter,
			discovery: Extract<GuidanceDiscovery, { readonly _tag: "ReadFailed" }>,
			exact_operation_id?: boolean,
			request_fingerprint?: string,
		) => Effect.Effect<GlobalGuidanceAcceptance, GlobalGuidanceServiceError>;
		readonly RecordProvider: (
			base: string,
			input: ProviderReconciliation,
			exact_operation_id?: boolean,
		) => Effect.Effect<GlobalGuidanceAcceptance, GlobalGuidanceServiceError>;
		readonly SyncAll: (
			canonical: CanonicalContent,
			base: string,
			force: boolean,
			expectations?: ProviderExpectations,
		) => Effect.Effect<void, GlobalGuidanceServiceError>;
		readonly SyncNative: (
			adapter: NativeGuidanceProviderAdapter,
			canonical: CanonicalContent,
			base: string,
			force: boolean,
			exact_operation_id?: boolean,
			request_fingerprint?: string,
			expected_observed_hash?: string,
			expected_state?: ExpectedProviderState,
		) => Effect.Effect<GlobalGuidanceAcceptance, GlobalGuidanceServiceError>;
	}
>()("Artisan/GuidanceProviderSync") {}

export const make_guidance_provider_sync_layer = (options: GlobalGuidanceServiceOptions) =>
	Layer.effect(
		GuidanceProviderSync,
		Effect.gen(function* () {
			const files = yield* GuidanceFileStore;
			const providers = yield* GuidanceProviderRegistry;
			const repository = yield* GlobalGuidanceRepository;
			const snowflake_id = yield* SnowflakeId;
			const canonical = yield* GuidanceCanonical;

			const RecordProvider = (
				base: string,
				input: ProviderReconciliation,
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
				expected: CanonicalContent,
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

						const observed = yield* ContentMetadata(discovery.content);

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
						discovery.hash !== expected.content_hash &&
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

					if (discovery._tag === "Absent" || discovery.hash !== expected.content_hash) {
						const replacement = yield* files.ReplaceAtomic({
							backup_name: backup_name(
								adapter.provider,
								discovery.path,
								yield* snowflake_id.Make("backup"),
							),
							backups_directory: options.backups_directory,
							content: expected.content,
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

							const observed = yield* ContentMetadata(changed.content);

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

					const verified = yield* canonical.Verify(discovery.path, expected);
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

					const observed = yield* ContentMetadata(refreshed.content);

					if (observed.content_hash !== expected.content_hash) {
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
				expected: CanonicalContent,
				base: string,
				force: boolean,
				expectations?: ProviderExpectations,
			) =>
				Effect.gen(function* () {
					yield* RecordNonNative(base);

					yield* Effect.forEach(
						providers.Providers.filter(
							(adapter): adapter is NativeGuidanceProviderAdapter =>
								adapter.mode === "native_file",
						),
						(adapter) =>
							SyncNative(
								adapter,
								expected,
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

			return {
				DiscoverAllNative,
				RecordDiscoveryFailure,
				RecordProvider,
				SyncAll,
				SyncNative,
			};
		}),
	);
