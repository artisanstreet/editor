import { Context, Effect, Layer } from "effect";

import {
	type Engine,
	type EngineDescriptor,
	type EngineProbe,
	type EngineProbeInput,
	EngineProbeTimeoutError,
	EngineProcessError,
	EngineProtocolError,
	EngineUnavailableError,
	EngineUnsupportedOperationError,
} from "../engine";
import { CodexJsonlFramer, CodexJsonlMalformedLineError } from "./codex-jsonl";
import {
	CodexProcessFactory,
	type CodexProcessHandle,
	type CodexProcessSpawnInput,
} from "./codex-process";
import {
	CodexTransportMetadata,
	DecodeCodexInitializeResponse,
	make_codex_initialize_request,
} from "./codex-protocol";

/** Identifies the Codex adapter and its currently available capabilities. @since 0.2.0 */
export const CodexEngineDescriptor: EngineDescriptor = {
	capabilities: {
		approval: { reason: "Open is not implemented", state: "unsupported" },
		auth: { reason: "Initialize does not verify authenticated access", state: "experimental" },
		cancel: { reason: "Open is not implemented", state: "unsupported" },
		close: { reason: "Open is not implemented", state: "unsupported" },
		events: { reason: "Open is not implemented", state: "unsupported" },
		model_selection: { reason: "Open is not implemented", state: "unsupported" },
		native_tools: { reason: "Open is not implemented", state: "unsupported" },
		probe: { state: "supported" },
		question: { reason: "Open is not implemented", state: "unsupported" },
		raw_frames: { reason: "Open is not implemented", state: "unsupported" },
		resume: { reason: "Open is not implemented", state: "unsupported" },
		start: { reason: "Open is not implemented", state: "unsupported" },
		steer: { reason: "Open is not implemented", state: "unsupported" },
		subagents: { reason: "Open is not implemented", state: "unsupported" },
	},
	display_name: "Codex CLI",
	id: "codex",
	transport: CodexTransportMetadata.transport,
};

/** Configures the Codex executable and non-billable probe deadlines. @since 0.2.0 */
export interface CodexEngineOptions {
	readonly executable?: string;
	readonly initialize_timeout_ms?: number;
	readonly version_timeout_ms?: number;
}

/** Provides a dependency-free Codex engine assembled by its Layer. @since 0.2.0 */
export class CodexEngine extends Context.Service<CodexEngine, Engine>()("Artisan/CodexEngine") {}

function ReadStdout(handle: CodexProcessHandle) {
	return Effect.tryPromise({
		try: async () => {
			const chunks: Array<Uint8Array> = [];

			for await (const chunk of handle.Stdout) {
				chunks.push(chunk);
			}

			return chunks;
		},
		catch: (cause) => new EngineProcessError({ cause, operation: "read" }),
	});
}

function DrainStderr(handle: CodexProcessHandle) {
	return Effect.tryPromise({
		try: async () => {
			for await (const _chunk of handle.Stderr) {
			}
		},
		catch: (cause) => new EngineProcessError({ cause, operation: "read" }),
	});
}

function ReadInitializeResponse(handle: CodexProcessHandle) {
	return Effect.tryPromise({
		try: async () => {
			const framer = new CodexJsonlFramer();

			for await (const chunk of handle.Stdout) {
				const messages = framer.Push(chunk);
				const response = messages.find(
					(message) =>
						typeof message === "object" &&
						message !== null &&
						"id" in message &&
						(message as { readonly id?: unknown }).id === 1,
				);

				if (response) {
					return response;
				}
			}

			const response = framer
				.Finish()
				.find(
					(message) =>
						typeof message === "object" &&
						message !== null &&
						"id" in message &&
						(message as { readonly id?: unknown }).id === 1,
				);

			if (!response) {
				throw new Error("Codex app-server exited before initialize responded");
			}

			return response;
		},
		catch: (cause) =>
			cause instanceof CodexJsonlMalformedLineError
				? new EngineProtocolError({ engine_id: "codex", message: cause.message })
				: new EngineProcessError({ cause, operation: "read" }),
	});
}

function ParseCodexVersion(chunks: ReadonlyArray<Uint8Array>) {
	const output = new TextDecoder().decode(
		chunks.reduce((combined, chunk) => {
			const next = new Uint8Array(combined.length + chunk.length);

			next.set(combined);
			next.set(chunk, combined.length);

			return next;
		}, new Uint8Array()),
	);
	const version = output.match(/\b(\d+\.\d+\.\d+)\b/)?.[1];

	return version
		? Effect.succeed(version)
		: Effect.fail(
				new EngineUnavailableError({
					engine_id: "codex",
					message: "Codex --version did not contain a semantic version",
				}),
			);
}

function ValidateCodexTransportVersion(version: string) {
	return version === CodexTransportMetadata.cli_version
		? Effect.void
		: Effect.fail(
				new EngineProtocolError({
					engine_id: "codex",
					message: `Codex ${version} does not match the pinned ${CodexTransportMetadata.cli_version} transport`,
				}),
			);
}

function RunProbePhase<A, E>(
	factory: typeof CodexProcessFactory.Service,
	spawn_input: CodexProcessSpawnInput,
	phase: EngineProbeTimeoutError["phase"],
	timeout_ms: number,
	Use: (handle: CodexProcessHandle) => Effect.Effect<A, E>,
) {
	const Run = Effect.gen(function* () {
		const handle = yield* factory.Spawn(spawn_input);

		return yield* Effect.scoped(
			Effect.gen(function* () {
				yield* Effect.forkScoped(DrainStderr(handle).pipe(Effect.ignore));

				return yield* Use(handle);
			}),
		).pipe(Effect.ensuring(handle.Close));
	});

	return Run.pipe(
		Effect.timeoutOrElse({
			duration: timeout_ms,
			orElse: () =>
				Effect.fail(new EngineProbeTimeoutError({ engine_id: "codex", phase, timeout_ms })),
		}),
	);
}

function make_codex_engine(
	factory: typeof CodexProcessFactory.Service,
	options: CodexEngineOptions,
): Engine {
	const executable = options.executable ?? "codex";
	const initialize_timeout_ms = options.initialize_timeout_ms ?? 5_000;
	const version_timeout_ms = options.version_timeout_ms ?? 5_000;
	const Probe = (input: EngineProbeInput) =>
		Effect.gen(function* () {
			const version_chunks = yield* RunProbePhase(
				factory,
				{
					args: ["--version"],
					command: executable,
					shell: process.platform === "win32",
				},
				"version",
				version_timeout_ms,
				ReadStdout,
			);
			const version = yield* ParseCodexVersion(version_chunks);

			yield* ValidateCodexTransportVersion(version);

			const request = make_codex_initialize_request(
				input.client_name ?? "artisan-editor",
				input.client_version ?? "0.2.0",
			);
			const response = yield* RunProbePhase(
				factory,
				{
					args: ["app-server", "--stdio"],
					command: executable,
					shell: process.platform === "win32",
				},
				"initialize",
				initialize_timeout_ms,
				(handle) =>
					handle
						.Write(new TextEncoder().encode(`${JSON.stringify(request)}\n`))
						.pipe(Effect.andThen(ReadInitializeResponse(handle))),
			);
			const initialized = yield* DecodeCodexInitializeResponse(response).pipe(
				Effect.mapError(
					() =>
						new EngineProtocolError({
							engine_id: "codex",
							message: "Codex app-server returned an invalid initialize response",
						}),
				),
			);

			return {
				authentication: {
					reason: "Initialize does not verify authenticated access",
					state: "unknown",
				},
				capabilities: CodexEngineDescriptor.capabilities,
				descriptor: CodexEngineDescriptor,
				metadata: {
					codex_home: initialized.result.codexHome,
					platform_family: initialized.result.platformFamily,
					platform_os: initialized.result.platformOs,
					user_agent: initialized.result.userAgent,
				},
				ready: true,
				version,
			} satisfies EngineProbe;
		});

	return {
		Descriptor: CodexEngineDescriptor,
		Open: () =>
			Effect.fail(
				new EngineUnsupportedOperationError({ engine_id: "codex", operation: "open" }),
			),
		Probe,
	};
}

/**
 * Builds the Codex engine Layer and captures its process factory at composition.
 *
 * @since 0.2.0
 * @param options - Executable override and phase-specific probe deadlines.
 * @returns A Layer whose public engine has no process dependency in its Effects.
 */
export function make_codex_engine_layer(
	options: CodexEngineOptions = {},
): Layer.Layer<CodexEngine, never, CodexProcessFactory> {
	return Layer.effect(
		CodexEngine,
		Effect.gen(function* () {
			const factory = yield* CodexProcessFactory;

			return make_codex_engine(factory, options);
		}),
	);
}
