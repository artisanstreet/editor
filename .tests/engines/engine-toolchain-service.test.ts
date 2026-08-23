import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import * as NodeFileSystem from "@effect/platform-node-shared/NodeFileSystem";
import * as NodePath from "@effect/platform-node-shared/NodePath";
import { describe, expect, it } from "@effect/vitest";
import { Cause, Effect, Exit, FileSystem, Layer, Stream } from "effect";

import {
	type EngineDistribution,
	type EngineProcessSpawnInput,
	EngineProcessFactory,
	EngineToolchain,
	make_engine_toolchain_layer,
	ToolchainReleaseHttp,
} from "@artisan/engines";

const bytes = (value: string) => new TextEncoder().encode(value);
const digest = (value: string) => createHash("sha256").update(value).digest("hex");

interface ReleaseFixture {
	readonly body: string;
	readonly chunks?: ReadonlyArray<string>;
	readonly expected_sha256?: string;
	readonly size_bytes?: number;
}

interface ServiceFixtureOptions {
	readonly fail_credential_copy?: boolean;
	readonly health_output_prefix?: string;
	readonly releases: Readonly<Record<string, ReleaseFixture>>;
}

interface ServiceFixture {
	readonly FailNextHealthChecks: (version: string, count?: number) => void;
	readonly layer: Layer.Layer<EngineToolchain>;
	readonly spawns: Array<EngineProcessSpawnInput>;
}

const MakeService = (root: string, options: ServiceFixtureOptions): ServiceFixture => {
	const distribution: EngineDistribution = {
		credential_files: [".credentials.json"],
		display_name: "Claude",
		engine_id: "claude",
		home_environment_variable: "CLAUDE_CONFIG_DIR",
		login_args: ["auth", "login"],
		LatestVersion: Effect.succeed("2.0.0"),
		ResolveRelease: (version) =>
			Effect.gen(function* () {
				const release = options.releases[version];
				if (release === undefined) return yield* Effect.die(`missing fixture ${version}`);
				return {
					binary: "claude",
					sha256: release.expected_sha256 ?? digest(release.body),
					...(release.size_bytes === undefined ? {} : { size_bytes: release.size_bytes }),
					url: `https://example.invalid/${version}`,
					version,
				};
			}),
		vendor_home_directory: ".claude",
	};
	const health_failures = new Map<string, number>();
	const spawns: Array<EngineProcessSpawnInput> = [];
	const factory = EngineProcessFactory.of({
		Spawn: (input) =>
			Effect.sync(() => {
				spawns.push(input);
				const version =
					Object.keys(options.releases).find((candidate) => input.command.includes(candidate)) ??
					"";
				const remaining_failures = health_failures.get(version) ?? 0;
				if (remaining_failures > 0) health_failures.set(version, remaining_failures - 1);
				return {
					Close: Effect.void,
					EndInput: Effect.void,
					Exit: Effect.succeed({ code: remaining_failures > 0 ? 1 : 0, signal: null }),
					Kill: () => Effect.void,
					Stderr: (async function* () {})(),
					Stdout: (async function* () {
						yield bytes(`${options.health_output_prefix ?? ""}${version}\n`);
					})(),
					Write: () => Effect.gen(function* () {}),
				};
			}),
	});
	const http = ToolchainReleaseHttp.of({
		Get: () =>
			Effect.gen(function* () {
				return yield* Effect.die("metadata is not used");
			}),
		GetStream: (url) => {
			const release = options.releases[url.split("/").at(-1)!];
			const chunks = release?.chunks ?? (release === undefined ? [] : [release.body]);
			return Stream.fromIterable(chunks.map(bytes));
		},
	});
	const file_system_layer =
		options.fail_credential_copy !== true
			? NodeFileSystem.layer
			: Layer.effect(
					FileSystem.FileSystem,
					Effect.gen(function* () {
						const file_system = yield* FileSystem.FileSystem;
						return FileSystem.FileSystem.of({
							...file_system,
							copyFile: (_source, destination) =>
								file_system.copyFile(join(root, "missing-credential-source"), destination),
						});
					}),
				).pipe(Layer.provide(NodeFileSystem.layer));
	const layer = make_engine_toolchain_layer({
		distributions: [distribution],
		platform: { architecture: "x64", platform: "linux" },
		root,
		user_home_directory: join(root, "vendor"),
	}).pipe(
		Layer.provideMerge(file_system_layer),
		Layer.provideMerge(NodePath.layer),
		Layer.provideMerge(Layer.succeed(EngineProcessFactory, factory)),
		Layer.provideMerge(Layer.succeed(ToolchainReleaseHttp, http)),
	);
	return {
		FailNextHealthChecks: (version, count = 1) => health_failures.set(version, count),
		layer,
		spawns,
	};
};

const WithService = <A>(
	options: ServiceFixtureOptions,
	test: (
		service: typeof EngineToolchain.Service,
		root: string,
		fixture: ServiceFixture,
	) => Effect.Effect<A>,
) =>
	Effect.gen(function* () {
		const root = yield* Effect.promise(() => mkdtemp(join(tmpdir(), "artisan-toolchain-")));
		const fixture = MakeService(root, options);
		return yield* Effect.scoped(
			Effect.gen(function* () {
				const service = yield* EngineToolchain;
				return yield* test(service, root, fixture);
			}).pipe(Effect.provide(fixture.layer)),
		).pipe(Effect.ensuring(Effect.promise(() => rm(root, { force: true, recursive: true }))));
	});

const ReadStateText = (root: string) =>
	Effect.promise(() => readFile(join(root, "claude", "state.json"), "utf8"));

const ReadState = (root: string) =>
	Effect.gen(function* () {
		return JSON.parse(yield* ReadStateText(root)) as {
			readonly active: {
				readonly binary: string;
				readonly directory: string;
				readonly version: string;
			};
			readonly previous?: {
				readonly binary: string;
				readonly directory: string;
				readonly version: string;
			};
		};
	});

const FailureFrom = (exit: Exit.Exit<unknown, unknown>) =>
	Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;

const ExpectEmptyStaging = (root: string) =>
	Effect.gen(function* () {
		expect(yield* Effect.promise(() => readdir(join(root, "claude", "staging")))).toEqual([]);
	});

describe("engine toolchain service", () => {
	it.effect("runs a verified staged installer into an owned Hermes generation", () =>
		Effect.acquireUseRelease(
			Effect.promise(() => mkdtemp(join(tmpdir(), "artisan-hermes-toolchain-"))),
			(root) =>
				Effect.gen(function* () {
					const installer = bytes("pinned hermes installer");
					const spawns: Array<EngineProcessSpawnInput> = [];
					const distribution: EngineDistribution = {
						credential_files: [".env", "auth.json"],
						display_name: "Hermes",
						engine_id: "hermes",
						home_environment_variable: "HERMES_HOME",
						login_args: ["setup"],
						LatestVersion: Effect.succeed("0.20.5"),
						recommended_version: "0.20.5",
						ResolveRelease: () =>
							Effect.succeed({
								artifact_kind: "staged-installer" as const,
								binary: "hermes-agent/bin/hermes.exe",
								commit: "a".repeat(40),
								installer_sha256: digest("pinned hermes installer"),
								stages: ["repository", "bootstrap-marker"],
								url: "https://example.invalid/install.ps1",
								version: "0.20.5",
							}),
						vendor_home_directory: "AppData/Local/hermes",
					};
					const factory = EngineProcessFactory.of({
						Spawn: (input) =>
							Effect.promise(async () => {
								spawns.push(input);
								if (input.command === "powershell.exe") {
									const install_index = input.args.indexOf("-InstallDir");
									const stage_index = input.args.indexOf("-Stage");
									const install_directory = input.args[install_index + 1]!;
									const stage = input.args[stage_index + 1]!;
									if (stage === "bootstrap-marker") {
										await mkdir(join(install_directory, "bin"), { recursive: true });
										await writeFile(join(install_directory, "bin", "hermes.exe"), "hermes");
									}
								}
								return {
									Close: Effect.void,
									EndInput: Effect.void,
									Exit: Effect.succeed({ code: 0, signal: null }),
									Kill: () => Effect.void,
									Stderr: (async function* () {})(),
									Stdout: (async function* () {
										yield bytes(
											input.command === "powershell.exe"
												? '{"ok":true}\n'
												: "Hermes Agent v0.20.5\n",
										);
									})(),
									Write: () => Effect.void,
								};
							}),
					});
					const http = ToolchainReleaseHttp.of({
						Get: () => Effect.die("metadata is not used"),
						GetStream: () => Stream.succeed(installer),
					});
					const layer = make_engine_toolchain_layer({
						distributions: [distribution],
						platform: { architecture: "x64", platform: "win32" },
						root,
						user_home_directory: join(root, "vendor"),
					}).pipe(
						Layer.provideMerge(NodeFileSystem.layer),
						Layer.provideMerge(NodePath.layer),
						Layer.provideMerge(Layer.succeed(EngineProcessFactory, factory)),
						Layer.provideMerge(Layer.succeed(ToolchainReleaseHttp, http)),
					);

					return yield* Effect.gen(function* () {
						const service = yield* EngineToolchain;
						const installed = yield* service.Install("hermes");
						const spawn = yield* service.ResolveSpawn("hermes");
						expect(installed.active_version).toBe("0.20.5");
						expect(spawn.executable.replaceAll("\\", "/")).toContain(
							"/hermes-agent/bin/hermes.exe",
						);
						expect(spawn.environment.HERMES_HOME).toBe(join(root, "hermes", "home"));
						expect(
							spawns
								.filter((spawn) => spawn.command === "powershell.exe")
								.map((spawn) => spawn.args[spawn.args.indexOf("-Stage") + 1]),
						).toEqual(["repository", "bootstrap-marker"]);
					}).pipe(Effect.provide(layer));
				}),
			(root) => Effect.promise(() => rm(root, { force: true, recursive: true })),
		),
	);

	it.effect("accepts a labeled version with the conventional v prefix", () =>
		WithService(
			{
				health_output_prefix: "opencode2 v",
				releases: { "0.0.0-beta-17778": { body: "opencode2" } },
			},
			(service) =>
				Effect.gen(function* () {
					const installed = yield* service.Install("claude", "0.0.0-beta-17778").pipe(Effect.orDie);
					expect(installed.active_version).toBe("0.0.0-beta-17778");
				}),
		),
	);

	it.effect("installs immutable generations, rolls back, and preserves managed Claude env", () =>
		WithService(
			{
				releases: {
					"1.0.0": { body: "one" },
					"2.0.0": { body: "two" },
				},
			},
			(service, root, fixture) =>
				Effect.gen(function* () {
					yield* service.Install("claude", "1.0.0").pipe(Effect.orDie);
					yield* service.Install("claude", "2.0.0").pipe(Effect.orDie);
					expect((yield* service.Rollback("claude").pipe(Effect.orDie)).active_version).toBe(
						"1.0.0",
					);
					yield* service.Install("claude", "2.0.0").pipe(Effect.orDie);
					const state = yield* ReadState(root);
					expect(state.active.version).toBe("2.0.0");
					expect(state.active.directory).not.toBe(state.previous?.directory);
					expect(
						fixture.spawns.every(
							(spawn) =>
								(spawn.env as Record<string, string>).CLAUDE_CONFIG_DIR ===
									join(root, "claude", "home") &&
								(spawn.env as Record<string, string>).DISABLE_INSTALLATION_CHECKS === "1" &&
								(spawn.env as Record<string, string>).DISABLE_UPDATES === "1",
						),
					).toBe(true);
				}),
		),
	);

	it.effect(
		"bounds streaming downloads and preserves the active state on verification failure",
		() =>
			WithService(
				{
					releases: {
						"1.0.0": { body: "one" },
						"2.0.0": { body: "overflow", chunks: ["ab", "cd"], size_bytes: 3 },
						"3.0.0": { body: "bad", expected_sha256: digest("expected") },
					},
				},
				(service, root) =>
					Effect.gen(function* () {
						yield* service.Install("claude", "1.0.0").pipe(Effect.orDie);
						const before = yield* ReadStateText(root);
						const overflow = yield* service.Install("claude", "2.0.0").pipe(Effect.exit);
						expect(FailureFrom(overflow)).toMatchObject({
							_tag: "ToolchainHttpFailure",
							code: "body_too_large",
						});
						expect(yield* ReadStateText(root)).toBe(before);
						yield* ExpectEmptyStaging(root);

						const checksum = yield* service.Install("claude", "3.0.0").pipe(Effect.exit);
						expect(FailureFrom(checksum)).toMatchObject({
							_tag: "EngineToolchainVerificationError",
						});
						expect(yield* ReadStateText(root)).toBe(before);
						yield* ExpectEmptyStaging(root);
					}),
			),
	);

	it.effect("repairs an unhealthy active version without retaining it as rollback", () =>
		WithService(
			{
				releases: {
					"1.0.0": { body: "one" },
					"2.0.0": { body: "two" },
				},
			},
			(service, root, fixture) =>
				Effect.gen(function* () {
					yield* service.Install("claude", "1.0.0").pipe(Effect.orDie);
					yield* service.Install("claude", "2.0.0").pipe(Effect.orDie);
					const before = yield* ReadState(root);
					fixture.FailNextHealthChecks("2.0.0");
					yield* service.Install("claude", "2.0.0").pipe(Effect.orDie);
					const after = yield* ReadState(root);
					expect(after.active.version).toBe("2.0.0");
					expect(after.active.directory).not.toBe(before.active.directory);
					expect(after.previous?.directory).toBe(before.previous?.directory);
					expect(after.previous?.version).toBe("1.0.0");
				}),
		),
	);

	it.effect("rejects a corrupt rollback generation without changing the active state", () =>
		WithService(
			{
				releases: {
					"1.0.0": { body: "one" },
					"2.0.0": { body: "two" },
				},
			},
			(service, root) =>
				Effect.gen(function* () {
					yield* service.Install("claude", "1.0.0").pipe(Effect.orDie);
					yield* service.Install("claude", "2.0.0").pipe(Effect.orDie);
					const state = yield* ReadState(root);
					const before = yield* ReadStateText(root);
					yield* Effect.promise(() =>
						writeFile(
							join(root, "claude", "versions", state.previous!.directory, "claude"),
							"corrupt",
						),
					);
					const rollback = yield* service.Rollback("claude").pipe(Effect.exit);
					expect(FailureFrom(rollback)).toMatchObject({
						_tag: "EngineToolchainVerificationError",
					});
					expect(yield* ReadStateText(root)).toBe(before);
				}),
		),
	);

	it.effect("rejects an unhealthy rollback generation without changing active state", () =>
		WithService(
			{
				releases: {
					"1.0.0": { body: "one" },
					"2.0.0": { body: "two" },
				},
			},
			(service, root, fixture) =>
				Effect.gen(function* () {
					yield* service.Install("claude", "1.0.0").pipe(Effect.orDie);
					yield* service.Install("claude", "2.0.0").pipe(Effect.orDie);
					const before = yield* ReadStateText(root);
					fixture.FailNextHealthChecks("1.0.0");
					const rollback = yield* service.Rollback("claude").pipe(Effect.exit);
					expect(FailureFrom(rollback)).toMatchObject({
						_tag: "EngineToolchainHealthError",
					});
					expect(yield* ReadStateText(root)).toBe(before);
				}),
		),
	);

	it.effect("degrades a failed credential seed to normal owned-home sign-in", () =>
		WithService(
			{
				fail_credential_copy: true,
				releases: { "1.0.0": { body: "one" } },
			},
			(service, root) =>
				Effect.gen(function* () {
					const vendor_home = join(root, "vendor", ".claude");
					yield* Effect.promise(() => mkdir(vendor_home, { recursive: true }));
					yield* Effect.promise(() =>
						writeFile(join(vendor_home, ".credentials.json"), "credential"),
					);
					const installed = yield* service.Install("claude", "1.0.0").pipe(Effect.orDie);
					expect(installed.active_version).toBe("1.0.0");
					expect(installed.credentials_present).toBe(false);
					expect(
						(yield* service.ResolveSpawn("claude").pipe(Effect.orDie)).environment,
					).toMatchObject({
						CLAUDE_CONFIG_DIR: join(root, "claude", "home"),
						DISABLE_INSTALLATION_CHECKS: "1",
						DISABLE_UPDATES: "1",
					});
				}),
		),
	);
});
