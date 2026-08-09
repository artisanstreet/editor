import { Effect, Schema, Scope } from "effect";

import {
	type EngineDescriptor,
	type EngineFailure,
	type EngineNativeContinuationDecision,
	type EngineNativeContinuationInput,
	type EngineProbe,
	type EngineProbeInput,
	EngineProbeTimeoutError,
	EngineProcessError,
	EngineProtocolError,
	EngineUnavailableError,
} from "../engine";
import {
	type CodexAppServerSession,
	type CodexAppServerSessionFailure,
} from "./app-server-session";
import { CodexProcessFactory, type CodexProcessSpawnInput } from "./process";
import { CodexTransportMetadata } from "./protocol";

export const CodexAccountReadSchema = Schema.Struct({
	account: Schema.NullOr(
		Schema.Union([
			Schema.Struct({ type: Schema.Literal("apiKey") }),
			Schema.Struct({
				email: Schema.NullOr(Schema.String),
				planType: Schema.Unknown,
				type: Schema.Literal("chatgpt"),
			}),
			Schema.Struct({
				credentialSource: Schema.Unknown,
				type: Schema.Literal("amazonBedrock"),
			}),
		]),
	),
	requiresOpenaiAuth: Schema.Boolean,
});
const ModelListSchema = Schema.Struct({
	data: Schema.Array(Schema.Struct({ id: Schema.String })),
	nextCursor: Schema.optional(Schema.NullOr(Schema.String)),
});

const MapSessionFailure = <A, R>(
	effect: Effect.Effect<A, CodexAppServerSessionFailure, R>,
): Effect.Effect<A, EngineProcessError | EngineProtocolError, R> =>
	effect.pipe(
		Effect.mapError((error) => {
			if (error instanceof EngineProcessError) return error;

			const message = (() => {
				switch (error._tag) {
					case "CodexAppServerClosedError":
						return `Codex app-server closed: ${error.reason}`;
					case "CodexAppServerConfigurationError":
						return `Invalid Codex app-server option ${error.option}: ${error.value}`;
					case "CodexAppServerProtocolError":
					case "CodexAppServerSerializationError":
						return error.message;
					case "CodexAppServerRequestTimeoutError":
						return `Codex ${error.method} request ${error.id} timed out after ${error.timeout_ms}ms`;
					case "CodexAppServerResponseError":
						return `Codex ${error.method} request ${error.id} failed (${error.error.code}): ${error.error.message}`;
				}
			})();

			return new EngineProtocolError({ engine_id: "codex", message });
		}),
	);

const ReadBoundedStream = (
	stream: AsyncIterable<Uint8Array>,
	channel: "stderr" | "stdout",
	max_bytes: number,
) =>
	Effect.tryPromise({
		try: async () => {
			const output = new Uint8Array(max_bytes);
			let length = 0;

			for await (const chunk of stream) {
				if (length + chunk.length > max_bytes) {
					throw new EngineProtocolError({
						engine_id: "codex",
						message: `Codex --version ${channel} exceeded ${max_bytes} bytes`,
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

const RunVersionProbe = (
	factory: typeof CodexProcessFactory.Service,
	spawn_input: CodexProcessSpawnInput,
	timeout_ms: number,
) =>
	Effect.gen(function* () {
		const handle = yield* factory.Spawn(spawn_input);

		/** A version probe is one-shot; unlike an app-server session, it must observe EOF. */
		yield* handle.EndInput;

		return yield* Effect.all(
			[
				ReadBoundedStream(handle.Stdout, "stdout", 64 * 1_024),
				ReadBoundedStream(handle.Stderr, "stderr", 64 * 1_024),
				handle.Exit,
			],
			{ concurrency: "unbounded" },
		).pipe(
			Effect.ensuring(handle.Close),
			Effect.flatMap(([stdout, stderr, process_exit]) => {
				if (process_exit.code === 0) return Effect.succeed(stdout);

				const detail = new TextDecoder().decode(stderr).trim();

				return Effect.fail(
					new EngineUnavailableError({
						engine_id: "codex",
						message: `Codex --version exited with code ${String(process_exit.code)}${detail.length === 0 ? "" : `: ${detail}`}`,
					}),
				);
			}),
		);
	}).pipe(
		Effect.timeoutOrElse({
			duration: timeout_ms,
			orElse: () =>
				Effect.fail(
					new EngineProbeTimeoutError({
						engine_id: "codex",
						phase: "version",
						timeout_ms,
					}),
				),
		}),
	);

const ParseCodexVersion = (stdout: Uint8Array) => {
	const version = new TextDecoder().decode(stdout).match(/\b(\d+\.\d+\.\d+)\b/)?.[1];

	return version
		? Effect.succeed(version)
		: Effect.fail(
				new EngineUnavailableError({
					engine_id: "codex",
					message: "Codex --version did not contain a semantic version",
				}),
			);
};

const CompareSemanticVersions = (left: string, right: string) => {
	const left_parts = left.split(".").map(Number);
	const right_parts = right.split(".").map(Number);

	for (let index = 0; index < 3; index += 1) {
		const difference = (left_parts[index] ?? 0) - (right_parts[index] ?? 0);
		if (difference !== 0) return difference;
	}

	return 0;
};

const ValidateCodexTransportVersion = (version: string) =>
	CompareSemanticVersions(version, CodexTransportMetadata.minimum_cli_version) >= 0
		? Effect.void
		: Effect.fail(
				new EngineProtocolError({
					engine_id: "codex",
					message: `Codex ${version} is older than minimum supported ${CodexTransportMetadata.minimum_cli_version}`,
				}),
			);

const ParseCodexContinuationVersion = (stdout: Uint8Array) => {
	const version = new TextDecoder()
		.decode(stdout)
		.match(/\b(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?)\b/)?.[1];

	return version === CodexTransportMetadata.continuation_cli_version
		? Effect.succeed(version)
		: Effect.fail(
				new EngineProtocolError({
					engine_id: "codex",
					message: `Codex continuation is verified only for ${CodexTransportMetadata.continuation_cli_version}; found ${version ?? "an unknown version"}`,
				}),
			);
};

export interface CodexProbeOptions {
	readonly descriptor: EngineDescriptor;
	readonly executable: string;
	readonly executable_args: ReadonlyArray<string>;
	readonly factory: typeof CodexProcessFactory.Service;
	readonly initialize_timeout_ms: number;
	readonly OpenSession: () => Effect.Effect<
		CodexAppServerSession,
		EngineProcessError | EngineProtocolError,
		Scope.Scope
	>;
	readonly version_timeout_ms: number;
}

/** Builds the bounded, non-billable readiness and continuation checks. */
export const MakeCodexProbe = (options: CodexProbeOptions) => {
	const ReadVersion = () =>
		RunVersionProbe(
			options.factory,
			{ args: [...options.executable_args, "--version"], command: options.executable },
			options.version_timeout_ms,
		);
	const Probe = (input: EngineProbeInput): Effect.Effect<EngineProbe, EngineFailure> =>
		Effect.scoped(
			Effect.gen(function* () {
				const version = yield* ReadVersion().pipe(Effect.flatMap(ParseCodexVersion));

				yield* ValidateCodexTransportVersion(version);

				const probe_result = yield* Effect.scoped(
					Effect.gen(function* () {
						const session = yield* options.OpenSession();
						const initialized = yield* MapSessionFailure(
							session.Handshake({
								client_name: input.client_name ?? "artisan-editor",
								client_version: input.client_version ?? "0.3.0",
							}),
						);
						const account_response = yield* MapSessionFailure(
							session.Request("account/read", {}),
						);
						const account = yield* Schema.decodeUnknownEffect(CodexAccountReadSchema)(
							account_response.result,
						).pipe(
							Effect.mapError(
								() =>
									new EngineProtocolError({
										engine_id: "codex",
										message: "Codex account/read returned an invalid result",
									}),
							),
						);

						return { account, initialized };
					}),
				).pipe(
					Effect.timeoutOrElse({
						duration: options.initialize_timeout_ms,
						orElse: () =>
							Effect.fail(
								new EngineProbeTimeoutError({
									engine_id: "codex",
									phase: "initialize",
									timeout_ms: options.initialize_timeout_ms,
								}),
							),
					}),
				);
				const account_type = probe_result.account.account?.type;
				const authentication: EngineProbe["authentication"] =
					account_type !== undefined
						? { state: "authenticated", reason: account_type }
						: {
								state: "unauthenticated",
								reason: probe_result.account.requiresOpenaiAuth
									? "OpenAI authentication required"
									: "No ChatGPT or API-key account is active",
							};

				return {
					authentication,
					capabilities: options.descriptor.capabilities,
					descriptor: options.descriptor,
					metadata: {
						codex_home: probe_result.initialized.result.codexHome,
						platform_family: probe_result.initialized.result.platformFamily,
						platform_os: probe_result.initialized.result.platformOs,
						user_agent: probe_result.initialized.result.userAgent,
					},
					ready: authentication.state === "authenticated",
					version,
				};
			}),
		);
	const CheckNativeContinuation = (
		input: EngineNativeContinuationInput,
	): Effect.Effect<EngineNativeContinuationDecision, EngineFailure> =>
		Effect.scoped(
			Effect.gen(function* () {
				yield* ReadVersion().pipe(Effect.flatMap(ParseCodexContinuationVersion));

				if (input.target_model === undefined) {
					return {
						reason: "Codex native continuation requires an explicit target model",
						state: "incompatible",
					};
				}

				const session = yield* options.OpenSession();
				yield* MapSessionFailure(
					session.Handshake({ client_name: "artisan-editor", client_version: "0.7.0" }),
				);
				let cursor: string | undefined;
				const visited_cursors = new Set<string>();

				for (let page = 0; page < 16; page += 1) {
					const response = yield* MapSessionFailure(
						session.Request("model/list", cursor === undefined ? {} : { cursor }),
					);
					const models = yield* Schema.decodeUnknownEffect(ModelListSchema)(
						response.result,
					).pipe(
						Effect.mapError(
							() =>
								new EngineProtocolError({
									engine_id: "codex",
									message: "Codex model/list returned an invalid result",
								}),
						),
					);

					if (models.data.some((model) => model.id === input.target_model)) {
						return { state: "compatible" };
					}

					if (models.nextCursor === undefined || models.nextCursor === null) break;
					if (visited_cursors.has(models.nextCursor)) {
						return {
							reason: "Codex model/list returned a repeated page cursor",
							state: "incompatible",
						};
					}
					visited_cursors.add(models.nextCursor);
					cursor = models.nextCursor;
				}

				return {
					reason: `Codex does not currently advertise model ${input.target_model}`,
					state: "incompatible",
				};
			}),
		);

	return {
		CheckNativeContinuation,
		Probe,
		ReadTransportVersion: ReadVersion().pipe(
			Effect.flatMap(ParseCodexVersion),
			Effect.tap(ValidateCodexTransportVersion),
		),
	};
};
