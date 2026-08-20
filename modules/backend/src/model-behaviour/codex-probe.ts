import { Buffer } from "node:buffer";
import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Context, Data, Duration, Effect, Layer, Schema } from "effect";

import { ProcessRunner } from "../git/process-runner";
import { codex_auto_compaction_native_key } from "./codex-config";

/** Reports an installed Codex binary and whether it recognizes the owned mapping. */
export interface CodexModelBehaviourProbeAvailable {
	readonly installed_version: string;
	readonly mapping_available: boolean;
	readonly type: "available";
}

/** Explains why Artisan cannot safely decide whether the mapping is available. */
export interface CodexModelBehaviourProbeUnavailable {
	readonly reason: "invalid_output" | "process_failed";
	readonly type: "unavailable";
}

/** Represents the feature-probed Codex Model Behaviour capability. */
export type CodexModelBehaviourProbeResult =
	| CodexModelBehaviourProbeAvailable
	| CodexModelBehaviourProbeUnavailable;

/** Configures the isolated Codex capability probe. */
export interface CodexModelBehaviourProbeOptions {
	readonly command?: string;
	readonly cwd: string;
	/** Production supplies the managed toolchain resolver through this Layer. */
	readonly executable?: Layer.Layer<CodexModelBehaviourExecutable>;
	readonly temporary_directory?: string;
	/** Bounds all isolated doctor processes and their temporary directories together. */
	readonly timeout?: Duration.Input;
}

export class CodexModelBehaviourExecutableUnavailable extends Data.TaggedError(
	"CodexModelBehaviourExecutableUnavailable",
)<{
	readonly cause?: unknown;
}> {}

/** Resolves the exact Codex executable used by the isolated capability probe. */
export class CodexModelBehaviourExecutable extends Context.Service<
	CodexModelBehaviourExecutable,
	{
		readonly Resolve: Effect.Effect<string, CodexModelBehaviourExecutableUnavailable>;
	}
>()("Artisan/CodexModelBehaviourExecutable") {}

export const make_codex_model_behaviour_executable_layer = (
	Resolve: typeof CodexModelBehaviourExecutable.Service.Resolve,
) => Layer.succeed(CodexModelBehaviourExecutable, CodexModelBehaviourExecutable.of({ Resolve }));

/** Probes installed Codex behavior without reading or changing the user's config. */
export class CodexModelBehaviourProbe extends Context.Service<
	CodexModelBehaviourProbe,
	{
		readonly Probe: Effect.Effect<CodexModelBehaviourProbeResult>;
	}
>()("Artisan/CodexModelBehaviourProbe") {}

const CodexDoctorReport = Schema.Struct({
	checks: Schema.Struct({
		"config.load": Schema.Struct({ status: Schema.String }),
	}),
	codexVersion: Schema.NonEmptyString,
});

function ParseDoctorReport(stdout: Uint8Array) {
	return Schema.decodeUnknownEffect(Schema.fromJsonString(CodexDoctorReport))(
		Buffer.from(stdout).toString("utf8"),
	).pipe(Effect.option);
}

function AcquireProbeDirectory(temporary_directory: string) {
	return Effect.tryPromise(() =>
		fs.mkdtemp(join(temporary_directory, "artisan-codex-model-behaviour-")),
	);
}

function ReleaseProbeDirectory(directory: string) {
	return Effect.tryPromise(() => fs.rm(directory, { force: true, recursive: true })).pipe(
		Effect.ignore,
	);
}

/** Builds the shell-free Codex capability probe used by desktop composition. */
export function make_codex_model_behaviour_probe_layer(options: CodexModelBehaviourProbeOptions) {
	const temporary_directory = options.temporary_directory ?? tmpdir();
	const timeout = options.timeout ?? "15 seconds";
	const executable =
		options.executable ??
		make_codex_model_behaviour_executable_layer(Effect.succeed(options.command ?? "codex"));

	return Layer.effect(
		CodexModelBehaviourProbe,
		Effect.gen(function* () {
			const process_runner = yield* ProcessRunner;
			const command = yield* CodexModelBehaviourExecutable;
			const RunProbe = (command: string, directory: string, content: string) =>
				Effect.gen(function* () {
					yield* Effect.tryPromise(() =>
						fs.writeFile(join(directory, "config.toml"), content, "utf8"),
					);
					const result = yield* process_runner.Run({
						args: ["--strict-config", "doctor", "--json", "--summary"],
						command,
						cwd: options.cwd,
						environment: { CODEX_HOME: directory },
						max_stderr_bytes: 64 * 1024,
						max_stdout_bytes: 2 * 1024 * 1024,
					});

					if (result.exit_code !== 0) {
						return yield* Effect.fail("process_failed" as const);
					}

					if (result.stdout_truncated) {
						return yield* Effect.fail("invalid_output" as const);
					}

					return yield* ParseDoctorReport(result.stdout).pipe(
						Effect.flatMap((report) =>
							report._tag === "None"
								? Effect.fail("invalid_output" as const)
								: Effect.succeed(report.value),
						),
					);
				});

			const Probe = Effect.gen(function* () {
				const resolved_command = yield* command.Resolve;
				const RunIsolated = (content: string) =>
					Effect.acquireUseRelease(
						AcquireProbeDirectory(temporary_directory),
						(directory) => RunProbe(resolved_command, directory, content),
						ReleaseProbeDirectory,
					);
				const [valid, invalid] = yield* Effect.all(
					[
						RunIsolated(`${codex_auto_compaction_native_key} = 250000\n`),
						RunIsolated(
							`${codex_auto_compaction_native_key} = "artisan-invalid-probe"\n`,
						),
					],
					{ concurrency: "unbounded" },
				);

				return {
					installed_version: valid.codexVersion,
					mapping_available:
						valid.checks["config.load"].status === "ok" &&
						invalid.checks["config.load"].status === "fail",
					type: "available" as const,
				};
			}).pipe(
				Effect.timeoutOrElse({
					duration: timeout,
					orElse: () => Effect.fail("process_failed" as const),
				}),
				Effect.catch((reason) =>
					Effect.succeed({
						reason:
							reason === "invalid_output"
								? ("invalid_output" as const)
								: ("process_failed" as const),
						type: "unavailable" as const,
					}),
				),
			);

			return { Probe };
		}),
	).pipe(Layer.provide(executable));
}
