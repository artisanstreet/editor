import { spawn, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import { createWriteStream, type WriteStream } from "node:fs";
import { delimiter, dirname, join } from "node:path";
import { createInterface } from "node:readline";

import {
	Data,
	Deferred,
	Duration,
	Context,
	Effect,
	Fiber,
	Option,
	Ref,
	Schedule,
	Schema,
	Scope,
	Layer,
} from "effect";

import type { ResolvedDesktopPaths } from "./paths";

const ForgeReadyMessage = Schema.Struct({
	endpoint: Schema.String,
	kind: Schema.Literal("artisan:forge-ready"),
	pid: Schema.Number,
});

export interface ForgeConnection {
	readonly http_endpoint: string;
	readonly websocket_endpoint: string;
}

export interface ForgeBrowserSession {
	readonly http_endpoint: string;
	readonly token: string;
}

export interface ForgeProcessSupervisorOptions {
	readonly Fetch?: typeof fetch;
	readonly InstallBrowserSession: (session: ForgeBrowserSession) => Effect.Effect<void, unknown>;
}

export interface DesktopProcessEnvironmentValue {
	readonly diagnostic_log_path: Option.Option<string>;
	readonly inherited: Readonly<Record<string, string | undefined>>;
	readonly packaged_smoke: boolean;
}

export class DesktopProcessEnvironment extends Context.Service<
	DesktopProcessEnvironment,
	DesktopProcessEnvironmentValue
>()("Artisan/DesktopProcessEnvironment") {}

/** The only Desktop adapter allowed to inspect the parent process environment. */
export const NodeDesktopProcessEnvironmentLive = Layer.succeed(
	DesktopProcessEnvironment,
	DesktopProcessEnvironment.of({
		diagnostic_log_path: Option.fromNullishOr(process.env.ARTISAN_FORGE_LOG_PATH),
		inherited: { ...process.env },
		packaged_smoke: process.env.ARTISAN_PACKAGED_SMOKE === "1",
	}),
);

export class ForgeProcessSupervisorError extends Data.TaggedError("ForgeProcessSupervisorError")<{
	readonly cause: unknown;
	readonly operation: "disposed" | "pair" | "spawn" | "startup";
}> {}

export interface ForgeProcessSupervisor {
	readonly Dispose: Effect.Effect<void>;
	readonly GetForgePid: Effect.Effect<Option.Option<number>>;
	readonly Start: Effect.Effect<ForgeConnection, ForgeProcessSupervisorError>;
}

interface SupervisorState {
	readonly child: Option.Option<ChildProcess>;
	readonly disposed: boolean;
	readonly listen_port: string;
}

const initial_restart_delay = Duration.millis(250);
const maximum_restart_delay = Duration.seconds(5);

const RestartSchedule = Schedule.min([
	Schedule.exponential(initial_restart_delay),
	Schedule.duration(maximum_restart_delay),
]);

const decode_ready = (line: string): Option.Option<typeof ForgeReadyMessage.Type> => {
	try {
		return Option.some(Schema.decodeUnknownSync(ForgeReadyMessage)(JSON.parse(line)));
	} catch {
		return Option.none();
	}
};

const AwaitChildExit = (child: ChildProcess) =>
	Effect.callback<{ readonly code: number | null }, never>((resume) => {
		const on_exit = (code: number | null) => resume(Effect.succeed({ code }));
		const on_error = () => resume(Effect.succeed({ code: 1 }));
		child.once("exit", on_exit);
		child.once("error", on_error);

		return Effect.sync(() => {
			child.off("exit", on_exit);
			child.off("error", on_error);
		});
	});

const AwaitReady = (child: ChildProcess) =>
	Effect.callback<string, never>((resume) => {
		if (!child.stdout) return;
		const readline = createInterface({ input: child.stdout });
		const on_line = (line: string) => {
			const message = decode_ready(line);
			if (Option.isSome(message)) resume(Effect.succeed(message.value.endpoint));
		};
		readline.on("line", on_line);

		return Effect.sync(() => {
			readline.off("line", on_line);
			readline.close();
		});
	});

const StopChild = (child: ChildProcess) =>
	Effect.gen(function* () {
		if (child.exitCode !== null) return;
		const exited = yield* Deferred.make<void>();
		const on_exit = () => {
			Deferred.doneUnsafe(exited, Effect.void);
		};
		child.once("exit", on_exit);
		yield* Effect.sync(() => {
			child.send({ kind: "artisan:forge-shutdown" }, (cause) => {
				if (cause) child.kill();
			});
		});
		yield* Deferred.await(exited).pipe(
			Effect.timeoutOption(Duration.seconds(5)),
			Effect.flatMap(
				Option.match({
					onNone: () => Effect.sync(() => child.kill()),
					onSome: () => Effect.void,
				}),
			),
			Effect.ensuring(Effect.sync(() => child.off("exit", on_exit))),
		);
	});

const PrepareConnection = (
	endpoint: string,
	token: string,
	options: ForgeProcessSupervisorOptions,
) =>
	Effect.gen(function* () {
		const fetch_api = options.Fetch ?? fetch;
		const http_endpoint = new URL(endpoint);
		http_endpoint.pathname = "/";
		http_endpoint.search = "";
		http_endpoint.hash = "";
		const pair_request = yield* Effect.tryPromise({
			try: () =>
				fetch_api(new URL("/api/pair/request", http_endpoint), {
					headers: { authorization: `Bearer ${token}` },
					method: "POST",
				}),
			catch: (cause) => new ForgeProcessSupervisorError({ cause, operation: "pair" }),
		});
		if (!pair_request.ok) {
			return yield* new ForgeProcessSupervisorError({
				cause: new Error(`Artisan Forge pairing request failed (${pair_request.status})`),
				operation: "pair",
			});
		}
		const pair_body = yield* Effect.tryPromise({
			try: () => pair_request.json(),
			catch: (cause) => new ForgeProcessSupervisorError({ cause, operation: "pair" }),
		});
		const code =
			typeof pair_body === "object" &&
			pair_body !== null &&
			"code" in pair_body &&
			typeof pair_body.code === "string"
				? pair_body.code
				: undefined;
		if (!code) {
			return yield* new ForgeProcessSupervisorError({
				cause: new Error("Artisan Forge returned an invalid pairing code"),
				operation: "pair",
			});
		}
		const pair_response = yield* Effect.tryPromise({
			try: () =>
				fetch_api(new URL("/api/pair", http_endpoint), {
					body: JSON.stringify({ code }),
					headers: { "content-type": "application/json" },
					method: "POST",
				}),
			catch: (cause) => new ForgeProcessSupervisorError({ cause, operation: "pair" }),
		});
		const token_match = /(?:^|,\s*)artisan_forge_session=([^;,\s]+)/i.exec(
			pair_response.headers.get("set-cookie") ?? "",
		);
		if (!pair_response.ok || !token_match?.[1]) {
			return yield* new ForgeProcessSupervisorError({
				cause: new Error("Artisan Forge pairing exchange did not return a browser session"),
				operation: "pair",
			});
		}
		yield* options
			.InstallBrowserSession({
				http_endpoint: http_endpoint.toString(),
				token: token_match[1],
			})
			.pipe(
				Effect.mapError(
					(cause) => new ForgeProcessSupervisorError({ cause, operation: "pair" }),
				),
			);
		const websocket_endpoint = new URL("/api/ws", http_endpoint);
		websocket_endpoint.protocol = websocket_endpoint.protocol === "https:" ? "wss:" : "ws:";
		return {
			http_endpoint: http_endpoint.toString(),
			websocket_endpoint: websocket_endpoint.toString(),
		};
	});

/** Acquires one scoped, restartable Forge process supervisor. */
export const MakeForgeProcessSupervisor = (
	paths: ResolvedDesktopPaths,
	options: ForgeProcessSupervisorOptions,
): Effect.Effect<
	ForgeProcessSupervisor,
	ForgeProcessSupervisorError,
	Scope.Scope | DesktopProcessEnvironment
> =>
	Effect.gen(function* () {
		const environment = yield* DesktopProcessEnvironment;
		const state = yield* Ref.make<SupervisorState>({
			child: Option.none(),
			disposed: false,
			listen_port: "0",
		});
		const connection = yield* Deferred.make<ForgeConnection, ForgeProcessSupervisorError>();
		const token = yield* Effect.try({
			try: () => randomBytes(32).toString("base64url"),
			catch: (cause) => new ForgeProcessSupervisorError({ cause, operation: "startup" }),
		});
		const diagnostic_log: WriteStream | undefined = Option.isNone(
			environment.diagnostic_log_path,
		)
			? undefined
			: createWriteStream(environment.diagnostic_log_path.value, { flags: "a" });
		diagnostic_log?.on("error", () => undefined);

		const RunOnce = Effect.gen(function* () {
			const current = yield* Ref.get(state);
			if (current.disposed) return { code: 0 };
			const forge_root = dirname(paths.forge_entry_path);
			const child = yield* Effect.try({
				try: () =>
					spawn(paths.forge_executable_path, [paths.forge_entry_path], {
						env: {
							...environment.inherited,
							ARTISAN_ALLOWED_ORIGINS: "",
							ARTISAN_AUTH_TOKEN: token,
							ARTISAN_DATABASE_PATH: paths.database_path,
							ARTISAN_LISTEN_HOST: "127.0.0.1",
							ARTISAN_LISTEN_PORT: current.listen_port,
							ARTISAN_MIGRATIONS_PATH: join(forge_root, "migrations"),
							ARTISAN_NATIVE_RUNTIME: paths.forge_native_runtime_path,
							ARTISAN_NODE_EXECUTABLE: paths.forge_node_executable_path,
							ARTISAN_STATIC_FRONTEND_ROOT: join(forge_root, "frontend"),
							ARTISAN_WINDOWS_PROCESS_HOST: join(
								forge_root,
								"windows-process-host.js",
							),
							NODE_PATH: [
								paths.forge_native_runtime_path,
								environment.inherited.NODE_PATH,
							]
								.filter(Boolean)
								.join(delimiter),
						},
						stdio: ["ignore", "pipe", "pipe", "ipc"],
						windowsHide: true,
					}),
				catch: (cause) => new ForgeProcessSupervisorError({ cause, operation: "spawn" }),
			});
			yield* Ref.update(state, (value) => ({ ...value, child: Option.some(child) }));
			if (!child.stdout || !child.stderr) {
				child.kill();
				return { code: 1 };
			}
			child.stdout.on("data", (data) => diagnostic_log?.write(data));
			child.stderr.on("data", (data) => {
				diagnostic_log?.write(data);
				if (environment.packaged_smoke) process.stderr.write(data);
			});
			const ready_fiber = yield* AwaitReady(child).pipe(
				Effect.timeoutOrElse({
					duration: Duration.seconds(20),
					orElse: () =>
						Effect.fail(
							new ForgeProcessSupervisorError({
								cause: new Error("Artisan Forge readiness timed out"),
								operation: "startup",
							}),
						),
				}),
				Effect.flatMap((endpoint) => PrepareConnection(endpoint, token, options)),
				Effect.tap((value) =>
					Effect.gen(function* () {
						const url = new URL(value.http_endpoint);
						yield* Ref.update(state, (previous) => ({
							...previous,
							listen_port:
								previous.listen_port === "0" ? url.port : previous.listen_port,
						}));
						yield* Deferred.succeed(connection, value);
					}),
				),
				Effect.catch((cause) =>
					Deferred.fail(connection, cause).pipe(
						Effect.andThen(Effect.sync(() => child.kill())),
					),
				),
				Effect.forkScoped,
			);
			/** Ensure the readiness listener is installed before the child can emit its first line. */
			yield* Effect.yieldNow;
			const result = yield* AwaitChildExit(child);
			yield* Fiber.interrupt(ready_fiber);
			yield* Ref.update(state, (value) => ({ ...value, child: Option.none() }));
			return result;
		});

		const lifecycle = RunOnce.pipe(
			Effect.flatMap(({ code }) =>
				code === 0
					? Effect.void
					: Effect.fail(
							new ForgeProcessSupervisorError({
								cause: new Error("Artisan Forge exited"),
								operation: "startup",
							}),
						),
			),
			Effect.retry(RestartSchedule),
			Effect.catch(() => Effect.void),
			Effect.forkScoped,
		);
		yield* lifecycle;

		return {
			Dispose: Ref.update(state, (value) => ({ ...value, disposed: true })).pipe(
				Effect.andThen(Ref.get(state)),
				Effect.flatMap((value) =>
					Option.match(value.child, {
						onNone: () => Effect.void,
						onSome: StopChild,
					}),
				),
				Effect.ensuring(
					Effect.sync(() => {
						diagnostic_log?.end();
					}),
				),
			),
			GetForgePid: Ref.get(state).pipe(
				Effect.map((value) =>
					Option.flatMap(value.child, (child) => Option.fromNullishOr(child.pid)),
				),
			),
			Start: Ref.get(state).pipe(
				Effect.flatMap((value) =>
					value.disposed
						? Effect.fail(
								new ForgeProcessSupervisorError({
									cause: new Error("Artisan Forge supervisor has been disposed"),
									operation: "disposed",
								}),
							)
						: Deferred.await(connection),
				),
			),
		} satisfies ForgeProcessSupervisor;
	});

/** Owns the supervisor for the surrounding Effect scope. */
export const AcquireForgeProcessSupervisor = (
	paths: ResolvedDesktopPaths,
	options: ForgeProcessSupervisorOptions,
): Effect.Effect<ForgeProcessSupervisor, ForgeProcessSupervisorError, Scope.Scope> =>
	Effect.acquireRelease(
		MakeForgeProcessSupervisor(paths, options).pipe(
			Effect.provide(NodeDesktopProcessEnvironmentLive),
		),
		(supervisor) => supervisor.Dispose,
	);
