import { spawn } from "node:child_process";
import { access, mkdir, open } from "node:fs/promises";
import { delimiter, dirname, join } from "node:path";

import { MakeSnowflakeIdLive } from "@artisan/protocol";
import { Clock, Context, Effect, Layer } from "effect";
import { ChildProcess } from "effect/unstable/process";
import { decode_forge_config, RemoveForgeState, StartForge, WriteForgeState } from "@artisan/forge";

import { ResolveForgeArtifact } from "./forge-adapter";
import { ForgeLauncher, ForgeLifecycleError } from "./lifecycle";
import { ForgeProfileStore } from "./profile";
import type { ForgeRuntimeState } from "./profile";

interface ForgeLauncherEnvironmentValue {
	readonly inherited: Readonly<Record<string, string | undefined>>;
	readonly node_path: string | undefined;
}

class ForgeLauncherEnvironment extends Context.Service<
	ForgeLauncherEnvironment,
	ForgeLauncherEnvironmentValue
>()("Artisan/ForgeLauncherEnvironment") {}

/** Confines inheritance of the parent process environment to the Node adapter. */
const NodeForgeLauncherEnvironmentLive = Layer.succeed(
	ForgeLauncherEnvironment,
	ForgeLauncherEnvironment.of({
		inherited: { ...process.env },
		node_path: process.env.NODE_PATH,
	}),
);

/** Reclaims state only when the recorded PID is definitely no longer alive. */
export const VerifyStoppedForgeProcess = (
	state: ForgeRuntimeState,
): Effect.Effect<void, ForgeLifecycleError> =>
	Effect.try({
		try: () => {
			try {
				process.kill(state.pid, 0);
			} catch (cause) {
				if ((cause as NodeJS.ErrnoException).code === "ESRCH") return;
				throw new ForgeLifecycleError({ cause, code: "ownership" });
			}
			throw new ForgeLifecycleError({ code: "ownership" });
		},
		catch: (cause) =>
			cause instanceof ForgeLifecycleError
				? cause
				: new ForgeLifecycleError({ cause, code: "ownership" }),
	});

/**
 * Process ownership is a narrow adapter. `ChildProcess` remains the command
 * vocabulary for Effect composition; Node's detached/unref API is used here
 * because the current unstable process API deliberately does not expose the
 * Windows-specific unref and inherited-stdio controls required by Forge.
 */
export const make_node_forge_launcher_layer = Layer.effect(
	ForgeLauncher,
	Effect.gen(function* () {
		const store = yield* ForgeProfileStore;
		const environment = yield* ForgeLauncherEnvironment;
		const artifact = yield* ResolveForgeArtifact;
		const Launch = (
			input: {
				readonly config: {
					readonly data_root: string;
					readonly listen_host: "127.0.0.1" | "::1";
					readonly listen_port: number;
					readonly project_roots: ReadonlyArray<string>;
				};
				readonly profile: string;
				readonly token: string;
				readonly instance_id: string;
			},
			foreground: boolean,
		) =>
			Effect.gen(function* () {
				const paths = yield* store
					.Paths(input.profile)
					.pipe(
						Effect.mapError(
							(cause) => new ForgeLifecycleError({ cause, code: "timeout" }),
						),
					);
				yield* Effect.forEach(
					[
						artifact.executable_path,
						artifact.host_entry_path,
						artifact.migrations_path,
						artifact.static_frontend_root,
						artifact.native_runtime_path,
						artifact.node_executable_path,
						artifact.windows_process_host_path,
					],
					(path) =>
						Effect.tryPromise({
							try: () => access(path),
							catch: (cause) => new ForgeLifecycleError({ cause, code: "timeout" }),
						}),
					{ concurrency: "unbounded", discard: true },
				);
				yield* Effect.tryPromise({
					try: () => mkdir(dirname(paths.log_path), { recursive: true, mode: 0o700 }),
					catch: (cause) => new ForgeLifecycleError({ cause, code: "timeout" }),
				});
				const log = foreground
					? undefined
					: yield* Effect.tryPromise({
							try: () => open(paths.log_path, "a", 0o600),
							catch: (cause) => new ForgeLifecycleError({ cause, code: "timeout" }),
						});
				const env = {
					...environment.inherited,
					ARTISAN_AUTH_TOKEN: input.token,
					ARTISAN_DATABASE_PATH: join(input.config.data_root, "artisan.sqlite"),
					ARTISAN_MIGRATIONS_PATH: artifact.migrations_path,
					ARTISAN_STATIC_FRONTEND_ROOT: artifact.static_frontend_root,
					ARTISAN_NODE_EXECUTABLE: artifact.node_executable_path,
					ARTISAN_NATIVE_RUNTIME: artifact.native_runtime_path,
					NODE_PATH: [artifact.native_runtime_path, environment.node_path]
						.filter((value): value is string => value !== undefined && value !== "")
						.join(delimiter),
					ARTISAN_FORGE_INSTANCE_ID: input.instance_id,
					ARTISAN_FORGE_PROFILE: input.profile,
					ARTISAN_FORGE_STATE_PATH: paths.state_path,
					ARTISAN_LISTEN_HOST: input.config.listen_host,
					ARTISAN_LISTEN_PORT: String(input.config.listen_port),
					ARTISAN_PROJECT_ROOTS: JSON.stringify(input.config.project_roots),
					ARTISAN_WINDOWS_PROCESS_HOST: artifact.windows_process_host_path,
				};
				// Build an argv-only Effect process command for traceability; spawn supplies
				// the platform-specific detached/file-descriptor controls below.
				void ChildProcess.make(artifact.executable_path, [artifact.host_entry_path]);
				const child = spawn(artifact.executable_path, [artifact.host_entry_path], {
					detached: !foreground,
					env,
					stdio: foreground ? "inherit" : ["ignore", log!.fd, log!.fd],
					windowsHide: !foreground,
				});
				if (!foreground) {
					child.unref();
					yield* Effect.promise(() => log!.close());
					return;
				}
				yield* Effect.callback<void, ForgeLifecycleError>((resume) => {
					const on_error = (cause: Error) =>
						resume(Effect.fail(new ForgeLifecycleError({ cause, code: "timeout" })));
					const on_exit = (code: number | null) =>
						resume(
							code === 0
								? Effect.void
								: Effect.fail(
										new ForgeLifecycleError({
											cause: new Error(`Forge exited ${String(code)}`),
											code: "timeout",
										}),
									),
						);
					child.once("error", on_error);
					child.once("exit", on_exit);
					return Effect.sync(() => {
						child.off("error", on_error);
						child.off("exit", on_exit);
						child.kill();
					});
				}).pipe(
					Effect.mapError((cause) =>
						cause instanceof ForgeLifecycleError
							? cause
							: new ForgeLifecycleError({ cause, code: "timeout" }),
					),
				);
			});
		const StartForeground = (input: {
			readonly config: {
				readonly data_root: string;
				readonly listen_host: "127.0.0.1" | "::1";
				readonly listen_port: number;
				readonly project_roots: ReadonlyArray<string>;
			};
			readonly instance_id: string;
			readonly profile: string;
			readonly token: string;
		}) =>
			Effect.scoped(
				Effect.gen(function* () {
					const paths = yield* store
						.Paths(input.profile)
						.pipe(
							Effect.mapError(
								(cause) => new ForgeLifecycleError({ cause, code: "timeout" }),
							),
						);
					const host = yield* StartForge(
						decode_forge_config({
							auth_token: input.token,
							database_path: join(input.config.data_root, "artisan.sqlite"),
							instance_id: input.instance_id,
							listen_host: input.config.listen_host,
							listen_port: input.config.listen_port,
							migrations_path: artifact.migrations_path,
							project_directory_roots: input.config.project_roots,
							static_frontend_root: artifact.static_frontend_root,
						}),
					).pipe(
						Effect.mapError(
							(cause) => new ForgeLifecycleError({ cause, code: "timeout" }),
						),
					);
					const now = yield* Clock.currentTimeMillis;
					yield* WriteForgeState(paths.state_path, {
						endpoint: host.endpoint.toString(),
						instance_id: input.instance_id,
						pid: process.pid,
						profile: input.profile,
						started_at: new Date(now).toISOString(),
						version: 1,
					}).pipe(
						Effect.provide(MakeSnowflakeIdLive(2).pipe(Layer.orDie)),
						Effect.mapError((cause) =>
							cause instanceof ForgeLifecycleError
								? cause
								: new ForgeLifecycleError({ cause, code: "timeout" }),
						),
					);
					yield* host.ShutdownRequested.pipe(
						Effect.mapError(
							(cause) => new ForgeLifecycleError({ cause, code: "timeout" }),
						),
						Effect.ensuring(
							RemoveForgeState(paths.state_path, input.instance_id).pipe(
								Effect.ignore,
							),
						),
					);
				}),
			);
		return ForgeLauncher.of({
			StartBackground: (input) => Launch(input, false),
			StartForeground,
			TerminateVerified: VerifyStoppedForgeProcess,
		});
	}),
).pipe(Layer.provide(NodeForgeLauncherEnvironmentLive));
