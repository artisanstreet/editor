import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { Effect, Layer } from "effect";

import {
	ClaudeEngine,
	CodexEngine,
	EngineProcessFactory,
	EngineProcessFactoryLive,
	make_claude_engine_layer,
	make_codex_engine_layer,
	with_engine_spawn_override,
	type EngineProcessSpawnInput,
} from "@artisan/engines";

const claude_fixture = fileURLToPath(new URL("./fixtures/fake-claude.ts", import.meta.url));
const codex_fixture = fileURLToPath(new URL("./fixtures/fake-app-server.ts", import.meta.url));
const original_claude_home = process.env.CLAUDE_CONFIG_DIR;
const original_codex_home = process.env.CODEX_HOME;
const original_codex_executable = process.env.ARTISAN_CODEX_EXECUTABLE;

afterEach(() => {
	if (original_claude_home === undefined) delete process.env.CLAUDE_CONFIG_DIR;
	else process.env.CLAUDE_CONFIG_DIR = original_claude_home;
	if (original_codex_home === undefined) delete process.env.CODEX_HOME;
	else process.env.CODEX_HOME = original_codex_home;
	if (original_codex_executable === undefined) delete process.env.ARTISAN_CODEX_EXECUTABLE;
	else process.env.ARTISAN_CODEX_EXECUTABLE = original_codex_executable;
});

/** Records the final adapter spawn input while retaining the real non-billable fixture process. */
const make_capturing_factory_layer = (spawns: Array<EngineProcessSpawnInput>) =>
	Layer.effect(
		EngineProcessFactory,
		Effect.gen(function* () {
			const factory = yield* EngineProcessFactory;
			return EngineProcessFactory.of({
				Spawn: (input) =>
					Effect.sync(() => spawns.push(input)).pipe(
						Effect.andThen(factory.Spawn(input)),
					),
			});
		}),
	).pipe(Layer.provide(EngineProcessFactoryLive));

describe("managed engine spawn overrides", () => {
	it("routes every Claude readiness spawn through the owned executable and home", async () => {
		process.env.CLAUDE_CONFIG_DIR = "C:\\hostile\\claude-home";
		const spawns: Array<EngineProcessSpawnInput> = [];
		let resolve_count = 0;
		const managed_home = "C:\\artisan\\toolchain\\claude\\home";
		const engine = await Effect.runPromise(
			ClaudeEngine.pipe(
				Effect.provide(
					make_claude_engine_layer({
						auth_retry_attempts: 0,
						executable: "hostile-claude-on-path",
						executable_args: [claude_fixture],
						ResolveSpawnOverride: () =>
							Effect.sync(() => {
								resolve_count += 1;
								return {
									environment: { CLAUDE_CONFIG_DIR: managed_home },
									executable: process.execPath,
								};
							}),
					}).pipe(Layer.provide(make_capturing_factory_layer(spawns))),
				),
			),
		);

		const probe = await Effect.runPromise(engine.Probe({}));
		expect(probe.ready).toBe(true);
		expect(resolve_count).toBe(spawns.length);
		expect(spawns).toHaveLength(2);
		for (const spawn of spawns) {
			expect(spawn.command).toBe(process.execPath);
			expect(spawn.env?.CLAUDE_CONFIG_DIR).toBe(managed_home);
		}
	});

	it("routes every Codex readiness spawn through the owned executable and home", async () => {
		process.env.ARTISAN_CODEX_EXECUTABLE = "C:\\hostile\\system-codex.exe";
		process.env.CODEX_HOME = "C:\\hostile\\codex-home";
		const spawns: Array<EngineProcessSpawnInput> = [];
		let resolve_count = 0;
		const managed_home = "C:\\artisan\\toolchain\\codex\\home";
		const probe = await Effect.runPromise(
			Effect.gen(function* () {
				const engine = yield* CodexEngine;
				return yield* engine.Probe({});
			}).pipe(
				Effect.provide(
					make_codex_engine_layer({
						executable_args: [codex_fixture],
						ResolveSpawnOverride: () =>
							Effect.sync(() => {
								resolve_count += 1;
								return {
									environment: { CODEX_HOME: managed_home },
									executable: process.execPath,
								};
							}),
					}).pipe(Layer.provide(make_capturing_factory_layer(spawns))),
				),
			),
		);

		expect(probe.ready).toBe(true);
		expect(resolve_count).toBe(spawns.length);
		expect(spawns).toHaveLength(2);
		for (const spawn of spawns) {
			expect(spawn.command).toBe(process.execPath);
			expect(spawn.env?.CODEX_HOME).toBe(managed_home);
		}
	});

	it("routes concurrent spawns to the home of the profile each one names", async () => {
		const spawns: Array<EngineProcessSpawnInput> = [];
		const home_of = (profile_id?: string) =>
			`C:\\artisan\\toolchain\\claude\\${profile_id === undefined ? "home" : `homes\\${profile_id}`}`;
		const factory = with_engine_spawn_override(
			{
				Spawn: (input) =>
					Effect.sync(() => spawns.push(input)).pipe(
						Effect.as({} as never),
						/** The routing decision is made before any process exists. */
					),
			},
			(profile_id) =>
				Effect.succeed({
					environment: { CLAUDE_CONFIG_DIR: home_of(profile_id) },
					executable: process.execPath,
				}),
		);

		await Effect.runPromise(
			Effect.all(
				[
					factory.Spawn({ args: [], command: "claude", profile_id: "work" }),
					factory.Spawn({ args: [], command: "claude", profile_id: "personal" }),
					factory.Spawn({ args: [], command: "claude" }),
				],
				{ concurrency: "unbounded" },
			).pipe(Effect.scoped),
		);

		expect(spawns.map((spawn) => spawn.env?.CLAUDE_CONFIG_DIR).sort()).toEqual(
			[home_of("work"), home_of("personal"), home_of(undefined)].sort(),
		);
	});
});
