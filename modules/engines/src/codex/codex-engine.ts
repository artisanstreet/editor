import { Effect } from "effect";

import {
	type Engine,
	type EngineApprovalInput,
	type EngineCancelInput,
	type EngineCloseInput,
	type EngineDescriptor,
	type EngineInspectInput,
	type EngineInspection,
	type EngineResumeInput,
	type EngineStartInput,
	type EngineSteerInput,
	EngineProcessError,
	EngineProtocolError,
	EngineUnavailableError,
	EngineUnsupportedOperationError,
} from "../engine";
import { CodexJsonlFramer, CodexJsonlMalformedLineError } from "./codex-jsonl";
import { CodexProcessFactory, type CodexProcessHandle } from "./codex-process";
import {
	CodexTransportMetadata,
	DecodeCodexInitializeResponse,
	make_codex_initialize_request,
} from "./codex-protocol";

/** Identifies the first supported Codex engine adapter. @since 0.1.0 */
export const CodexEngineDescriptor: EngineDescriptor = {
	capabilities: {
		approval: "unsupported",
		cancel: "unsupported",
		close: "unsupported",
		inspect: "supported",
		resume: "unsupported",
		start: "unsupported",
		steer: "unsupported",
	},
	display_name: "Codex CLI",
	id: "codex",
	transport: CodexTransportMetadata.transport,
};

/** Configures the Codex executable used by an engine adapter. @since 0.1.0 */
export interface CodexEngineOptions {
	readonly executable?: string;
}

function read_stdout(handle: CodexProcessHandle) {
	return Effect.tryPromise({
		try: async () => {
			const chunks: Array<Uint8Array> = [];

			for await (const chunk of handle.Stdout) {
				chunks.push(chunk);
			}

			return chunks;
		},
		catch: (cause) => new EngineProcessError({ cause, operation: "exit" }),
	});
}

function read_initialize_response(handle: CodexProcessHandle) {
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
				: new EngineProcessError({ cause, operation: "exit" }),
	});
}

function parse_codex_version(chunks: ReadonlyArray<Uint8Array>) {
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

function validate_codex_transport_version(version: string) {
	return version === CodexTransportMetadata.cli_version
		? Effect.void
		: Effect.fail(
				new EngineProtocolError({
					engine_id: "codex",
					message: `Codex ${version} does not match the pinned ${CodexTransportMetadata.cli_version} transport`,
				}),
			);
}

function unsupported(operation: EngineUnsupportedOperationError["operation"]) {
	return Effect.fail(new EngineUnsupportedOperationError({ engine_id: "codex", operation }));
}

/**
 * Creates the Codex engine foundation with a non-billable inspect handshake.
 *
 * @since 0.1.0
 * @param options - Optional executable override used by integration environments.
 * @returns An engine requiring a `CodexProcessFactory` service when run.
 */
export function make_codex_engine(options: CodexEngineOptions = {}): Engine<CodexProcessFactory> {
	const executable = options.executable ?? "codex";
	const Inspect = (input: EngineInspectInput) =>
		Effect.gen(function* () {
			const factory = yield* CodexProcessFactory;
			const version_process = yield* factory.Spawn({
				args: ["--version"],
				command: executable,
				shell: process.platform === "win32",
			});
			const version_chunks = yield* read_stdout(version_process).pipe(
				Effect.ensuring(version_process.Close),
			);
			const version = yield* parse_codex_version(version_chunks);

			yield* validate_codex_transport_version(version);

			const initialize_process = yield* factory.Spawn({
				args: ["app-server", "--stdio"],
				command: executable,
				shell: process.platform === "win32",
			});
			const request = make_codex_initialize_request(
				input.client_name ?? "artisan-editor",
				input.client_version ?? "0.1.0",
			);
			const response = yield* initialize_process
				.Write(new TextEncoder().encode(`${JSON.stringify(request)}\n`))
				.pipe(
					Effect.andThen(read_initialize_response(initialize_process)),
					Effect.ensuring(initialize_process.Close),
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
				descriptor: CodexEngineDescriptor,
				metadata: {
					codex_home: initialized.result.codexHome,
					platform_family: initialized.result.platformFamily,
					platform_os: initialized.result.platformOs,
					user_agent: initialized.result.userAgent,
				},
				version,
			} satisfies EngineInspection;
		});

	return {
		Approve: (_input: EngineApprovalInput) => unsupported("approval"),
		Cancel: (_input: EngineCancelInput) => unsupported("cancel"),
		Close: (_input: EngineCloseInput) => unsupported("close"),
		Descriptor: CodexEngineDescriptor,
		Inspect,
		Resume: (_input: EngineResumeInput) => unsupported("resume"),
		Start: (_input: EngineStartInput) => unsupported("start"),
		Steer: (_input: EngineSteerInput) => unsupported("steer"),
	};
}
