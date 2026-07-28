import { delimiter, join } from "node:path";
import { StringDecoder } from "node:string_decoder";

import {
	Context,
	Data,
	Effect,
	Layer,
	Option,
	Result,
	Schedule,
	Stream,
	SynchronizedRef,
	FileSystem,
	Config,
} from "effect";
import { ChildProcessSpawner } from "effect/unstable/process/ChildProcessSpawner";

import { ForgeAutostart } from "./adapters";
import { ResolveForgeArtifact } from "./forge-adapter";
import { ForgeLifecycle } from "./lifecycle";
import { ForgeInstanceStore } from "./instance";

export class ForgeOperationsError extends Data.TaggedError("ForgeOperationsError")<{
	readonly cause?: unknown;
	readonly code: "invalid" | "unavailable";
}> {}

export interface ForgeDoctorCheck {
	readonly name: "instance" | "config" | "artifacts" | "codex" | "autostart" | "live";
	readonly state: "ok" | "warning" | "error" | "unsupported";
	readonly detail: string;
}

export interface ForgeDoctorReport {
	readonly healthy: boolean;
	readonly checks: ReadonlyArray<ForgeDoctorCheck>;
}

export class ForgeOperations extends Context.Service<
	ForgeOperations,
	{
		readonly Doctor: () => Effect.Effect<
			ForgeDoctorReport,
			ForgeOperationsError,
			ChildProcessSpawner
		>;
		readonly Repair: () => Effect.Effect<
			ForgeDoctorReport,
			ForgeOperationsError,
			ChildProcessSpawner
		>;
		readonly FollowLogs: () => Stream.Stream<string, ForgeOperationsError>;
		readonly ReadLogs: (
			lines: number,
		) => Effect.Effect<ReadonlyArray<string>, ForgeOperationsError>;
	}
>()("Artisan/ForgeOperations") {}

export const ClampLogLines = (lines: number) => Math.max(1, Math.min(10_000, lines));

/** A foreground tail is deliberately limited even if a prior Forge produced a malformed large log. */
export const MAX_LOG_TAIL_READ_BYTES = 1024 * 1024;

/** One follow poll may read at most this much new output before yielding to the next poll. */
export const MAX_LOG_FOLLOW_READ_BYTES = 64 * 1024;

const TailLines = (text: string, lines: number) =>
	text.split(/\r?\n/).filter(Boolean).slice(-ClampLogLines(lines));

const ConcatenateBytes = (chunks: ReadonlyArray<Uint8Array>) => {
	const length = chunks.reduce((total, chunk) => total + chunk.byteLength, 0);
	const bytes = new Uint8Array(length);
	let offset = 0;
	for (const chunk of chunks) {
		bytes.set(chunk, offset);
		offset += chunk.byteLength;
	}
	return bytes;
};

const IsNotFound = (cause: unknown) =>
	typeof cause === "object" &&
	cause !== null &&
	"_tag" in cause &&
	cause._tag === "PlatformError" &&
	"reason" in cause &&
	typeof cause.reason === "object" &&
	cause.reason !== null &&
	"_tag" in cause.reason &&
	cause.reason._tag === "NotFound";

const ReadLogTail = (file_system: FileSystem.FileSystem, path: string) =>
	Effect.gen(function* () {
		const info = yield* file_system.stat(path);
		const cap = BigInt(MAX_LOG_TAIL_READ_BYTES);
		const size = BigInt(info.size);
		const offset = size > cap ? size - cap : 0n;
		const chunks = yield* file_system
			.stream(path, {
				bytesToRead: cap,
				offset,
			})
			.pipe(Stream.runCollect);
		return new TextDecoder().decode(ConcatenateBytes(chunks));
	});

interface LogFollowerState {
	readonly decoder: StringDecoder;
	readonly identity: string;
	readonly initialized: boolean;
	readonly offset: bigint;
}

export const FollowForgeLogFile = (path: string) => (file_system: FileSystem.FileSystem) =>
	Stream.unwrap(
		Effect.gen(function* () {
			const state = yield* SynchronizedRef.make<LogFollowerState>({
				decoder: new StringDecoder("utf8"),
				identity: "",
				initialized: false,
				offset: 0n,
			});
			const Poll = Effect.gen(function* () {
				const result = yield* file_system.stat(path).pipe(Effect.result);
				if (Result.isFailure(result)) {
					if (!IsNotFound(result.failure)) {
						return yield* Effect.fail(
							new ForgeOperationsError({
								cause: result.failure,
								code: "unavailable",
							}),
						);
					}
					yield* SynchronizedRef.set(state, {
						decoder: new StringDecoder("utf8"),
						identity: "",
						initialized: true,
						offset: 0n,
					});
					yield* Effect.sleep("200 millis");
					return "";
				}
				const chunk = yield* SynchronizedRef.modifyEffect(state, (current) =>
					Effect.gen(function* () {
						const size = BigInt(result.success.size);
						const next_identity = `${result.success.dev}:${Option.getOrElse(result.success.ino, () => 0)}`;
						const reset =
							(current.initialized &&
								(next_identity !== current.identity || size < current.offset)) ||
							!current.initialized;
						const offset = !current.initialized ? size : reset ? 0n : current.offset;
						const decoder = reset ? new StringDecoder("utf8") : current.decoder;
						const available = size - offset;
						const bytes_to_read =
							available > 0n
								? available > BigInt(MAX_LOG_FOLLOW_READ_BYTES)
									? BigInt(MAX_LOG_FOLLOW_READ_BYTES)
									: available
								: 0n;
						const chunks =
							bytes_to_read === 0n
								? []
								: yield* file_system
										.stream(path, {
											bytesToRead: BigInt(bytes_to_read),
											offset,
										})
										.pipe(Stream.runCollect);
						const bytes = ConcatenateBytes(chunks);
						const decoded = bytes.byteLength === 0 ? "" : decoder.write(bytes);
						return [
							decoded,
							{
								decoder,
								identity: next_identity,
								initialized: true,
								offset: offset + BigInt(bytes.byteLength),
							},
						] as const;
					}),
				);
				yield* Effect.sleep("200 millis");
				return chunk;
			});
			return Stream.fromEffect(Poll).pipe(
				Stream.repeat(Schedule.forever),
				Stream.filter((chunk) => chunk.length > 0),
				Stream.mapError((cause) =>
					cause instanceof ForgeOperationsError
						? cause
						: new ForgeOperationsError({ cause, code: "unavailable" }),
				),
			);
		}),
	);

const CodexExecutableAvailable = (file_system: FileSystem.FileSystem, path: string) =>
	Effect.gen(function* () {
		const names = ["codex.exe", "codex.cmd", "codex.bat", "codex"];
		const candidates = path
			.split(delimiter)
			.filter(Boolean)
			.flatMap((directory) => names.map((name) => join(directory, name)));
		const availability = yield* Effect.forEach(candidates, (candidate) =>
			file_system.exists(candidate).pipe(Effect.orElseSucceed(() => false)),
		);
		return availability.some(Boolean);
	});

/** Diagnostics deliberately inspect public config and state only, never secrets. */
export const make_forge_operations_layer = Layer.effect(
	ForgeOperations,
	Effect.gen(function* () {
		const store = yield* ForgeInstanceStore;
		const lifecycle = yield* ForgeLifecycle;
		const autostart = yield* ForgeAutostart;
		const file_system = yield* FileSystem.FileSystem;
		const artifact = yield* ResolveForgeArtifact;
		const executable_path = yield* Config.string("PATH").pipe(Config.withDefault(""));
		const Doctor = () =>
			Effect.gen(function* () {
				const config_option = yield* store.Load().pipe(Effect.option);
				const live = Option.isSome(config_option)
					? yield* lifecycle
							.Status()
							.pipe(
								Effect.mapError(
									(cause) =>
										new ForgeOperationsError({ cause, code: "unavailable" }),
								),
							)
					: { state: "missing" as const };
				const autostart_state = yield* autostart
					.Status()
					.pipe(Effect.catch(() => Effect.succeed({ state: "unsupported" as const })));
				const artifacts_available = (yield* Effect.forEach(
					[
						artifact.executable_path,
						artifact.host_entry_path,
						artifact.migrations_path,
						artifact.native_runtime_path,
						artifact.node_executable_path,
						artifact.static_frontend_root,
					],
					(path) => file_system.exists(path).pipe(Effect.orElseSucceed(() => false)),
				)).every(Boolean);
				const codex_available = yield* CodexExecutableAvailable(
					file_system,
					executable_path,
				);
				const checks: ReadonlyArray<ForgeDoctorCheck> = [
					{
						detail: Option.isSome(config_option)
							? "Forge instance loaded"
							: "Forge is not configured in this home; run ae setup",
						name: "instance",
						state: Option.isSome(config_option) ? "ok" : "error",
					},
					{
						detail: Option.isSome(config_option)
							? `Loopback listener ${config_option.value.listen_host}`
							: "Configuration cannot be checked without a configured Forge",
						name: "config",
						state: Option.isSome(config_option) ? "ok" : "error",
					},
					{
						detail: artifacts_available
							? "Forge runtime artifacts are available"
							: "Forge runtime artifacts are incomplete",
						name: "artifacts",
						state: artifacts_available ? "ok" : "error",
					},
					{
						detail: codex_available
							? "Codex executable is discoverable"
							: "Codex executable is not discoverable on PATH",
						name: "codex",
						state: codex_available ? "ok" : "error",
					},
					{
						detail: autostart_state.state,
						name: "autostart",
						state: autostart_state.state === "unsupported" ? "unsupported" : "ok",
					},
					{
						detail: live.state,
						name: "live",
						state: live.state === "running" ? "ok" : "warning",
					},
				];
				return { checks, healthy: !checks.some((check) => check.state === "error") };
			});

		return ForgeOperations.of({
			Doctor,
			FollowLogs: () =>
				Stream.unwrap(
					store.Paths().pipe(
						Effect.map((paths) => FollowForgeLogFile(paths.log_path)(file_system)),
						Effect.mapError(
							(cause) => new ForgeOperationsError({ cause, code: "invalid" }),
						),
					),
				),
			ReadLogs: (lines) =>
				store.Paths().pipe(
					Effect.flatMap((paths) => ReadLogTail(file_system, paths.log_path)),
					Effect.map((text) => TailLines(text, lines)),
					Effect.mapError((cause) =>
						cause instanceof ForgeOperationsError
							? cause
							: new ForgeOperationsError({ cause, code: "invalid" }),
					),
				),
			Repair: Doctor,
		});
	}),
);
