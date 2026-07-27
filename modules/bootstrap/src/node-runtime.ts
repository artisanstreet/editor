import { spawn } from "node:child_process";
import { dirname, extname, isAbsolute, posix, resolve, win32 } from "node:path";

import { NodeChildProcessSpawner, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, Schema } from "effect";

import { make_installation_store_layer } from "@artisan/distribution";

import {
	BootstrapCleanup,
	BootstrapCleanupFailure,
	BootstrapInstallFailure,
	BootstrapInstaller,
	NpmCleanupPlan,
	PermanentAe,
	PermanentAeCommandResult,
	PermanentAeFailure,
	type BootstrapInvocation,
} from "./contract";
import { make_node_bootstrap_installer_layer } from "./installer-runtime";

const cleanup_flag = "--artisan-cleanup";
const permanent_ae_output_limit = 64 * 1_024;
const permanent_ae_timeout_ms = 30_000;

const PlatformLive = Layer.mergeAll(
	NodeFileSystem.layer,
	NodePath.layer,
	NodeChildProcessSpawner.layer.pipe(
		Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
	),
);

const IsProcessAlive = (process_id: number) =>
	Effect.sync(() => {
		try {
			process.kill(process_id, 0);
			return true;
		} catch {
			return false;
		}
	});

const AwaitProcessExit = (process_id: number): Effect.Effect<void> =>
	IsProcessAlive(process_id).pipe(
		Effect.flatMap((alive) =>
			alive
				? Effect.sleep("250 millis").pipe(
						Effect.flatMap(() => AwaitProcessExit(process_id)),
					)
				: Effect.void,
		),
	);

const RunCleanupCommand = (plan: NpmCleanupPlan) =>
	Effect.callback<number, BootstrapCleanupFailure>((resume) => {
		let executable = plan.executable;
		let command_argv = [...plan.argv];
		let windows_verbatim_arguments = false;
		try {
			if (
				process.platform === "win32" &&
				[".bat", ".cmd"].includes(extname(plan.executable).toLowerCase())
			) {
				const EscapeArgument = (argument: string) => {
					if (
						argument.includes("\u0000") ||
						argument.includes("\r") ||
						argument.includes("\n")
					)
						throw new Error("Cleanup arguments may not contain control lines");
					return `"${argument.replaceAll("%", "%%").replaceAll('"', '""')}"`;
				};
				executable = process.env.ComSpec ?? "C:\\Windows\\System32\\cmd.exe";
				command_argv = [
					"/d",
					"/s",
					"/c",
					`""${plan.executable}" ${plan.argv.map(EscapeArgument).join(" ")}"`,
				];
				windows_verbatim_arguments = true;
			}
			const child = spawn(executable, command_argv, {
				shell: false,
				stdio: ["ignore", "inherit", "inherit"],
				windowsHide: true,
				windowsVerbatimArguments: windows_verbatim_arguments,
			});
			child.once("error", (cause) =>
				resume(
					Effect.fail(
						new BootstrapCleanupFailure({
							cause,
							plan,
						}),
					),
				),
			);
			child.once("close", (exit_code) => resume(Effect.succeed(exit_code ?? -1)));
			return Effect.sync(() => {
				if (child.exitCode === null) KillProcessTree(child);
			});
		} catch (cause) {
			resume(
				Effect.fail(
					new BootstrapCleanupFailure({
						cause,
						plan,
					}),
				),
			);
		}
	});

/** Executes only inside the detached cleanup helper after the bootstrap exits. */
export const RunDetachedCleanup = (input: unknown) =>
	Effect.gen(function* () {
		const plan = yield* Schema.decodeUnknownEffect(NpmCleanupPlan)(input).pipe(
			Effect.mapError(
				(cause) =>
					new BootstrapCleanupFailure({
						cause,
						plan: input as NpmCleanupPlan,
					}),
			),
		);
		yield* AwaitProcessExit(plan.bootstrap_pid);
		const exit_code = yield* RunCleanupCommand(plan);
		if (exit_code !== 0) {
			return yield* new BootstrapCleanupFailure({
				cause: new Error(`npm cleanup exited with code ${exit_code}`),
				plan,
			});
		}
	}).pipe(Effect.provide(PlatformLive));

const DecodeCleanupPlan = (encoded: string) =>
	Effect.try({
		try: () => JSON.parse(Buffer.from(encoded, "base64url").toString("utf8")) as unknown,
		catch: (cause) => cause,
	}).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(NpmCleanupPlan)),
		Effect.mapError(
			(cause) =>
				new BootstrapCleanupFailure({
					cause,
					plan: {
						executable: process.execPath,
						argv: [
							"uninstall",
							"--global",
							"--prefix",
							resolve(dirname(dirname(dirname(import.meta.filename)))),
							"artisan-editor",
						],
						bootstrap_pid: process.pid,
						manual_command:
							"npm uninstall --global --prefix <installation-prefix> artisan-editor",
					},
				}),
		),
	);

export const ParseDetachedCleanup = (argv: ReadonlyArray<string>) =>
	Effect.gen(function* () {
		const encoded_plan = argv[0] === cleanup_flag ? argv[1] : undefined;
		if (encoded_plan === undefined) return { _tag: "Bootstrap" } as const;
		const plan = yield* DecodeCleanupPlan(encoded_plan);
		return { _tag: "Cleanup", plan } as const;
	});

const KillProcessTree = (child: ReturnType<typeof spawn>) => {
	if (child.pid !== undefined && process.platform === "win32") {
		const killer = spawn("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], {
			shell: false,
			stdio: "ignore",
			windowsHide: true,
		});
		killer.once("error", () => child.kill());
		killer.unref();
		return;
	}
	child.kill();
};

/** Checks and invokes the installed CLI using an absolute executable path and argv array. */
export const make_node_permanent_ae_layer_with_timeout = (timeout_ms = permanent_ae_timeout_ms) =>
	Layer.effect(
		PermanentAe,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const ExecuteCaptured = (
				permanent_ae_path: string,
				operation: PermanentAeFailure["operation"],
				argv: ReadonlyArray<string>,
			) =>
				Effect.callback<PermanentAeCommandResult, PermanentAeFailure>((resume) => {
					let executable = permanent_ae_path;
					let command_argv = [...argv];
					let windows_verbatim_arguments = false;
					try {
						if ([".bat", ".cmd"].includes(extname(permanent_ae_path).toLowerCase())) {
							const EscapeArgument = (argument: string) => {
								if (
									argument.includes("\u0000") ||
									argument.includes("\r") ||
									argument.includes("\n")
								)
									throw new Error(
										"Permanent ae arguments may not contain control lines",
									);
								return `"${argument.replaceAll("%", "%%").replaceAll('"', '""')}"`;
							};
							executable = process.env.ComSpec ?? "C:\\Windows\\System32\\cmd.exe";
							command_argv = [
								"/d",
								"/s",
								"/c",
								`""${permanent_ae_path}" ${argv.map(EscapeArgument).join(" ")}"`,
							];
							windows_verbatim_arguments = true;
						}
						const child = spawn(executable, command_argv, {
							shell: false,
							stdio: ["ignore", "pipe", "pipe"],
							windowsHide: true,
							windowsVerbatimArguments: windows_verbatim_arguments,
						});
						const stdout_chunks: Array<Buffer> = [];
						const stderr_chunks: Array<Buffer> = [];
						let stdout_bytes = 0;
						let stderr_bytes = 0;
						let stdout_retained_bytes = 0;
						let stderr_retained_bytes = 0;
						let completed = false;
						const timer = setTimeout(() => {
							if (completed) return;
							completed = true;
							KillProcessTree(child);
							resume(
								Effect.fail(
									new PermanentAeFailure({
										cause: new Error(
											`Permanent ae ${operation} timed out after ${timeout_ms}ms`,
										),
										operation,
										permanent_ae_path,
									}),
								),
							);
						}, timeout_ms);
						timer.unref();
						const Append = (
							chunks: Array<Buffer>,
							retained_bytes: number,
							chunk: Buffer,
						) => {
							const remaining = permanent_ae_output_limit - retained_bytes;
							const retained =
								remaining > 0 ? chunk.subarray(0, remaining) : undefined;
							if (retained !== undefined) chunks.push(retained);
							return retained_bytes + (retained?.byteLength ?? 0);
						};
						child.stdout.on("data", (chunk: Buffer) => {
							stdout_bytes += chunk.byteLength;
							stdout_retained_bytes = Append(
								stdout_chunks,
								stdout_retained_bytes,
								chunk,
							);
						});
						child.stderr.on("data", (chunk: Buffer) => {
							stderr_bytes += chunk.byteLength;
							stderr_retained_bytes = Append(
								stderr_chunks,
								stderr_retained_bytes,
								chunk,
							);
						});
						child.once("error", (cause) => {
							if (completed) return;
							completed = true;
							clearTimeout(timer);
							resume(
								Effect.fail(
									new PermanentAeFailure({
										cause,
										operation,
										permanent_ae_path,
									}),
								),
							);
						});
						child.once("close", (exit_code) => {
							if (completed) return;
							completed = true;
							clearTimeout(timer);
							resume(
								Schema.decodeUnknownEffect(PermanentAeCommandResult)({
									exit_code: exit_code ?? -1,
									stdout: Buffer.concat(stdout_chunks).toString("utf8"),
									stderr: Buffer.concat(stderr_chunks).toString("utf8"),
									stdout_truncated: stdout_bytes > permanent_ae_output_limit,
									stderr_truncated: stderr_bytes > permanent_ae_output_limit,
								}).pipe(
									Effect.mapError(
										(cause) =>
											new PermanentAeFailure({
												cause,
												operation,
												permanent_ae_path,
											}),
									),
								),
							);
						});
						return Effect.sync(() => {
							if (completed) return;
							completed = true;
							clearTimeout(timer);
							if (child.exitCode === null) KillProcessTree(child);
						});
					} catch (cause) {
						resume(
							Effect.fail(
								new PermanentAeFailure({
									cause,
									operation,
									permanent_ae_path,
								}),
							),
						);
					}
				});
			const ExecuteInherited = (permanent_ae_path: string, argv: ReadonlyArray<string>) =>
				Effect.callback<number, PermanentAeFailure>((resume) => {
					let executable = permanent_ae_path;
					let command_argv = [...argv];
					let windows_verbatim_arguments = false;
					try {
						if ([".bat", ".cmd"].includes(extname(permanent_ae_path).toLowerCase())) {
							const EscapeArgument = (argument: string) => {
								if (
									argument.includes("\u0000") ||
									argument.includes("\r") ||
									argument.includes("\n")
								)
									throw new Error(
										"Permanent ae arguments may not contain control lines",
									);
								return `"${argument.replaceAll("%", "%%").replaceAll('"', '""')}"`;
							};
							executable = process.env.ComSpec ?? "C:\\Windows\\System32\\cmd.exe";
							command_argv = [
								"/d",
								"/s",
								"/c",
								`""${permanent_ae_path}" ${argv.map(EscapeArgument).join(" ")}"`,
							];
							windows_verbatim_arguments = true;
						}
						const child = spawn(executable, command_argv, {
							shell: false,
							stdio: "inherit",
							windowsHide: true,
							windowsVerbatimArguments: windows_verbatim_arguments,
						});
						let completed = false;
						child.once("error", (cause) => {
							if (completed) return;
							completed = true;
							resume(
								Effect.fail(
									new PermanentAeFailure({
										cause,
										operation: "delegate",
										permanent_ae_path,
									}),
								),
							);
						});
						child.once("close", (exit_code) => {
							if (completed) return;
							completed = true;
							resume(Effect.succeed(exit_code ?? -1));
						});
						return Effect.sync(() => {
							if (!completed && child.exitCode === null) KillProcessTree(child);
						});
					} catch (cause) {
						resume(
							Effect.fail(
								new PermanentAeFailure({
									cause,
									operation: "delegate",
									permanent_ae_path,
								}),
							),
						);
					}
				});
			return PermanentAe.of({
				VerifyHandoff: (permanent_ae_path) =>
					Effect.gen(function* () {
						if (!isAbsolute(permanent_ae_path)) {
							return yield* new PermanentAeFailure({
								cause: new Error("Permanent ae path is not absolute"),
								operation: "verify",
								permanent_ae_path,
							});
						}
						const metadata = yield* file_system.stat(permanent_ae_path).pipe(
							Effect.mapError(
								(cause) =>
									new PermanentAeFailure({
										cause,
										operation: "verify",
										permanent_ae_path,
									}),
							),
						);
						if (metadata.type !== "File") {
							return yield* new PermanentAeFailure({
								cause: new Error("Permanent ae handoff is not a file"),
								operation: "verify",
								permanent_ae_path,
							});
						}
						const result = yield* ExecuteCaptured(permanent_ae_path, "verify", [
							"--version",
						]);
						if (result.exit_code !== 0) {
							return yield* new PermanentAeFailure({
								cause: new Error(
									`Permanent ae health check exited with code ${result.exit_code}`,
								),
								operation: "verify",
								permanent_ae_path,
							});
						}
					}),
				Delegate: (permanent_ae_path, argv) => ExecuteInherited(permanent_ae_path, argv),
				Execute: (permanent_ae_path, operation, argv) =>
					ExecuteCaptured(permanent_ae_path, operation, argv),
			});
		}),
	).pipe(Layer.provide(PlatformLive));

export const make_node_permanent_ae_layer = make_node_permanent_ae_layer_with_timeout();

/** Starts a detached Node helper with explicit argv; no shell is involved. */
export const make_node_cleanup_layer = (entry_path: string) =>
	Layer.succeed(
		BootstrapCleanup,
		BootstrapCleanup.of({
			ScheduleDetached: (plan) =>
				Effect.try({
					try: () => {
						const encoded = Buffer.from(JSON.stringify(plan), "utf8").toString(
							"base64url",
						);
						const child = spawn(process.execPath, [entry_path, cleanup_flag, encoded], {
							detached: true,
							shell: false,
							stdio: "ignore",
							windowsHide: true,
						});
						child.unref();
					},
					catch: (cause) => new BootstrapCleanupFailure({ cause, plan }),
				}),
		}),
	);

/** Publication configuration is deliberately required before installation can begin. */
export const BootstrapInstallerUnavailableLive = Layer.succeed(
	BootstrapInstaller,
	BootstrapInstaller.of({
		InstallFirstTime: (_invocation: BootstrapInvocation) =>
			Effect.fail(
				new BootstrapInstallFailure({
					cause: new Error(
						"No Artisan release endpoint and trusted signing key are configured",
					),
					operation: "install",
				}),
			),
		Resume: () =>
			Effect.fail(
				new BootstrapInstallFailure({
					cause: new Error(
						"No Artisan release endpoint and trusted signing key are configured",
					),
					operation: "resume",
				}),
			),
	}),
);

export const ResolveInstallationRoot = (
	environment: NodeJS.ProcessEnv = process.env,
	platform: NodeJS.Platform = process.platform,
) => {
	const configured_root = environment.ARTISAN_HOME;
	const path_service = platform === "win32" ? win32 : posix;
	if (configured_root !== undefined && path_service.isAbsolute(configured_root)) {
		return configured_root;
	}
	if (platform === "win32") {
		return win32.resolve(
			environment.LOCALAPPDATA ?? environment.USERPROFILE ?? process.cwd(),
			"Artisan",
		);
	}
	return posix.resolve(
		environment.XDG_DATA_HOME ??
			posix.resolve(environment.HOME ?? process.cwd(), ".local", "share"),
		"artisan",
	);
};

export const ResolveNpmExecutable = (
	environment: NodeJS.ProcessEnv = process.env,
	platform: NodeJS.Platform = process.platform,
) => {
	const configured_executable = environment.ARTISAN_NPM_EXECUTABLE;
	if (configured_executable !== undefined && isAbsolute(configured_executable)) {
		return configured_executable;
	}
	return resolve(dirname(process.execPath), platform === "win32" ? "npm.cmd" : "npm");
};

/**
 * Captures the exact global prefix that owns this disposable package. npm does
 * not necessarily use its default prefix when the package was installed with
 * `--prefix`, so cleanup must never rediscover the target later.
 */
export const ResolveNpmPrefix = (
	entry_path: string,
	environment: NodeJS.ProcessEnv = process.env,
	platform: NodeJS.Platform = process.platform,
) => {
	const path_service = platform === "win32" ? win32 : posix;
	const configured_prefix = environment.ARTISAN_NPM_PREFIX ?? environment.npm_config_prefix;
	if (configured_prefix !== undefined && path_service.isAbsolute(configured_prefix))
		return path_service.resolve(configured_prefix);

	const package_root = path_service.dirname(path_service.dirname(entry_path));
	const node_modules_root = path_service.dirname(package_root);
	if (
		path_service.basename(package_root) !== "artisan-editor" ||
		path_service.basename(node_modules_root).toLocaleLowerCase("en-US") !== "node_modules"
	)
		throw new Error("Cannot derive the npm prefix from the running artisan-editor package");
	return path_service.dirname(node_modules_root);
};

export const make_node_bootstrap_layer = (entry_path: string) => {
	const installation_root = ResolveInstallationRoot();
	return Layer.mergeAll(
		PlatformLive,
		make_installation_store_layer(installation_root).pipe(
			Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
		),
		make_node_permanent_ae_layer.pipe(
			Layer.provide(
				Layer.mergeAll(
					NodeFileSystem.layer,
					NodeChildProcessSpawner.layer.pipe(
						Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
					),
				),
			),
		),
		make_node_cleanup_layer(entry_path),
		make_node_bootstrap_installer_layer(),
	);
};
