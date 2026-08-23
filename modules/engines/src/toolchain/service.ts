import { createHash, randomUUID } from "node:crypto";
import { homedir } from "node:os";

import {
	Clock,
	Context,
	Data,
	Effect,
	FileSystem,
	Layer,
	Option,
	Path,
	Ref,
	Schema,
	Scope,
	Semaphore,
	Stream,
} from "effect";

import { EngineProcessFactory } from "../process/process";
import type { EngineSpawnOverride } from "../process/spawn-override";
import { OpenCode2ManagedConfigContent } from "../opencode2/config";
import {
	engine_distributions,
	maximum_engine_binary_bytes,
	ToolchainSemanticVersion,
	type EngineDistribution,
	type ToolchainPlatformTarget,
	type ToolchainReleaseError,
} from "./distribution";
import { ToolchainHttpFailure, ToolchainReleaseHttp } from "./http";
import { ExtractNpmTarballExecutable } from "./npm-tarball";
import { ExtractZipBundle } from "./zip-bundle";
import {
	make_toolchain_layout,
	PruneToolchainVersions,
	default_toolchain_profile_id,
	ReadToolchainState,
	ValidateToolchainPathComponent,
	ValidateToolchainRelativePath,
	WriteToolchainState,
	type ToolchainInstallationState,
	type ToolchainLayout,
	type ToolchainStateError,
} from "./store";

export class EngineToolchainUnknownEngineError extends Data.TaggedError(
	"EngineToolchainUnknownEngineError",
)<{
	readonly engine_id: string;
}> {}

export class EngineToolchainBusyError extends Data.TaggedError("EngineToolchainBusyError")<{
	readonly engine_id: string;
}> {}

export class EngineToolchainVerificationError extends Data.TaggedError(
	"EngineToolchainVerificationError",
)<{
	readonly actual: string;
	readonly engine_id: string;
	readonly expected: string;
	readonly version: string;
}> {}

export class EngineToolchainStagingError extends Data.TaggedError("EngineToolchainStagingError")<{
	readonly cause: unknown;
	readonly engine_id: string;
	readonly version: string;
}> {}

export class EngineToolchainHealthError extends Data.TaggedError("EngineToolchainHealthError")<{
	readonly engine_id: string;
	readonly message: string;
	readonly version: string;
}> {}

export class EngineToolchainRollbackUnavailableError extends Data.TaggedError(
	"EngineToolchainRollbackUnavailableError",
)<{
	readonly engine_id: string;
}> {}

/** Raised by the disabled composition where no toolchain root exists. */
export class EngineToolchainUnavailableError extends Data.TaggedError(
	"EngineToolchainUnavailableError",
)<{
	readonly engine_id: string;
}> {}

/** A managed engine was requested before an owned healthy binary exists. */
export class EngineToolchainManagedInstallMissingError extends Data.TaggedError(
	"EngineToolchainManagedInstallMissingError",
)<{
	readonly engine_id: string;
	readonly reason: "invalid_state" | "missing_binary" | "not_installed";
}> {}

export class EngineToolchainAuthenticationError extends Data.TaggedError(
	"EngineToolchainAuthenticationError",
)<{
	readonly engine_id: string;
	readonly message: string;
}> {}

export type EngineToolchainFailure =
	| EngineToolchainBusyError
	| EngineToolchainAuthenticationError
	| EngineToolchainHealthError
	| EngineToolchainManagedInstallMissingError
	| EngineToolchainRollbackUnavailableError
	| EngineToolchainStagingError
	| EngineToolchainUnavailableError
	| EngineToolchainUnknownEngineError
	| EngineToolchainVerificationError
	| ToolchainHttpFailure
	| ToolchainReleaseError
	| ToolchainStateError;

/** Renders a short, secret-free message for a failed toolchain operation. */
export const describe_engine_toolchain_failure = (failure: EngineToolchainFailure): string => {
	switch (failure._tag) {
		case "EngineToolchainBusyError":
			return "Another install is already running for this engine.";
		case "EngineToolchainAuthenticationError":
			return "The managed engine sign-in did not complete.";
		case "EngineToolchainHealthError":
			return "The managed binary failed its health check.";
		case "EngineToolchainManagedInstallMissingError":
			return "Artisan has no healthy managed binary installed for this engine.";
		case "EngineToolchainRollbackUnavailableError":
			return "No previous version is retained to roll back to.";
		case "EngineToolchainStagingError":
			return failure.engine_id === "hermes"
				? "The Hermes installer did not complete."
				: "The downloaded binary could not be written into the toolchain.";
		case "EngineToolchainUnavailableError":
			return "Engine installs are not available in this composition.";
		case "EngineToolchainUnknownEngineError":
			return `No managed distribution exists for engine "${failure.engine_id}".`;
		case "EngineToolchainVerificationError":
			return "The managed binary failed its integrity check.";
		case "ToolchainHttpFailure":
			return `The release download failed (${failure.code}).`;
		case "ToolchainReleaseError":
			return failure.reason === "platform_unsupported"
				? "This platform has no managed binary for the engine yet."
				: failure.reason === "version_unavailable"
					? "The requested version does not exist in the vendor's release channel."
					: "The vendor's release metadata could not be read.";
		case "ToolchainStateError":
			return "The toolchain installation record could not be accessed.";
	}
};

export type EngineToolchainInstallPhase =
	| "checking"
	| "downloading"
	| "provisioning"
	| "resolving"
	| "staging"
	| "verifying";

export type EngineToolchainActivity =
	| { readonly _tag: "authenticating" }
	| { readonly _tag: "failed"; readonly message: string }
	| {
			readonly _tag: "installing";
			readonly detail?: string;
			readonly phase: EngineToolchainInstallPhase;
			readonly version?: string;
	  }
	| { readonly _tag: "idle" };

export interface EngineToolchainStatus {
	readonly active_version?: string;
	readonly activity: EngineToolchainActivity;
	readonly credentials_present: boolean;
	readonly display_name: string;
	readonly engine_id: string;
	readonly executable_path?: string;
	readonly home_path: string;
	readonly latest_version?: string;
	readonly minimum_version?: string;
	readonly previous_version?: string;
	readonly recommended_version?: string;
}

export interface EngineToolchainReadOptions {
	readonly check_updates?: boolean;
}

/**
 * Owns engine binaries and config homes end to end: vendor artifacts are
 * downloaded straight from source, checksum-verified, health-checked, and
 * activated under Artisan's toolchain root; the engines are then spawned from
 * that root with `CLAUDE_CONFIG_DIR`/`CODEX_HOME` pointing at owned homes.
 */
export class EngineToolchain extends Context.Service<
	EngineToolchain,
	{
		readonly Install: (
			engine_id: string,
			version?: string,
		) => Effect.Effect<EngineToolchainStatus, EngineToolchainFailure>;
		/** Starts an install in the owning Layer scope after reserving its engine slot. */
		readonly StartInstall: (
			engine_id: string,
			version?: string,
		) => Effect.Effect<EngineToolchainStatus, EngineToolchainFailure>;
		/**
		 * Runs the vendor sign-in into one profile's owned home. Omitting
		 * `profile_id` targets the engine's default profile, which is the
		 * account an engine has always signed in as.
		 */
		readonly StartAuthentication: (
			engine_id: string,
			profile_id?: string,
		) => Effect.Effect<EngineToolchainStatus, EngineToolchainFailure>;
		readonly List: (
			options?: EngineToolchainReadOptions,
		) => Effect.Effect<ReadonlyArray<EngineToolchainStatus>>;
		/**
		 * Resolves the spawn override engines consult before every process
		 * start. Production composition is managed-only: a missing or malformed
		 * owned installation is a typed error, never a PATH fallback.
		 *
		 * `profile_id` selects which account home the process is spawned
		 * against; the binary is shared, so concurrent spawns under different
		 * profiles are independent runs of one installation.
		 */
		readonly ResolveSpawn: (
			engine_id: string,
			profile_id?: string,
		) => Effect.Effect<EngineSpawnOverride, EngineToolchainFailure>;
		readonly Rollback: (
			engine_id: string,
		) => Effect.Effect<EngineToolchainStatus, EngineToolchainFailure>;
		readonly Status: (
			engine_id: string,
			options?: EngineToolchainReadOptions,
		) => Effect.Effect<EngineToolchainStatus, EngineToolchainFailure>;
	}
>()("Artisan/EngineToolchain") {}

export interface EngineToolchainOptions {
	readonly distributions?: ReadonlyArray<EngineDistribution>;
	readonly health_check_timeout_ms?: number;
	readonly platform?: ToolchainPlatformTarget;
	readonly root: string;
	readonly user_home_directory?: string;
}

const latest_version_fresh_window_ms = 600_000;

const HashFile = (file_system: FileSystem.FileSystem, file_path: string) =>
	Effect.gen(function* () {
		const hash = createHash("sha256");
		yield* file_system
			.stream(file_path)
			.pipe(Stream.runForEach((chunk) => Effect.sync(() => hash.update(chunk))));
		return hash.digest("hex");
	});

const AsToolchainFailure = (failure: unknown, engine_id: string, version: string) => {
	if (failure !== null && typeof failure === "object" && "_tag" in failure) {
		const tag = failure._tag;
		if (typeof tag === "string" && tag.startsWith("EngineToolchain"))
			return failure as EngineToolchainFailure;
		if (
			tag === "ToolchainHttpFailure" ||
			tag === "ToolchainReleaseError" ||
			tag === "ToolchainStateError"
		)
			return failure as EngineToolchainFailure;
	}
	return new EngineToolchainStagingError({ cause: failure, engine_id, version });
};

interface LatestVersionCacheEntry {
	readonly fetched_at_ms: number;
	readonly version: string;
}

interface VerifiedExecutableCacheEntry {
	readonly expected_sha256: string;
	readonly fingerprint: string;
}

/**
 * Captures cheap drift evidence around an expensive digest read. A changed
 * path, size, timestamp, inode, or mode forces a fresh SHA-256 before spawn;
 * a Forge restart naturally starts with an empty cache and revalidates once.
 */
const executable_fingerprint = (metadata: FileSystem.File.Info) =>
	[
		String(metadata.size),
		Option.match(metadata.mtime, {
			onNone: () => "mtime:none",
			onSome: (value) => `mtime:${value.getTime()}`,
		}),
		`dev:${metadata.dev}`,
		Option.match(metadata.ino, {
			onNone: () => "ino:none",
			onSome: (value) => `ino:${value}`,
		}),
		`mode:${metadata.mode}`,
	].join("|");

/** Reads bounded stdout from a spawned health-check process. */
const ReadHealthOutput = (stream: AsyncIterable<Uint8Array>, maximum_bytes: number) =>
	Effect.tryPromise({
		try: async () => {
			const chunks: Array<Uint8Array> = [];
			let total = 0;
			for await (const chunk of stream) {
				total += chunk.length;
				if (total > maximum_bytes) throw new Error("Health check output exceeded bounds");
				chunks.push(chunk);
			}
			return chunks.map((chunk) => new TextDecoder().decode(chunk)).join("");
		},
		catch: (cause) => cause,
	});

export const make_engine_toolchain_layer = (options: EngineToolchainOptions) =>
	Layer.effect(
		EngineToolchain,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path = yield* Path.Path;
			/** The Forge DB lease is the cross-process install owner; this slot only serializes this Layer. */
			const service_scope = yield* Scope.Scope;
			const factory = yield* EngineProcessFactory;
			const http = yield* ToolchainReleaseHttp;
			const distributions = options.distributions ?? engine_distributions;
			const platform: ToolchainPlatformTarget = options.platform ?? {
				architecture: process.arch,
				platform: process.platform,
			};
			const user_home = options.user_home_directory ?? homedir();
			const health_check_timeout_ms = options.health_check_timeout_ms ?? 30_000;
			const activities = yield* Ref.make<ReadonlyMap<string, EngineToolchainActivity>>(
				new Map(),
			);
			const states = yield* Ref.make<
				ReadonlyMap<string, { readonly state: ToolchainInstallationState | undefined }>
			>(new Map());
			const latest_versions = yield* Ref.make<ReadonlyMap<string, LatestVersionCacheEntry>>(
				new Map(),
			);
			const verified_executables = yield* Ref.make<
				ReadonlyMap<string, VerifiedExecutableCacheEntry>
			>(new Map());
			const executable_verification_lock = yield* Semaphore.make(1);

			const Distribution = (engine_id: string) => {
				const distribution = distributions.find(
					(candidate) => candidate.engine_id === engine_id,
				);
				return distribution === undefined
					? Effect.fail(new EngineToolchainUnknownEngineError({ engine_id }))
					: Effect.succeed(distribution);
			};

			const Layout = (engine_id: string, profile_id?: string) =>
				make_toolchain_layout(options.root, engine_id, path, profile_id);

			const SetActivity = (engine_id: string, activity: EngineToolchainActivity) =>
				Ref.update(activities, (current) => new Map(current).set(engine_id, activity));

			const CachedState = (engine_id: string, layout: ToolchainLayout) =>
				Effect.gen(function* () {
					const cached = (yield* Ref.get(states)).get(engine_id);
					if (cached !== undefined) return cached.state;
					const state = yield* ReadToolchainState(layout, engine_id, file_system);
					yield* Ref.update(states, (current) =>
						new Map(current).set(engine_id, { state }),
					);
					return state;
				});

			const RememberState = (
				engine_id: string,
				state: ToolchainInstallationState | undefined,
			) => Ref.update(states, (current) => new Map(current).set(engine_id, { state }));

			const LatestVersion = (distribution: EngineDistribution) =>
				Effect.gen(function* () {
					const now_ms = yield* Clock.currentTimeMillis;
					const cached = (yield* Ref.get(latest_versions)).get(distribution.engine_id);
					if (
						cached !== undefined &&
						now_ms - cached.fetched_at_ms < latest_version_fresh_window_ms
					)
						return cached.version;
					const version = yield* distribution.LatestVersion.pipe(
						Effect.provideService(ToolchainReleaseHttp, http),
					);
					yield* Ref.update(latest_versions, (current) =>
						new Map(current).set(distribution.engine_id, {
							fetched_at_ms: now_ms,
							version,
						}),
					);
					return version;
				});

			/** Creates one profile's config home with owner-only access; idempotent. */
			const EnsureHome = (layout: ToolchainLayout) =>
				Effect.gen(function* () {
					yield* file_system.makeDirectory(layout.home_path, { recursive: true });
					if (platform.platform !== "win32")
						yield* file_system.chmod(layout.home_path, 0o700);
				});

			/** Seeds the owned config home once; every step is idempotent. */
			const ProvisionHome = (distribution: EngineDistribution, layout: ToolchainLayout) =>
				Effect.gen(function* () {
					yield* EnsureHome(layout);
					if (distribution.engine_id === "opencode2") {
						for (const directory of ["cache", "config", "data", "state", "tmp"])
							yield* file_system.makeDirectory(
								path.join(layout.home_path, directory),
								{
									recursive: true,
								},
							);
					}
					/**
					 * Only the default profile inherits the ambient vendor sign-in.
					 * Seeding an added profile would clone one account into every
					 * home, which is the opposite of what a second profile is for.
					 */
					if (layout.profile_id !== default_toolchain_profile_id) return;
					/** Import only credentials from a caller-selected vendor home, never from our target. */
					const configured_vendor_home =
						process.env[distribution.home_environment_variable]?.trim();
					const vendor_home =
						configured_vendor_home && configured_vendor_home !== layout.home_path
							? configured_vendor_home
							: path.join(user_home, distribution.vendor_home_directory);
					for (const credential_file of distribution.credential_files) {
						const target = path.join(layout.home_path, credential_file);
						const target_exists = yield* file_system
							.exists(target)
							.pipe(Effect.orElseSucceed(() => false));
						if (target_exists) continue;
						const source = path.join(vendor_home, credential_file);
						const source_metadata = yield* file_system.stat(source).pipe(Effect.option);
						if (
							source_metadata._tag !== "Some" ||
							source_metadata.value.type !== "File"
						)
							continue;
						/**
						 * Opportunistic seed of the vendor sign-in so the owned home
						 * starts authenticated. A failed copy or an invalidated grant
						 * degrades to the ordinary sign-in prompt, never to an error.
						 */
						const temporary_target = `${target}.${randomUUID()}.tmp`;
						yield* file_system.copyFile(source, temporary_target).pipe(
							Effect.andThen(
								platform.platform === "win32"
									? Effect.void
									: file_system.chmod(temporary_target, 0o600),
							),
							Effect.andThen(file_system.rename(temporary_target, target)),
							/** Credential import is an optional migration, never an install failure. */
							Effect.ignore,
							Effect.ensuring(
								file_system.remove(temporary_target).pipe(Effect.ignore),
							),
						);
					}
				});

			const CredentialsPresent = (
				distribution: EngineDistribution,
				layout: ToolchainLayout,
			) =>
				Effect.gen(function* () {
					for (const credential_file of distribution.credential_files) {
						const exists = yield* file_system
							.exists(path.join(layout.home_path, credential_file))
							.pipe(Effect.orElseSucceed(() => false));
						if (exists) return true;
					}
					return false;
				});

			const HealthCheck = (
				distribution: EngineDistribution,
				executable: string,
				version: string,
				environment: Readonly<Record<string, string>>,
			) =>
				Effect.scoped(
					Effect.gen(function* () {
						const handle = yield* factory.Spawn({
							args: ["--version"],
							command: executable,
							env: { ...process.env, ...environment },
						});
						const [stdout, exit] = yield* Effect.all(
							[ReadHealthOutput(handle.Stdout, 1_048_576), handle.Exit],
							{ concurrency: "unbounded" },
						).pipe(Effect.ensuring(handle.Close));
						if (exit.code !== 0)
							return yield* new EngineToolchainHealthError({
								engine_id: distribution.engine_id,
								message: `--version exited ${String(exit.code)}`,
								version,
							});
						if (
							!new RegExp(
								`(?:^|\\s)v?${version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?:\\s|$)`,
							).test(stdout)
						)
							return yield* new EngineToolchainHealthError({
								engine_id: distribution.engine_id,
								message: `--version reported "${stdout.trim()}" instead of ${version}`,
								version,
							});
					}),
				).pipe(
					Effect.timeoutOrElse({
						duration: health_check_timeout_ms,
						orElse: () =>
							Effect.fail(
								new EngineToolchainHealthError({
									engine_id: distribution.engine_id,
									message: "--version timed out",
									version,
								}),
							),
					}),
					Effect.catch((failure) =>
						failure instanceof EngineToolchainHealthError
							? Effect.fail(failure)
							: Effect.fail(
									new EngineToolchainHealthError({
										engine_id: distribution.engine_id,
										message: String(failure),
										version,
									}),
								),
					),
				);

			const HomeEnvironment = (
				distribution: EngineDistribution,
				layout: ToolchainLayout,
			): Readonly<Record<string, string>> => ({
				[distribution.home_environment_variable]:
					distribution.engine_id === "opencode2"
						? path.join(layout.home_path, "config")
						: layout.home_path,
				/** Claude's supported update disablement applies to all managed spawns. */
				...(distribution.engine_id === "claude"
					? {
							DISABLE_INSTALLATION_CHECKS: "1",
							DISABLE_UPDATES: "1",
						}
					: {}),
				...(distribution.engine_id === "opencode2"
					? {
							OPENCODE_CLIENT: "artisan-editor",
							OPENCODE_CONFIG_CONTENT: OpenCode2ManagedConfigContent(),
							OPENCODE_CONFIG_PROJECT_DISABLE: "1",
							OPENCODE_DB: path.join(layout.home_path, "data", "opencode.db"),
							OPENCODE_FILEWATCHER_DISABLE: "1",
							TMP: path.join(layout.home_path, "tmp"),
							TEMP: path.join(layout.home_path, "tmp"),
							XDG_CACHE_HOME: path.join(layout.home_path, "cache"),
							XDG_CONFIG_HOME: path.join(layout.home_path, "config"),
							XDG_DATA_HOME: path.join(layout.home_path, "data"),
							XDG_STATE_HOME: path.join(layout.home_path, "state"),
						}
					: {}),
				...(distribution.engine_id === "cursor"
					? {
							CURSOR_CONFIG_DIR: layout.home_path,
							CURSOR_DATA_DIR: layout.home_path,
						}
					: {}),
			});

			/** Revalidates drifted generations before their owned credentials can be used. */
			const VerifyExecutable = (
				engine_id: string,
				executable: string,
				expected_sha256: string,
				version: string,
			) =>
				executable_verification_lock.withPermit(
					Effect.gen(function* () {
						const before = yield* file_system.stat(executable).pipe(
							Effect.mapError(
								(cause) =>
									new EngineToolchainStagingError({
										cause,
										engine_id,
										version,
									}),
							),
						);
						if (before.type !== "File")
							return yield* new EngineToolchainManagedInstallMissingError({
								engine_id,
								reason: "missing_binary",
							});
						const fingerprint = executable_fingerprint(before);
						const cached = (yield* Ref.get(verified_executables)).get(executable);
						if (
							cached?.expected_sha256 === expected_sha256 &&
							cached.fingerprint === fingerprint
						)
							return;

						const actual = yield* HashFile(file_system, executable).pipe(
							Effect.mapError(
								(cause) =>
									new EngineToolchainStagingError({
										cause,
										engine_id,
										version,
									}),
							),
						);
						const after = yield* file_system.stat(executable).pipe(
							Effect.mapError(
								(cause) =>
									new EngineToolchainStagingError({
										cause,
										engine_id,
										version,
									}),
							),
						);
						if (
							after.type !== "File" ||
							executable_fingerprint(after) !== fingerprint ||
							actual !== expected_sha256
						)
							return yield* new EngineToolchainVerificationError({
								actual,
								engine_id,
								expected: expected_sha256,
								version,
							});
						yield* Ref.update(verified_executables, (current) =>
							new Map(current).set(executable, {
								expected_sha256,
								fingerprint,
							}),
						);
					}),
				);

			const MakeStatus = (
				distribution: EngineDistribution,
				layout: ToolchainLayout,
				read_options?: EngineToolchainReadOptions,
			) =>
				Effect.gen(function* () {
					const state_result = yield* CachedState(distribution.engine_id, layout).pipe(
						Effect.result,
					);
					const state =
						state_result._tag === "Success" ? state_result.success : undefined;
					const activity =
						state_result._tag === "Failure"
							? ({
									_tag: "failed",
									message: "The toolchain installation record could not be read.",
								} as const)
							: ((yield* Ref.get(activities)).get(distribution.engine_id) ??
								({ _tag: "idle" } as const));
					const latest_version =
						read_options?.check_updates === true
							? yield* LatestVersion(distribution).pipe(
									Effect.orElseSucceed(() => undefined),
								)
							: undefined;
					return {
						...(state === undefined
							? {}
							: {
									active_version: state.active.version,
									executable_path: layout.executable_path(state.active),
								}),
						activity,
						credentials_present: yield* CredentialsPresent(distribution, layout),
						display_name: distribution.display_name,
						engine_id: distribution.engine_id,
						home_path: layout.home_path,
						...(latest_version === undefined ? {} : { latest_version }),
						...(distribution.minimum_version === undefined
							? {}
							: { minimum_version: distribution.minimum_version }),
						...(state?.previous === undefined
							? {}
							: { previous_version: state.previous.version }),
						...(distribution.recommended_version === undefined
							? {}
							: { recommended_version: distribution.recommended_version }),
					} satisfies EngineToolchainStatus;
				});

			/** Marks the engine installing or fails when an install already runs. */
			const AcquireInstallSlot = (engine_id: string) =>
				Ref.modify(activities, (current) => {
					const activity = current.get(engine_id);
					if (activity?._tag === "installing" || activity?._tag === "authenticating")
						return [false, current] as const;
					return [
						true,
						new Map(current).set(engine_id, {
							_tag: "installing",
							phase: "resolving",
						} satisfies EngineToolchainActivity),
					] as const;
				}).pipe(
					Effect.filterOrFail(
						(acquired) => acquired,
						() => new EngineToolchainBusyError({ engine_id }),
					),
					Effect.asVoid,
				);

			const InstallResolved = (
				distribution: EngineDistribution,
				layout: ToolchainLayout,
				version: string,
			) =>
				Effect.gen(function* () {
					const engine_id = distribution.engine_id;
					const Phase = (phase: EngineToolchainInstallPhase, detail?: string) =>
						SetActivity(engine_id, {
							_tag: "installing",
							...(detail === undefined ? {} : { detail }),
							phase,
							version,
						});
					const prior = yield* CachedState(engine_id, layout);
					const rollback_candidate =
						prior?.active.version === version ? prior.previous : prior?.active;
					if (prior?.active.version === version) {
						const executable = layout.executable_path(prior.active);
						const metadata = yield* file_system.stat(executable).pipe(Effect.option);
						if (metadata._tag === "Some" && metadata.value.type === "File") {
							const health = yield* HealthCheck(
								distribution,
								executable,
								version,
								HomeEnvironment(distribution, layout),
							).pipe(Effect.result);
							if (health._tag === "Success") {
								yield* ProvisionHome(distribution, layout);
								return;
							}
						}
						/** Fall through to stage a fresh immutable generation for Repair. */
					}
					const release = yield* distribution
						.ResolveRelease(version, platform)
						.pipe(Effect.provideService(ToolchainReleaseHttp, http));
					const release_version = yield* Schema.decodeUnknownEffect(
						ToolchainSemanticVersion,
					)(release.version).pipe(
						Effect.mapError(
							(cause) =>
								new EngineToolchainStagingError({ cause, engine_id, version }),
						),
					);
					if (release_version !== version)
						return yield* new EngineToolchainStagingError({
							cause: new Error(
								"Release metadata did not resolve the requested version",
							),
							engine_id,
							version,
						});
					yield* ValidateToolchainRelativePath(release.binary).pipe(
						Effect.mapError(
							(cause) =>
								new EngineToolchainStagingError({ cause, engine_id, version }),
						),
					);
					yield* Phase("downloading");
					/** Never overwrite a generation: an active executable may still be running. */
					const generation_directory = `${version}-${randomUUID()}`;
					const version_path = layout.generation_path(generation_directory);
					const staging_generation = path.join(layout.staging_path, randomUUID());
					const staged_file = path.join(staging_generation, release.binary);
					const downloaded_file =
						release.artifact_kind === "npm-tarball"
							? path.join(staging_generation, "package.tgz")
							: release.artifact_kind === "archive-bundle"
								? path.join(staging_generation, "package.zip")
								: release.artifact_kind === "staged-installer"
									? path.join(staging_generation, "installer.ps1")
									: staged_file;
					let actual = "";
					yield* Effect.gen(function* () {
						yield* file_system.makeDirectory(layout.versions_path, { recursive: true });
						yield* file_system.makeDirectory(staging_generation, { recursive: true });
						let downloaded_bytes = 0;
						const hash = createHash(
							release.artifact_kind === "npm-tarball" ? "sha512" : "sha256",
						);
						const maximum_bytes = Math.min(
							release.size_bytes ?? maximum_engine_binary_bytes,
							maximum_engine_binary_bytes,
						);
						yield* http.GetStream(release.url, maximum_bytes).pipe(
							Stream.tap((chunk) =>
								Effect.gen(function* () {
									downloaded_bytes += chunk.byteLength;
									if (downloaded_bytes > maximum_bytes)
										return yield* new ToolchainHttpFailure({
											code: "body_too_large",
											url: release.url,
										});
									hash.update(chunk);
								}),
							),
							Stream.run(file_system.sink(downloaded_file)),
						);
						yield* Phase("verifying");
						if (release.artifact_kind === "staged-installer") {
							const installer_sha256 = hash.digest("hex");
							if (installer_sha256 !== release.installer_sha256)
								return yield* new EngineToolchainVerificationError({
									actual: installer_sha256,
									engine_id,
									expected: release.installer_sha256,
									version,
								});
							const existing = yield* file_system
								.exists(version_path)
								.pipe(Effect.orElseSucceed(() => false));
							if (existing)
								return yield* new EngineToolchainStagingError({
									cause: new Error(`Toolchain version already exists: ${version}`),
									engine_id,
									version,
								});
							yield* file_system.makeDirectory(version_path, { recursive: true });
							const install_directory = path.join(version_path, "hermes-agent");
							/** Seed the owned Hermes profile before its template stage observes it. */
							yield* ProvisionHome(distribution, layout);
							const stage_labels: Readonly<Record<string, string>> = {
								"bootstrap-marker": "Finalizing Hermes installation…",
								"config-templates": "Writing Hermes configuration…",
								dependencies: "Installing Python dependencies…",
								git: "Checking Git…",
								node: "Checking Node.js…",
								"node-deps": "Installing Node.js dependencies…",
								path: "Creating the Hermes launcher…",
								"platform-sdks": "Installing Hermes platform SDKs…",
								python: "Preparing Python…",
								repository: "Downloading Hermes…",
								"system-packages": "Installing system tools…",
								uv: "Preparing the Python package manager…",
								venv: "Creating the Hermes environment…",
							};
							const RunStage = (stage: string) =>
								Effect.scoped(
									Effect.gen(function* () {
										yield* Phase(
											stage === "repository" || stage === "venv" || stage === "dependencies"
												? "staging"
												: "provisioning",
											stage_labels[stage] ?? "Preparing Hermes…",
										);
										const handle = yield* factory.Spawn({
											args: [
												"-NoLogo",
												"-NoProfile",
												"-NonInteractive",
												"-ExecutionPolicy",
												"Bypass",
												"-File",
												downloaded_file,
												"-NonInteractive",
												"-Json",
												"-Stage",
												stage,
												"-SkipSetup",
												"-SkipComputerUse",
												"-HermesHome",
												layout.home_path,
												"-InstallDir",
												install_directory,
												"-Commit",
												release.commit,
												"-ForceCommit",
											],
											command: "powershell.exe",
											cwd: user_home,
											env: { ...process.env },
										});
										const [stdout, stderr, exit] = yield* Effect.all(
											[
												ReadHealthOutput(handle.Stdout, 16 * 1024 * 1024),
												ReadHealthOutput(handle.Stderr, 16 * 1024 * 1024),
												handle.Exit,
											],
											{ concurrency: "unbounded" },
										).pipe(Effect.ensuring(handle.Close));
										if (exit.code !== 0)
											return yield* new EngineToolchainStagingError({
												cause: new Error(
													`Hermes installer stage ${stage} failed: ${(stderr || stdout).trim().slice(-2_048)}`,
												),
												engine_id,
												version,
											});
									}),
								).pipe(
									Effect.timeoutOrElse({
										duration: "20 minutes",
										orElse: () =>
											Effect.fail(
												new EngineToolchainStagingError({
													cause: new Error(`Hermes installer stage ${stage} timed out`),
													engine_id,
													version,
												}),
											),
									}),
								);
							yield* Effect.forEach(release.stages, RunStage, {
								concurrency: 1,
								discard: true,
							}).pipe(
								Effect.catch((failure) =>
									file_system
										.remove(version_path, { recursive: true })
										.pipe(Effect.ignore, Effect.andThen(Effect.fail(failure))),
								),
							);
							actual = yield* HashFile(file_system, path.join(version_path, release.binary));
						} else if (release.artifact_kind === "npm-tarball") {
							const archive_integrity = hash.digest("base64");
							if (archive_integrity !== release.integrity_sha512)
								return yield* new EngineToolchainVerificationError({
									actual: `sha512-${archive_integrity}`,
									engine_id,
									expected: `sha512-${release.integrity_sha512}`,
									version,
								});
							const archive = yield* file_system.readFile(downloaded_file);
							const executable = yield* Effect.try({
								try: () =>
									ExtractNpmTarballExecutable(
										archive,
										release.archive_member,
										maximum_engine_binary_bytes,
									),
								catch: (cause) => cause,
							});
							actual = createHash("sha256").update(executable).digest("hex");
							yield* file_system.writeFile(staged_file, executable);
							yield* file_system.remove(downloaded_file);
						} else if (release.artifact_kind === "archive-bundle") {
							const archive_sha256 = hash.digest("hex");
							if (archive_sha256 !== release.sha256)
								return yield* new EngineToolchainVerificationError({
									actual: archive_sha256,
									engine_id,
									expected: release.sha256,
									version,
								});
							yield* Effect.tryPromise({
								try: () =>
									ExtractZipBundle({
										archive_path: downloaded_file,
										archive_root: release.archive_root,
										destination: staging_generation,
										maximum_output_bytes: release.expanded_size_bytes,
									}),
								catch: (cause) => cause,
							});
							yield* file_system.remove(downloaded_file);
							actual = yield* HashFile(file_system, staged_file);
						} else {
							actual = hash.digest("hex");
							if (actual !== release.sha256)
								return yield* new EngineToolchainVerificationError({
									actual,
									engine_id,
									expected: release.sha256,
									version,
								});
						}
						if (release.artifact_kind !== "staged-installer") {
							yield* Phase("staging");
							if (platform.platform !== "win32")
								yield* file_system.chmod(staged_file, 0o755);
							const existing = yield* file_system
								.exists(version_path)
								.pipe(Effect.orElseSucceed(() => false));
							if (existing)
								return yield* new EngineToolchainStagingError({
									cause: new Error(`Toolchain version already exists: ${version}`),
									engine_id,
									version,
								});
							yield* file_system.rename(staging_generation, version_path);
						}
					}).pipe(
						Effect.mapError((cause) => AsToolchainFailure(cause, engine_id, version)),
						Effect.tapError(() =>
							release.artifact_kind === "staged-installer"
								? file_system.remove(version_path, { recursive: true }).pipe(Effect.ignore)
								: Effect.void,
						),
						Effect.ensuring(
							file_system
								.remove(staging_generation, { recursive: true })
								.pipe(Effect.ignore),
						),
					);
					yield* Phase("checking");
					const metadata = yield* file_system
						.stat(path.join(version_path, release.binary))
						.pipe(
							Effect.mapError(
								(cause) =>
									new EngineToolchainStagingError({ cause, engine_id, version }),
							),
						);
					if (metadata.type !== "File")
						return yield* new EngineToolchainStagingError({
							cause: new Error("Staged engine executable is not a regular file"),
							engine_id,
							version,
						});
					yield* HealthCheck(
						distribution,
						path.join(version_path, release.binary),
						version,
						HomeEnvironment(distribution, layout),
					).pipe(
						Effect.catch((failure) =>
							file_system
								.remove(version_path, { recursive: true })
								.pipe(Effect.ignore, Effect.andThen(Effect.fail(failure))),
						),
					);
					yield* Phase("provisioning");
					yield* ProvisionHome(distribution, layout);
					const next: ToolchainInstallationState = {
						active: {
							binary: release.binary,
							directory: generation_directory,
							sha256: actual,
							version,
						},
						format_version: 1,
						...(rollback_candidate === undefined
							? {}
							: { previous: rollback_candidate }),
					};
					yield* WriteToolchainState(layout, engine_id, next, file_system);
					yield* RememberState(engine_id, next);
					yield* PruneToolchainVersions(layout, next, file_system, path);
				});

			const InstallAfterAcquire = (
				distribution: EngineDistribution,
				layout: ToolchainLayout,
				version?: string,
			) =>
				Effect.gen(function* () {
					const engine_id = distribution.engine_id;
					yield* Effect.gen(function* () {
						const resolved_version =
							version === undefined
								? (distribution.recommended_version ??
									(yield* LatestVersion(distribution)))
								: yield* Schema.decodeUnknownEffect(ToolchainSemanticVersion)(
										version,
									).pipe(
										Effect.mapError(
											() =>
												new EngineToolchainStagingError({
													cause: new Error(
														`"${version}" is not a semantic version`,
													),
													engine_id,
													version,
												}),
										),
									);
						yield* SetActivity(engine_id, {
							_tag: "installing",
							phase: "resolving",
							version: resolved_version,
						});
						yield* InstallResolved(distribution, layout, resolved_version);
						yield* SetActivity(engine_id, { _tag: "idle" });
					}).pipe(
						Effect.mapError((failure) =>
							AsToolchainFailure(failure, engine_id, version ?? "install"),
						),
						Effect.tapError((failure) =>
							SetActivity(engine_id, {
								_tag: "failed",
								message: describe_engine_toolchain_failure(failure),
							}),
						),
						Effect.onInterrupt(() => SetActivity(engine_id, { _tag: "idle" })),
					);
					return yield* MakeStatus(distribution, layout);
				});

			const Install = (engine_id: string, version?: string) =>
				Effect.gen(function* () {
					const distribution = yield* Distribution(engine_id);
					yield* AcquireInstallSlot(engine_id);
					return yield* InstallAfterAcquire(distribution, Layout(engine_id), version);
				});

			const StartInstall = (engine_id: string, version?: string) =>
				Effect.gen(function* () {
					const distribution = yield* Distribution(engine_id);
					const layout = Layout(engine_id);
					yield* AcquireInstallSlot(engine_id);
					const Run = InstallAfterAcquire(distribution, layout, version).pipe(
						Effect.catch(() => Effect.void),
						Effect.onInterrupt(() => SetActivity(engine_id, { _tag: "idle" })),
						Effect.catchCause(() =>
							SetActivity(engine_id, {
								_tag: "failed",
								message: "The managed engine install stopped unexpectedly.",
							}),
						),
					);
					yield* Effect.forkIn(Run, service_scope);
					return yield* MakeStatus(distribution, layout);
				});

			const Rollback = (engine_id: string) =>
				Effect.gen(function* () {
					const distribution = yield* Distribution(engine_id);
					const layout = Layout(engine_id);
					yield* AcquireInstallSlot(engine_id);
					yield* Effect.gen(function* () {
						const state = yield* CachedState(engine_id, layout);
						if (state?.previous === undefined)
							return yield* new EngineToolchainRollbackUnavailableError({
								engine_id,
							});
						const restored = state.previous;
						const restored_path = layout.executable_path(restored);
						const restored_exists = yield* file_system
							.exists(restored_path)
							.pipe(Effect.orElseSucceed(() => false));
						if (!restored_exists)
							return yield* new EngineToolchainRollbackUnavailableError({
								engine_id,
							});
						const actual = yield* HashFile(file_system, restored_path).pipe(
							Effect.mapError(
								(cause) =>
									new EngineToolchainStagingError({
										cause,
										engine_id,
										version: restored.version,
									}),
							),
						);
						if (actual !== restored.sha256)
							return yield* new EngineToolchainVerificationError({
								actual,
								engine_id,
								expected: restored.sha256,
								version: restored.version,
							});
						yield* HealthCheck(
							distribution,
							restored_path,
							restored.version,
							HomeEnvironment(distribution, layout),
						);
						const next: ToolchainInstallationState = {
							active: restored,
							format_version: 1,
							previous: state.active,
						};
						yield* WriteToolchainState(layout, engine_id, next, file_system);
						yield* RememberState(engine_id, next);
						yield* SetActivity(engine_id, { _tag: "idle" });
					}).pipe(
						Effect.tapError((failure) =>
							SetActivity(engine_id, {
								_tag: "failed",
								message: describe_engine_toolchain_failure(failure),
							}),
						),
						Effect.onInterrupt(() => SetActivity(engine_id, { _tag: "idle" })),
					);
					return yield* MakeStatus(distribution, layout);
				});

			const Status = (engine_id: string, read_options?: EngineToolchainReadOptions) =>
				Effect.gen(function* () {
					const distribution = yield* Distribution(engine_id);
					return yield* MakeStatus(distribution, Layout(engine_id), read_options);
				});

			const List = (read_options?: EngineToolchainReadOptions) =>
				Effect.forEach(distributions, (distribution) =>
					MakeStatus(distribution, Layout(distribution.engine_id), read_options),
				);

			const ResolveSpawn = (engine_id: string, profile_id?: string) =>
				Effect.gen(function* () {
					const distribution = yield* Distribution(engine_id);
					const layout = Layout(engine_id, profile_id);
					const state = yield* CachedState(engine_id, layout).pipe(
						Effect.mapError(
							() =>
								new EngineToolchainManagedInstallMissingError({
									engine_id,
									reason: "invalid_state",
								}),
						),
					);
					if (state === undefined)
						return yield* new EngineToolchainManagedInstallMissingError({
							engine_id,
							reason: "not_installed",
						});
					const executable = layout.executable_path(state.active);
					const metadata = yield* file_system.stat(executable).pipe(Effect.option);
					if (metadata._tag !== "Some" || metadata.value.type !== "File")
						return yield* new EngineToolchainManagedInstallMissingError({
							engine_id,
							reason: "missing_binary",
						});
					yield* VerifyExecutable(
						engine_id,
						executable,
						state.active.sha256,
						state.active.version,
					);
					return {
						environment: HomeEnvironment(distribution, layout),
						executable,
					} satisfies EngineSpawnOverride;
				}).pipe(
					Effect.tapError((failure) =>
						SetActivity(engine_id, {
							_tag: "failed",
							message: describe_engine_toolchain_failure(failure),
						}),
					),
				);

			/** Runs the vendor browser-login flow without retaining any provider output. */
			const StartAuthentication = (engine_id: string, profile_id?: string) =>
				Effect.gen(function* () {
					const distribution = yield* Distribution(engine_id);
					if (distribution.authentication === "service-api")
						return yield* new EngineToolchainAuthenticationError({
							engine_id,
							message:
								"This engine authenticates through its local service integration API, not a CLI login command.",
						});
					const layout = Layout(engine_id, profile_id);
					yield* AcquireInstallSlot(engine_id);
					yield* SetActivity(engine_id, { _tag: "authenticating" });
					/**
					 * An added profile has no home until its first sign-in, so create
					 * it here rather than at install time; the vendor CLI writes its
					 * credential into a directory that must already be owner-only.
					 */
					yield* EnsureHome(layout).pipe(
						Effect.mapError(
							(cause) =>
								new EngineToolchainAuthenticationError({
									engine_id,
									message: `The profile home could not be created: ${String(cause)}`,
								}),
						),
					);
					const Authenticate = Effect.scoped(
						Effect.gen(function* () {
							const override = yield* ResolveSpawn(engine_id, profile_id);
							const handle = yield* factory.Spawn({
								args: distribution.login_args,
								command: override.executable,
								env: { ...process.env, ...override.environment },
							});
							const [, , exit] = yield* Effect.all(
								[
									ReadHealthOutput(handle.Stdout, 1_048_576),
									ReadHealthOutput(handle.Stderr, 1_048_576),
									handle.Exit,
								],
								{ concurrency: "unbounded" },
							).pipe(Effect.ensuring(handle.Close));
							if (exit.code !== 0)
								return yield* new EngineToolchainAuthenticationError({
									engine_id,
									message: "The vendor login command exited unsuccessfully.",
								});
						}),
					).pipe(
						Effect.tap(() => SetActivity(engine_id, { _tag: "idle" })),
						Effect.timeoutOrElse({
							duration: 600_000,
							orElse: () =>
								Effect.fail(
									new EngineToolchainAuthenticationError({
										engine_id,
										message: "The vendor login command timed out.",
									}),
								),
						}),
						Effect.catch(() =>
							SetActivity(engine_id, {
								_tag: "failed",
								message: "The managed engine sign-in did not complete.",
							}),
						),
						Effect.onInterrupt(() => SetActivity(engine_id, { _tag: "idle" })),
						Effect.catchCause(() =>
							SetActivity(engine_id, {
								_tag: "failed",
								message: "The managed engine sign-in stopped unexpectedly.",
							}),
						),
					);
					yield* Effect.forkIn(Authenticate, service_scope);
					return yield* MakeStatus(distribution, layout);
				});

			return EngineToolchain.of({
				Install,
				List,
				ResolveSpawn,
				Rollback,
				StartAuthentication,
				StartInstall,
				Status,
			});
		}),
	);

/** Composition default for runtimes that do not manage engine binaries. */
export const EngineToolchainDisabled = Layer.succeed(
	EngineToolchain,
	EngineToolchain.of({
		Install: (engine_id) => Effect.fail(new EngineToolchainUnavailableError({ engine_id })),
		List: () => Effect.succeed([]),
		ResolveSpawn: (engine_id) =>
			Effect.fail(new EngineToolchainUnavailableError({ engine_id })),
		Rollback: (engine_id) => Effect.fail(new EngineToolchainUnavailableError({ engine_id })),
		StartInstall: (engine_id) =>
			Effect.fail(new EngineToolchainUnavailableError({ engine_id })),
		StartAuthentication: (engine_id) =>
			Effect.fail(new EngineToolchainUnavailableError({ engine_id })),
		Status: (engine_id) => Effect.fail(new EngineToolchainUnavailableError({ engine_id })),
	}),
);
