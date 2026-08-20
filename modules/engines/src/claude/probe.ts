import { Buffer } from "node:buffer";

import { Effect, Schema } from "effect";

import {
	type EngineDescriptor,
	type EngineFailure,
	type EngineProbe,
	EngineProbeTimeoutError,
	EngineProcessError,
	EngineProtocolError,
	EngineUnavailableError,
} from "../engine";
import { EngineProcessFactory } from "../process/process";

/** The Claude Code release whose native resume behavior Artisan has verified. @since 0.8.0 */
export const claude_native_continuation_version = "2.1.220";

/** Configures the bounded, non-billable Claude readiness probe. @since 0.8.0 */
export interface ClaudeProbeOptions {
	readonly auth_timeout_ms: number;
	readonly descriptor: EngineDescriptor;
	readonly executable: string;
	readonly executable_args: ReadonlyArray<string>;
	readonly max_stderr_bytes: number;
	readonly max_stdout_bytes: number;
	readonly version_timeout_ms: number;
}

function read_bounded(stream: AsyncIterable<Uint8Array>, max_bytes: number) {
	return Effect.tryPromise({
		try: async () => {
			const chunks: Array<Uint8Array> = [];
			let total = 0;
			for await (const chunk of stream) {
				total += chunk.length;
				if (total > max_bytes)
					throw new EngineProtocolError({
						engine_id: "claude",
						message: `Claude probe output exceeded ${max_bytes} bytes`,
					});
				chunks.push(chunk);
			}
			return Buffer.concat(chunks);
		},
		catch: (cause) =>
			cause instanceof EngineProtocolError
				? cause
				: new EngineProcessError({ cause, operation: "read" }),
	});
}

/**
 * Probes the installed Claude Code CLI for version and saved-session auth
 * without starting a billable run. Both readiness checks stay behind the same
 * external executable boundary used for actual Claude runs.
 *
 * @since 0.8.0
 * @param factory - The process factory used for the two bounded CLI spawns.
 * @param options - Executable, byte bounds, and per-phase deadlines.
 * @returns Version, authentication readiness, and the declared capabilities.
 */
function RunClaudeCli(
	factory: typeof EngineProcessFactory.Service,
	options: ClaudeProbeOptions,
	args: ReadonlyArray<string>,
	timeout_ms: number,
) {
	return Effect.scoped(
		Effect.gen(function* () {
			const handle = yield* factory.Spawn({
				command: options.executable,
				args: [...options.executable_args, ...args],
			});

			/** A probe is one-shot; unlike a run session, it must observe EOF. */
			yield* handle.EndInput;

			return yield* Effect.all(
				[
					read_bounded(handle.Stdout, options.max_stdout_bytes),
					read_bounded(handle.Stderr, options.max_stderr_bytes),
					handle.Exit,
				],
				{ concurrency: "unbounded" },
			).pipe(
				Effect.ensuring(handle.Close),
				Effect.timeoutOrElse({
					duration: timeout_ms,
					orElse: () =>
						Effect.fail(
							new EngineProbeTimeoutError({
								engine_id: "claude",
								phase: args[0] === "auth" ? "authentication" : "version",
								timeout_ms,
							}),
						),
				}),
			);
		}),
	);
}

/**
 * Reads saved-session auth readiness through the one CLI spawn that answers it.
 *
 * Split out because a usage read needs exactly this and nothing else. Asking a
 * full probe for it charged a second spawn on `--version` — whose only output
 * is a version string the usage path never looks at — on every refresh, and
 * gave that spawn its own timeout budget to fail inside.
 *
 * @since 0.9.0
 */
export function ReadClaudeAuthentication(
	factory: typeof EngineProcessFactory.Service,
	options: ClaudeProbeOptions,
): Effect.Effect<EngineProbe["authentication"], EngineFailure> {
	return Effect.gen(function* () {
		const [auth_stdout, auth_stderr, auth_exit] = yield* RunClaudeCli(
			factory,
			options,
			["auth", "status"],
			options.auth_timeout_ms,
		);
		const auth_status = Schema.decodeUnknownOption(
			Schema.fromJsonString(Schema.Struct({ loggedIn: Schema.Boolean })),
		)(new TextDecoder().decode(auth_stdout));
		const unknown = auth_status._tag === "None" || auth_exit.code !== 0;
		const authenticated = !unknown && auth_status._tag === "Some" && auth_status.value.loggedIn;

		return unknown
			? {
					reason:
						new TextDecoder().decode(auth_stderr).trim() ||
						"Claude auth status is unavailable",
					state: "unknown" as const,
				}
			: { state: authenticated ? ("authenticated" as const) : ("unauthenticated" as const) };
	});
}

export function MakeClaudeProbe(
	factory: typeof EngineProcessFactory.Service,
	options: ClaudeProbeOptions,
): Effect.Effect<EngineProbe, EngineFailure> {
	const run = (args: ReadonlyArray<string>, timeout_ms: number) =>
		RunClaudeCli(factory, options, args, timeout_ms);

	return Effect.gen(function* () {
		const [version_stdout, version_stderr, version_exit] = yield* run(
			["--version"],
			options.version_timeout_ms,
		);
		if (version_exit.code !== 0)
			return yield* Effect.fail(
				new EngineUnavailableError({
					engine_id: "claude",
					message: `Claude --version exited ${String(version_exit.code)}: ${new TextDecoder().decode(version_stderr)}`,
				}),
			);
		const version = new TextDecoder()
			.decode(version_stdout)
			.match(/\b\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?\b/)?.[0];
		if (version === undefined)
			return yield* Effect.fail(
				new EngineUnavailableError({
					engine_id: "claude",
					message: "Claude --version did not contain a semantic version",
				}),
			);
		const authentication = yield* ReadClaudeAuthentication(factory, options);
		return {
			authentication,
			capabilities: options.descriptor.capabilities,
			descriptor: options.descriptor,
			metadata: {},
			ready: authentication.state === "authenticated",
			version,
		};
	});
}
