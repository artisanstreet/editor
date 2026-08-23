import { Buffer } from "node:buffer";
import { randomBytes } from "node:crypto";

import { Effect, Option, Schema, Scope, Stream } from "effect";

import {
	EngineProcessError,
	EngineProtocolError,
	EngineUnavailableError,
	type EngineFailure,
} from "../engine";
import type { EngineProcessFactory, EngineProcessHandle } from "../process/process";
import { opencode2_certified_version } from "../toolchain/distribution";
import { MakeOpenCode2ApiClient, type OpenCode2ApiClient } from "./protocol";

const maximum_ready_line_bytes = 64 * 1024;

const ReadyPayload = Schema.Struct({ url: Schema.NonEmptyString });

const ReadReadyUrl = (stdout: AsyncIterable<Uint8Array>) =>
	Effect.tryPromise({
		try: async () => {
			const iterator = stdout[Symbol.asyncIterator]();
			const decoder = new TextDecoder("utf-8", { fatal: true });
			let pending = "";
			for (;;) {
				const chunk = await iterator.next();
				if (chunk.done) throw new Error("OpenCode exited before reporting readiness");
				pending += decoder.decode(chunk.value, { stream: true });
				if (Buffer.byteLength(pending, "utf8") > maximum_ready_line_bytes)
					throw new Error("OpenCode readiness output exceeded its bound");
				const newline = pending.indexOf("\n");
				if (newline === -1) continue;
				const line = pending.slice(0, newline).replace(/\r$/, "");
				const decoded = Schema.decodeUnknownSync(Schema.fromJsonString(ReadyPayload))(line);
				return { iterator, url: decoded.url };
			}
		},
		catch: (cause) =>
			new EngineProtocolError({
				engine_id: "opencode2",
				message: cause instanceof Error ? cause.message : "Invalid readiness payload",
			}),
	});

const LoopbackEndpoint = (value: string) =>
	Effect.try({
		try: () => {
			const url = new URL(value);
			const port = Number(url.port);
			if (
				url.protocol !== "http:" ||
				!new Set(["127.0.0.1", "[::1]"]).has(url.hostname.toLowerCase()) ||
				url.username !== "" ||
				url.password !== "" ||
				!Number.isSafeInteger(port) ||
				port <= 0 ||
				port > 65_535
			)
				throw new Error("OpenCode reported a non-loopback or credentialed endpoint");
			return url;
		},
		catch: (cause) =>
			new EngineProtocolError({
				engine_id: "opencode2",
				message: cause instanceof Error ? cause.message : "Invalid service endpoint",
			}),
	});

const ClosePrivateService = (handle: EngineProcessHandle) =>
	Effect.gen(function* () {
		yield* handle.EndInput.pipe(Effect.ignore);
		const exited = yield* handle.Exit.pipe(
			Effect.timeoutOption("3 seconds"),
			Effect.orElseSucceed(() => Option.none()),
		);
		if (Option.isNone(exited)) yield* handle.Kill().pipe(Effect.ignore);
		yield* handle.Close;
	}).pipe(Effect.uninterruptible);

export interface OpenCode2PrivateService {
	readonly Client: OpenCode2ApiClient;
	readonly Close: Effect.Effect<void>;
	readonly endpoint: URL;
	readonly version: string;
}

export interface OpenCode2PrivateServiceOptions {
	readonly factory: typeof EngineProcessFactory.Service;
	readonly profile_id: string;
	readonly working_directory: string;
}

/** Starts one Artisan-owned stdio-leased V2 server in the caller's scope. */
export const StartOpenCode2PrivateService = (
	options: OpenCode2PrivateServiceOptions,
): Effect.Effect<OpenCode2PrivateService, EngineFailure, Scope.Scope> =>
	Effect.gen(function* () {
		const password = randomBytes(32).toString("base64url");
		const handle = yield* options.factory.Spawn({
			args: ["serve", "--stdio", "--port", "0"],
			command: "opencode2",
			cwd: options.working_directory,
			env: {
				...process.env,
				OPENCODE_PASSWORD: password,
				OPENCODE_SERVER_PASSWORD: "",
			},
			profile_id: options.profile_id,
		});
		const scope = yield* Scope.Scope;
		const Close = yield* Effect.cached(ClosePrivateService(handle));
		yield* Scope.addFinalizer(scope, Close);
		const ready = yield* ReadReadyUrl(handle.Stdout).pipe(
			Effect.timeoutOrElse({
				duration: "15 seconds",
				orElse: () =>
					Effect.fail(
						new EngineUnavailableError({
							engine_id: "opencode2",
							message: "OpenCode did not report a private service endpoint in time.",
						}),
					),
			}),
		);
		/** Continue draining both streams so the child can never block on a full pipe. */
		const remainder: AsyncIterable<Uint8Array> = {
			[Symbol.asyncIterator]: () => ready.iterator,
		};
		yield* Stream.fromAsyncIterable(
			remainder,
			(cause) => new EngineProcessError({ cause, operation: "read" }),
		).pipe(Stream.runDrain, Effect.ignore, Effect.forkScoped);
		yield* Stream.fromAsyncIterable(
			handle.Stderr,
			(cause) => new EngineProcessError({ cause, operation: "read" }),
		).pipe(Stream.runDrain, Effect.ignore, Effect.forkScoped);
		const endpoint = yield* LoopbackEndpoint(ready.url);
		const Client = MakeOpenCode2ApiClient({ endpoint, password });
		const health = yield* Client.Health.pipe(
			Effect.timeoutOrElse({
				duration: "10 seconds",
				orElse: () =>
					Effect.fail(
						new EngineUnavailableError({
							engine_id: "opencode2",
							message: "OpenCode did not answer its private health probe.",
						}),
					),
			}),
			Effect.mapError((cause) =>
				cause instanceof EngineUnavailableError
					? cause
					: new EngineUnavailableError({
							engine_id: "opencode2",
							message: "OpenCode rejected its private health probe.",
						}),
			),
		);
		if (health.version !== opencode2_certified_version)
			return yield* new EngineUnavailableError({
				engine_id: "opencode2",
				message: `OpenCode ${health.version} is not the certified ${opencode2_certified_version} build.`,
			});
		return { Client, Close, endpoint, version: health.version };
	});
