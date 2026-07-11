import { Effect } from "effect";

import {
	EngineProbeTimeoutError,
	EngineProcessError,
	EngineProtocolError,
	EngineUnavailableError,
} from "../../engine";
import type { CodexProcessFactory } from "../codex-process";

/** Configures the bounded non-billable readiness probe for exec fallback. */
export interface CodexExecProbeOptions {
	readonly executable_args: ReadonlyArray<string>;
	readonly executable: string;
	readonly factory: typeof CodexProcessFactory.Service;
	readonly max_stderr_bytes: number;
	readonly max_stdout_bytes: number;
	readonly timeout_ms: number;
}

function ReadBoundedStream(
	stream: AsyncIterable<Uint8Array>,
	channel: "stderr" | "stdout",
	max_bytes: number,
) {
	return Effect.tryPromise({
		try: async () => {
			const output = new Uint8Array(max_bytes);
			let length = 0;

			for await (const chunk of stream) {
				if (length + chunk.length > max_bytes) {
					throw new EngineProtocolError({
						engine_id: "codex",
						message: `Codex version ${channel} exceeded ${max_bytes} bytes`,
					});
				}

				output.set(chunk, length);
				length += chunk.length;
			}

			return output.slice(0, length);
		},
		catch: (cause) =>
			cause instanceof EngineProtocolError
				? cause
				: new EngineProcessError({ cause, operation: "read" }),
	});
}

function ParseVersion(stdout: Uint8Array) {
	const output = new TextDecoder().decode(stdout);
	const version = output.match(/\b(\d+\.\d+\.\d+)\b/)?.[1];

	return version === undefined
		? Effect.fail(
				new EngineUnavailableError({
					engine_id: "codex",
					message: "Codex --version did not contain a semantic version",
				}),
			)
		: Effect.succeed(version);
}

/** Probes the exec executable with argv-only `--version` and bounded output. */
export function ProbeCodexExecVersion(options: CodexExecProbeOptions) {
	const Probe = Effect.gen(function* () {
		const handle = yield* options.factory.Spawn({
			args: [...options.executable_args, "--version"],
			command: options.executable,
		});

		return yield* Effect.all(
			[
				ReadBoundedStream(handle.Stdout, "stdout", options.max_stdout_bytes),
				ReadBoundedStream(handle.Stderr, "stderr", options.max_stderr_bytes),
				handle.Exit,
			],
			{ concurrency: "unbounded" },
		).pipe(
			Effect.ensuring(handle.Close),
			Effect.flatMap(([stdout, stderr, process_exit]) => {
				if (process_exit.code !== 0) {
					const detail = new TextDecoder().decode(stderr).trim();

					return Effect.fail(
						new EngineUnavailableError({
							engine_id: "codex",
							message: `Codex --version exited with code ${String(process_exit.code)}${detail.length === 0 ? "" : `: ${detail}`}`,
						}),
					);
				}

				return ParseVersion(stdout);
			}),
		);
	});

	return Probe.pipe(
		Effect.timeoutOrElse({
			duration: options.timeout_ms,
			orElse: () =>
				Effect.fail(
					new EngineProbeTimeoutError({
						engine_id: "codex",
						phase: "version",
						timeout_ms: options.timeout_ms,
					}),
				),
		}),
	);
}
