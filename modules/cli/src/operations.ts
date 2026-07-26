import { readFile, stat } from "node:fs/promises";
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
import { ForgeProfileStore } from "./profile";

export class ForgeOperationsError extends Data.TaggedError("ForgeOperationsError")<{
	readonly cause?: unknown;
	readonly code: "invalid" | "unavailable";
}> {}

export interface ForgeDoctorCheck {
	readonly name: "profile" | "config" | "roots" | "artifacts" | "codex" | "autostart" | "live";
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
		readonly Doctor: (
			profile: string,
		) => Effect.Effect<ForgeDoctorReport, ForgeOperationsError, ChildProcessSpawner>;
		readonly FollowLogs: (profile: string) => Stream.Stream<string, ForgeOperationsError>;
		readonly ReadLogs: (
			profile: string,
			lines: number,
		) => Effect.Effect<ReadonlyArray<string>, ForgeOperationsError>;
	}
>()("Artisan/ForgeOperations") {}

export const ClampLogLines = (lines: number) => Math.max(1, Math.min(10_000, lines));

const ReadText = (path: string) =>
	Effect.tryPromise({
		try: () => readFile(path, "utf8"),
		catch: (cause) => new ForgeOperationsError({ cause, code: "unavailable" }),
	});

const TailLines = (text: string, lines: number) =>
	text.split(/\r?\n/).filter(Boolean).slice(-ClampLogLines(lines));

interface LogFollowerState {
	readonly decoder: StringDecoder;
	readonly identity: string;
	readonly initialized: boolean;
	readonly offset: number;
}

const FollowFile = (path: string) =>
	Stream.unwrap(
		Effect.gen(function* () {
			const state = yield* SynchronizedRef.make<LogFollowerState>({
				decoder: new StringDecoder("utf8"),
				identity: "",
				initialized: false,
				offset: 0,
			});
			const Poll = Effect.gen(function* () {
				const result = yield* Effect.tryPromise({
					try: async () => ({ content: await readFile(path), info: await stat(path) }),
					catch: (cause) => cause,
				}).pipe(Effect.result);
				if (Result.isFailure(result)) {
					if ((result.failure as NodeJS.ErrnoException).code !== "ENOENT") {
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
						offset: 0,
					});
					yield* Effect.sleep("200 millis");
					return "";
				}
				const chunk = yield* SynchronizedRef.modifyEffect(state, (current) =>
					Effect.sync(() => {
						const next_identity = `${result.success.info.dev}:${result.success.info.ino}`;
						const reset =
							(current.initialized &&
								(next_identity !== current.identity ||
									result.success.info.size < current.offset)) ||
							!current.initialized;
						const offset = !current.initialized
							? result.success.info.size
							: reset
								? 0
								: current.offset;
						const decoder = reset ? new StringDecoder("utf8") : current.decoder;
						const decoded =
							result.success.info.size > offset
								? decoder.write(result.success.content.subarray(offset))
								: "";
						return [
							decoded,
							{
								decoder,
								identity: next_identity,
								initialized: true,
								offset: result.success.content.length,
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
		const store = yield* ForgeProfileStore;
		const lifecycle = yield* ForgeLifecycle;
		const autostart = yield* ForgeAutostart;
		const file_system = yield* FileSystem.FileSystem;
		const artifact = yield* ResolveForgeArtifact;
		const executable_path = yield* Config.string("PATH").pipe(Config.withDefault(""));
		return ForgeOperations.of({
			Doctor: (profile) =>
				Effect.gen(function* () {
					const config_option = yield* store.Load(profile).pipe(Effect.option);
					if (Option.isNone(config_option))
						return {
							healthy: false,
							checks: [
								{
									detail: "Profile is missing or invalid",
									name: "profile",
									state: "error",
								},
							] as const,
						};
					const config = config_option.value;
					const live = yield* lifecycle
						.Status(profile)
						.pipe(
							Effect.mapError(
								(cause) => new ForgeOperationsError({ cause, code: "unavailable" }),
							),
						);
					const autostart_state = yield* autostart
						.Status({ profile })
						.pipe(
							Effect.catch(() => Effect.succeed({ state: "unsupported" as const })),
						);
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
					const roots_available = (yield* Effect.forEach(config.project_roots, (path) =>
						file_system.exists(path).pipe(Effect.orElseSucceed(() => false)),
					)).every(Boolean);
					const checks: ReadonlyArray<ForgeDoctorCheck> = [
						{ detail: "Profile loaded", name: "profile", state: "ok" },
						{
							detail: `Loopback listener ${config.listen_host}`,
							name: "config",
							state: "ok",
						},
						{
							detail: roots_available
								? "Project roots are available"
								: "A project root is unavailable",
							name: "roots",
							state: roots_available ? "ok" : "error",
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
				}),
			FollowLogs: (profile) =>
				Stream.unwrap(
					store.Paths(profile).pipe(
						Effect.map((paths) => FollowFile(paths.log_path)),
						Effect.mapError(
							(cause) => new ForgeOperationsError({ cause, code: "invalid" }),
						),
					),
				),
			ReadLogs: (profile, lines) =>
				store.Paths(profile).pipe(
					Effect.flatMap((paths) => ReadText(paths.log_path)),
					Effect.map((text) => TailLines(text, lines)),
					Effect.mapError((cause) =>
						cause instanceof ForgeOperationsError
							? cause
							: new ForgeOperationsError({ cause, code: "invalid" }),
					),
				),
		});
	}),
);
