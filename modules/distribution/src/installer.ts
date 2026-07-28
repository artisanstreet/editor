import { Context, Data, Effect, FileSystem, Layer, Option, Path, Result, Schema } from "effect";

import {
	Activation,
	type ActivationFailure,
	type ActivationInvalidVersion,
	type ActivationRollbackUnavailable,
	type ActivationVersionUnavailable,
} from "./activation";
import { SelectReleaseArtifact } from "./artifact-selection";
import { AbsolutePath, Architecture, Platform, ReleaseChannel, SemanticVersion } from "./common";
import {
	type ActivatedInstallationManifest,
	InstalledComponents,
	type InstalledIntegrations,
	type InstallationManifest,
} from "./installation-manifest";
import {
	InstallationStore,
	type InstallationRootMismatch,
	type InstallationState,
	type InstallationStoreFailure,
} from "./installation-store";
import type { ReleaseArtifact, ReleaseManifest } from "./release-manifest";
import { ReleaseVerification, type ReleaseVerificationError } from "./verification";

export const InstallerConfiguration = Schema.Struct({
	install_root: AbsolutePath,
	platform: Platform,
	architecture: Architecture,
	channel: ReleaseChannel,
	bootstrap_version: SemanticVersion,
	cli_version: Schema.optional(SemanticVersion),
	permanent_ae_path: AbsolutePath,
	components: InstalledComponents,
});
export type InstallerConfiguration = typeof InstallerConfiguration.Type;

export interface ReleaseEnvelope {
	readonly manifest: Uint8Array;
	readonly signature: unknown;
}

export class ReleaseSourceFailure extends Data.TaggedError("ReleaseSourceFailure")<{
	readonly cause: unknown;
}> {}

export class ArtifactDownloadFailure extends Data.TaggedError("ArtifactDownloadFailure")<{
	readonly cause: unknown;
	readonly artifact_id: string;
}> {}

export class ArtifactStagingFailure extends Data.TaggedError("ArtifactStagingFailure")<{
	readonly cause: unknown;
	readonly target_version: string;
}> {}

export class IntegrationFailure extends Data.TaggedError("IntegrationFailure")<{
	readonly cause: unknown;
	readonly target_version: string;
}> {}

export class InstallationHealthFailure extends Data.TaggedError("InstallationHealthFailure")<{
	readonly cause: unknown;
	readonly version: string;
}> {}

export class InstallerStateFailure extends Data.TaggedError("InstallerStateFailure")<{
	readonly state: InstallationState["_tag"];
}> {}

export class CurrentInstallationConvergenceFailure extends Data.TaggedError(
	"CurrentInstallationConvergenceFailure",
)<{
	readonly cause?: unknown;
	readonly code: "active_pointer" | "version_layout";
	readonly version: string;
}> {}

export class ArtifactSelectionFailure extends Data.TaggedError("ArtifactSelectionFailure")<{
	readonly cause: unknown;
}> {}

export class InstallationRollbackFailure extends Data.TaggedError("InstallationRollbackFailure")<{
	readonly cause: unknown;
	readonly failed_version: string;
	readonly restored_version: string;
}> {}

export class VersionRetentionFailure extends Data.TaggedError("VersionRetentionFailure")<{
	readonly cause: unknown;
	readonly version: string;
}> {}

export class ReleaseSource extends Context.Service<
	ReleaseSource,
	{
		readonly Resolve: (
			channel: ReleaseChannel,
		) => Effect.Effect<ReleaseEnvelope, ReleaseSourceFailure>;
	}
>()("Artisan/Distribution/ReleaseSource") {}

export class ArtifactDownloader extends Context.Service<
	ArtifactDownloader,
	{
		readonly Download: (
			artifact: ReleaseArtifact,
		) => Effect.Effect<Uint8Array, ArtifactDownloadFailure>;
	}
>()("Artisan/Distribution/ArtifactDownloader") {}

export interface ArtifactStagingRequest {
	readonly artifact: ReleaseArtifact;
	readonly bytes: Uint8Array;
	readonly release: ReleaseManifest;
	readonly staging_path: string;
	readonly version_path: string;
}

export class ArtifactStager extends Context.Service<
	ArtifactStager,
	{
		/**
		 * Extracts into staging, validates the declared archive-entry boundary, then
		 * atomically publishes the completed version directory.
		 */
		readonly Stage: (
			request: ArtifactStagingRequest,
		) => Effect.Effect<void, ArtifactStagingFailure>;
	}
>()("Artisan/Distribution/ArtifactStager") {}

export interface IntegrationRequest {
	readonly artifact: ReleaseArtifact;
	readonly previous: InstalledIntegrations;
	readonly release: ReleaseManifest;
	readonly version_path: string;
}

export class InstallationIntegrations extends Context.Service<
	InstallationIntegrations,
	{
		readonly Apply: (
			request: IntegrationRequest,
		) => Effect.Effect<InstalledIntegrations, IntegrationFailure>;
	}
>()("Artisan/Distribution/InstallationIntegrations") {}

export class InstallationHealth extends Context.Service<
	InstallationHealth,
	{
		readonly Check: (version: string) => Effect.Effect<void, InstallationHealthFailure>;
	}
>()("Artisan/Distribution/InstallationHealth") {}

export interface RunningForgeSnapshot {
	readonly was_running: boolean;
}

export class ForgeUpdateLifecycleFailure extends Data.TaggedError("ForgeUpdateLifecycleFailure")<{
	readonly cause?: unknown;
	readonly operation: "quiesce" | "restore" | "resume" | "verify_current";
	readonly version: string;
}> {}

export class ForgeUpdateLifecycle extends Context.Service<
	ForgeUpdateLifecycle,
	{
		readonly Quiesce: (
			version: string,
		) => Effect.Effect<RunningForgeSnapshot, ForgeUpdateLifecycleFailure>;
		readonly Restore: (
			version: string,
			snapshot: RunningForgeSnapshot,
		) => Effect.Effect<void, ForgeUpdateLifecycleFailure>;
		readonly ResumeAndVerify: (
			version: string,
			snapshot: RunningForgeSnapshot,
		) => Effect.Effect<void, ForgeUpdateLifecycleFailure>;
		readonly VerifyCurrent: (
			version: string,
		) => Effect.Effect<void, ForgeUpdateLifecycleFailure>;
	}
>()("Artisan/Distribution/ForgeUpdateLifecycle") {}

export type InstallerOutcome =
	| { readonly _tag: "Current"; readonly manifest: ActivatedInstallationManifest }
	| { readonly _tag: "Installed"; readonly manifest: ActivatedInstallationManifest }
	| { readonly _tag: "Updated"; readonly manifest: ActivatedInstallationManifest };

export type InstallerError =
	| ActivationFailure
	| ActivationInvalidVersion
	| ActivationRollbackUnavailable
	| ActivationVersionUnavailable
	| ArtifactDownloadFailure
	| ArtifactSelectionFailure
	| ArtifactStagingFailure
	| InstallationHealthFailure
	| InstallationRollbackFailure
	| InstallationRootMismatch
	| InstallationStoreFailure
	| IntegrationFailure
	| InstallerStateFailure
	| CurrentInstallationConvergenceFailure
	| ForgeUpdateLifecycleFailure
	| ReleaseSourceFailure
	| ReleaseVerificationError
	| VersionRetentionFailure;

export class Installer extends Context.Service<
	Installer,
	{
		/** Converges an absent, interrupted, or active installation on the selected release. */
		readonly Converge: () => Effect.Effect<InstallerOutcome, InstallerError>;
	}
>()("Artisan/Distribution/Installer") {}

const IsoNow = () =>
	Effect.map(
		Effect.clockWith((clock) => clock.currentTimeMillis),
		(milliseconds) => new Date(milliseconds).toISOString(),
	);

const SelectArtifact = (release: ReleaseManifest, configuration: InstallerConfiguration) =>
	Result.match(
		SelectReleaseArtifact({
			release,
			target: {
				platform: configuration.platform,
				architecture: configuration.architecture,
			},
			bootstrap_version: configuration.bootstrap_version,
			...(configuration.cli_version === undefined
				? {}
				: { cli_version: configuration.cli_version }),
		}),
		{
			onFailure: (cause) => Effect.fail(new ArtifactSelectionFailure({ cause })),
			onSuccess: Effect.succeed,
		},
	);

/** Composes the platform adapters into the transactional distribution lifecycle. */
export const make_installer_layer = (configuration_input: unknown) =>
	Layer.effect(
		Installer,
		Effect.gen(function* () {
			const configuration =
				yield* Schema.decodeUnknownEffect(InstallerConfiguration)(configuration_input);
			const path_service = yield* Path.Path;
			const file_system = yield* FileSystem.FileSystem;
			const store = yield* InstallationStore;
			const activation = yield* Activation;
			const source = yield* ReleaseSource;
			const downloader = yield* ArtifactDownloader;
			const stager = yield* ArtifactStager;
			const integrations = yield* InstallationIntegrations;
			const health = yield* InstallationHealth;
			const forge_lifecycle = yield* ForgeUpdateLifecycle;
			const verification = yield* ReleaseVerification;
			const DecodeVersionDirectory = Schema.decodeUnknownOption(SemanticVersion);

			const RetainRollbackVersions = (
				versions_path: string,
				active_version: string,
				previous_version: string,
			) =>
				Effect.gen(function* () {
					const retained = new Set([active_version, previous_version]);
					for (const candidate of yield* file_system.readDirectory(versions_path).pipe(
						Effect.mapError(
							(cause) =>
								new VersionRetentionFailure({
									cause,
									version: active_version,
								}),
						),
					)) {
						if (retained.has(candidate)) continue;
						const decoded = DecodeVersionDirectory(candidate);
						if (Option.isNone(decoded)) continue;
						const candidate_path = path_service.join(versions_path, candidate);
						const metadata = yield* file_system.stat(candidate_path).pipe(
							Effect.mapError(
								(cause) =>
									new VersionRetentionFailure({
										cause,
										version: candidate,
									}),
							),
						);
						if (metadata.type !== "Directory") continue;
						yield* file_system.remove(candidate_path, { recursive: true }).pipe(
							Effect.mapError(
								(cause) =>
									new VersionRetentionFailure({
										cause,
										version: candidate,
									}),
							),
						);
					}
				});

			const Converge = () =>
				Effect.gen(function* () {
					const initial_state = yield* store.Inspect();
					if (initial_state._tag === "Malformed")
						return yield* new InstallerStateFailure({ state: initial_state._tag });

					const envelope = yield* source.Resolve(configuration.channel);
					const release = yield* verification.VerifyManifest(
						envelope.manifest,
						envelope.signature,
					);
					const artifact = yield* SelectArtifact(release, configuration);

					if (
						initial_state._tag !== "Absent" &&
						initial_state.manifest.activation_state === "active" &&
						initial_state.manifest.transaction.state === "idle" &&
						initial_state.manifest.active_version === release.product_version
					) {
						const active = yield* activation.ReadActive();
						if (
							active._tag !== "Active" ||
							active.pointer.active_version !== release.product_version
						)
							return yield* new CurrentInstallationConvergenceFailure({
								code: "active_pointer",
								version: release.product_version,
								...(active._tag === "Malformed" ? { cause: active.cause } : {}),
							});
						const version_path = path_service.join(
							configuration.install_root,
							"versions",
							release.product_version,
						);
						const metadata = yield* file_system.stat(version_path).pipe(
							Effect.mapError(
								(cause) =>
									new CurrentInstallationConvergenceFailure({
										cause,
										code: "version_layout",
										version: release.product_version,
									}),
							),
						);
						if (metadata.type !== "Directory")
							return yield* new CurrentInstallationConvergenceFailure({
								code: "version_layout",
								version: release.product_version,
							});
						const installed_integrations = yield* integrations.Apply({
							artifact,
							previous: initial_state.manifest.integrations,
							release,
							version_path,
						});
						yield* health.Check(release.product_version);
						if (initial_state.manifest.finalization_state === "complete")
							yield* forge_lifecycle.VerifyCurrent(release.product_version);
						if (initial_state.manifest.previous_version !== undefined)
							yield* RetainRollbackVersions(
								path_service.join(configuration.install_root, "versions"),
								initial_state.manifest.active_version,
								initial_state.manifest.previous_version,
							);
						const manifest =
							JSON.stringify(installed_integrations) ===
							JSON.stringify(initial_state.manifest.integrations)
								? initial_state.manifest
								: {
										...initial_state.manifest,
										integrations: installed_integrations,
										updated_at: yield* IsoNow(),
									};
						if (manifest !== initial_state.manifest) yield* store.WriteAtomic(manifest);
						return {
							_tag: "Current" as const,
							manifest,
						};
					}

					const previous =
						initial_state._tag === "Absent"
							? undefined
							: initial_state.manifest.activation_state === "active"
								? initial_state.manifest
								: undefined;
					const started_at = yield* IsoNow();
					const layout = yield* activation.EnsureLayout();
					const staging_path = path_service.join(layout.staging, release.product_version);
					const version_path = path_service.join(
						layout.versions,
						release.product_version,
					);
					const common = {
						format_version: 1 as const,
						install_root: configuration.install_root,
						platform: configuration.platform,
						architecture: configuration.architecture,
						channel: configuration.channel,
						components: configuration.components,
						integrations: previous?.integrations ?? {},
						installed_at: previous?.installed_at ?? started_at,
					};
					const WriteTransaction = (
						state: "activating" | "downloading" | "integrating" | "staged" | "verified",
						updated_at: string,
					) =>
						store.WriteAtomic(
							previous === undefined
								? {
										...common,
										activation_state: "unactivated",
										transaction: {
											state,
											target_version: release.product_version,
											staging_path,
											started_at,
										},
										updated_at,
									}
								: {
										...previous,
										...common,
										transaction: {
											state,
											target_version: release.product_version,
											staging_path,
											started_at,
										},
										updated_at,
									},
						);

					yield* WriteTransaction("downloading", started_at);
					const bytes = yield* downloader.Download(artifact);
					yield* verification.VerifyArtifact(artifact, bytes);
					yield* WriteTransaction("verified", yield* IsoNow());
					yield* stager.Stage({
						artifact,
						bytes,
						release,
						staging_path,
						version_path,
					});
					yield* WriteTransaction("staged", yield* IsoNow());
					yield* WriteTransaction("integrating", yield* IsoNow());
					const installed_integrations = yield* integrations.Apply({
						artifact,
						previous: previous?.integrations ?? {},
						release,
						version_path,
					});
					yield* health
						.Check(release.product_version)
						.pipe(
							Effect.onError(() =>
								previous === undefined
									? Effect.void
									: store.WriteAtomic(previous).pipe(Effect.ignore),
							),
						);
					const running_snapshot: RunningForgeSnapshot =
						previous === undefined
							? { was_running: false }
							: yield* forge_lifecycle.Quiesce(previous.active_version);
					const activated_at = yield* IsoNow();
					const activated: ActivatedInstallationManifest = {
						...common,
						activation_state: "active",
						finalization_state: previous?.finalization_state ?? "pending",
						active_version: release.product_version,
						permanent_ae_path: configuration.permanent_ae_path,
						...(previous === undefined
							? {}
							: { previous_version: previous.active_version }),
						artifact: {
							artifact_id: artifact.artifact_id,
							sha256: artifact.sha256,
							signing_key_id: release.signing_identity.key_id,
						},
						integrations: installed_integrations,
						transaction: { state: "idle" },
						updated_at: activated_at,
					};
					const switch_result = yield* Effect.gen(function* () {
						yield* WriteTransaction("activating", yield* IsoNow());
						yield* activation.Activate(release.product_version);
						yield* store.WriteAtomic(activated);
						if (previous !== undefined)
							yield* forge_lifecycle.ResumeAndVerify(
								release.product_version,
								running_snapshot,
							);
					}).pipe(Effect.result);
					if (switch_result._tag === "Failure") {
						const active = yield* activation.ReadActive().pipe(Effect.result);
						const switched =
							active._tag === "Success" &&
							active.success._tag === "Active" &&
							active.success.pointer.active_version === release.product_version;
						if (previous === undefined) {
							if (switched) yield* activation.Deactivate().pipe(Effect.ignore);
							yield* store
								.WriteAtomic({
									...common,
									activation_state: "unactivated",
									transaction: {
										state: "activating",
										target_version: release.product_version,
										staging_path,
										started_at,
									},
									updated_at: activated_at,
								})
								.pipe(Effect.ignore);
							return yield* Effect.fail(switch_result.failure);
						}
						if (switched) {
							const rollback_pending: InstallationManifest = {
								...previous,
								transaction: {
									state: "rollback_pending",
									target_version: release.product_version,
									restore_version: previous.active_version,
									staging_path,
									started_at,
								},
								updated_at: activated_at,
							};
							yield* store.WriteAtomic(rollback_pending).pipe(Effect.ignore);
						}
						const pointer_result = switched
							? yield* activation.Rollback().pipe(Effect.result)
							: undefined;
						const manifest_result = yield* store
							.WriteAtomic(previous)
							.pipe(Effect.result);
						const restore_result = yield* forge_lifecycle
							.Restore(previous.active_version, running_snapshot)
							.pipe(Effect.result);
						return yield* new InstallationRollbackFailure({
							cause: {
								update: switch_result.failure,
								...(pointer_result?._tag === "Failure"
									? { pointer: pointer_result.failure }
									: {}),
								...(manifest_result._tag === "Failure"
									? { manifest: manifest_result.failure }
									: {}),
								...(restore_result._tag === "Failure"
									? { restore: restore_result.failure }
									: {}),
							},
							failed_version: release.product_version,
							restored_version: previous.active_version,
						});
					}

					if (previous !== undefined)
						yield* RetainRollbackVersions(
							layout.versions,
							activated.active_version,
							previous.active_version,
						);

					return {
						_tag:
							previous === undefined ? ("Installed" as const) : ("Updated" as const),
						manifest: activated,
					};
				});

			return Installer.of({ Converge });
		}),
	);
