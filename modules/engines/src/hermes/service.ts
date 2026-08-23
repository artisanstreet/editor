import { Buffer } from "node:buffer";
import { randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import { join } from "node:path";

import { Effect, Scope, Stream } from "effect";

import {
	EngineProcessError,
	EngineProtocolError,
	EngineUnavailableError,
	type EngineFailure,
} from "../engine";
import type { EngineProcessFactory, EngineProcessHandle } from "../process/process";
import { ConnectHermesGateway, HermesGatewayError, type HermesGatewayClient } from "./protocol";

const maximum_ready_output_bytes = 256 * 1024;
const maximum_version_output_bytes = 1024 * 1024;
const minimum_hermes_version = [0, 20, 0] as const;

const HermesExecutable = () => {
	const configured = process.env.HERMES_EXECUTABLE?.trim();
	if (configured) return configured;
	if (process.platform === "win32" && process.env.LOCALAPPDATA) {
		const installed = join(
			process.env.LOCALAPPDATA,
			"hermes",
			"hermes-agent",
			"bin",
			"hermes.exe",
		);
		if (existsSync(installed)) return installed;
	}
	return "hermes";
};

const ReadBoundedText = (stream: AsyncIterable<Uint8Array>, maximum_bytes: number) =>
	Effect.tryPromise({
		try: async () => {
			const chunks: Array<Uint8Array> = [];
			let total = 0;
			for await (const chunk of stream) {
				total += chunk.byteLength;
				if (total > maximum_bytes) throw new Error("Hermes output exceeded its bound");
				chunks.push(chunk);
			}
			return new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks));
		},
		catch: (cause) => new EngineProcessError({ cause, operation: "read" }),
	});

const HermesVersion = (
	factory: typeof EngineProcessFactory.Service,
	working_directory: string,
	profile_id: string,
) =>
	Effect.scoped(
		Effect.gen(function* () {
			const handle = yield* factory.Spawn({
				args: ["--version"],
				command: HermesExecutable(),
				cwd: working_directory,
				env: { ...process.env },
				profile_id,
			});
			const [stdout, stderr, exit] = yield* Effect.all(
				[
					ReadBoundedText(handle.Stdout, maximum_version_output_bytes),
					ReadBoundedText(handle.Stderr, maximum_version_output_bytes),
					handle.Exit,
				],
				{ concurrency: "unbounded" },
			).pipe(Effect.ensuring(handle.Close));
			if (exit.code !== 0)
				return yield* new EngineUnavailableError({
					engine_id: "hermes",
					message: stderr.trim() || "Hermes version probe exited unsuccessfully.",
				});
			const match = /Hermes Agent v(\d+)\.(\d+)\.(\d+)/i.exec(stdout);
			if (match === null)
				return yield* new EngineProtocolError({
					engine_id: "hermes",
					message: "Hermes returned an unrecognized version string.",
				});
			const version = [Number(match[1]), Number(match[2]), Number(match[3])] as const;
			for (let index = 0; index < minimum_hermes_version.length; index += 1) {
				const difference = version[index]! - minimum_hermes_version[index]!;
				if (difference === 0) continue;
				if (difference < 0)
					return yield* new EngineUnavailableError({
						engine_id: "hermes",
						message: `Hermes ${version.join(".")} is older than the supported 0.20.0 gateway.`,
					});
				break;
			}
			return version.join(".");
		}),
	);

const ReadReadyPort = (stdout: AsyncIterable<Uint8Array>) =>
	Effect.tryPromise({
		try: async () => {
			const iterator = stdout[Symbol.asyncIterator]();
			const decoder = new TextDecoder("utf-8", { fatal: true });
			let pending = "";
			let consumed_bytes = 0;
			for (;;) {
				const chunk = await iterator.next();
				if (chunk.done) throw new Error("Hermes exited before reporting readiness");
				consumed_bytes += chunk.value.byteLength;
				if (consumed_bytes > maximum_ready_output_bytes)
					throw new Error("Hermes readiness output exceeded its bound");
				pending += decoder.decode(chunk.value, { stream: true });
				let newline = pending.indexOf("\n");
				while (newline !== -1) {
					const line = pending.slice(0, newline).replace(/\r$/, "").trim();
					pending = pending.slice(newline + 1);
					const match = /^HERMES_BACKEND_READY port=(\d+)$/.exec(line);
					if (match !== null) {
						const port = Number(match[1]);
						if (!Number.isSafeInteger(port) || port <= 0 || port > 65_535)
							throw new Error("Hermes reported an invalid loopback port");
						return { iterator, port };
					}
					newline = pending.indexOf("\n");
				}
			}
		},
		catch: (cause) =>
			new EngineProtocolError({
				engine_id: "hermes",
				message:
					cause instanceof Error ? cause.message : "Invalid Hermes readiness output.",
			}),
	});

const ClosePrivateService = (client: HermesGatewayClient, handle: EngineProcessHandle) =>
	Effect.gen(function* () {
		yield* client.Close.pipe(Effect.ignore);
		yield* handle.Kill().pipe(Effect.ignore);
		yield* handle.Close;
	}).pipe(Effect.timeout("8 seconds"), Effect.ignore);

export interface HermesPrivateService {
	readonly Client: HermesGatewayClient;
	readonly Close: Effect.Effect<void>;
	readonly Closed: Effect.Effect<void>;
	readonly endpoint: URL;
	readonly version: string;
}

export interface HermesPrivateServiceOptions {
	readonly factory: typeof EngineProcessFactory.Service;
	readonly profile_id: string;
	readonly working_directory: string;
}

const gateway_failure = (cause: HermesGatewayError): EngineFailure =>
	new EngineUnavailableError({ engine_id: "hermes", message: cause.message });

/** Starts one private loopback Hermes backend owned by the caller's service scope. */
export const StartHermesPrivateService = (
	options: HermesPrivateServiceOptions,
): Effect.Effect<HermesPrivateService, EngineFailure, Scope.Scope> =>
	Effect.gen(function* () {
		const version = yield* HermesVersion(
			options.factory,
			options.working_directory,
			options.profile_id,
		);
		const session_token = randomBytes(32).toString("base64url");
		const handle = yield* options.factory.Spawn({
			args: ["serve", "--host", "127.0.0.1", "--port", "0"],
			command: HermesExecutable(),
			cwd: options.working_directory,
			env: {
				...process.env,
				HERMES_DASHBOARD_SESSION_TOKEN: session_token,
				HERMES_PARENT_PID: String(process.pid),
			},
			profile_id: options.profile_id,
		});
		const ready = yield* ReadReadyPort(handle.Stdout).pipe(
			Effect.timeoutOrElse({
				duration: "30 seconds",
				orElse: () =>
					Effect.fail(
						new EngineUnavailableError({
							engine_id: "hermes",
							message: "Hermes did not report its private backend endpoint in time.",
						}),
					),
			}),
		);
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
		const endpoint = new URL(`ws://127.0.0.1:${ready.port}/api/ws`);
		endpoint.searchParams.set("token", session_token);
		const Client = yield* ConnectHermesGateway(endpoint).pipe(Effect.mapError(gateway_failure));
		const Close = yield* Effect.cached(ClosePrivateService(Client, handle));
		const scope = yield* Scope.Scope;
		yield* Scope.addFinalizer(scope, Close);
		return {
			Client,
			Close,
			Closed: Effect.raceFirst(Client.Closed, handle.Exit.pipe(Effect.asVoid, Effect.orDie)),
			endpoint,
			version,
		};
	});
