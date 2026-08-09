import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Deferred, Effect, Fiber, Ref } from "effect";
import { describe, expect, it } from "vitest";

import {
	usage_meter_segments,
	usage_segment_fraction,
} from "../../modules/frontend/src/lib/identity/usage-meter";
import { MakeEngineUsageRefreshController } from "../../modules/frontend/src/lib/identity/usage-refresh-controller";

const read = (path: string) => readFileSync(resolve(path), "utf8");

describe("sidebar engine usage refresh", () => {
	it("refreshes overlapping providers once while preserving independent in-flight state", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* MakeEngineUsageRefreshController;
				const calls = yield* Ref.make<ReadonlyArray<string>>([]);
				const loaded = yield* Ref.make<ReadonlyArray<string>>([]);
				const cache_saves = yield* Ref.make<ReadonlyArray<ReadonlyArray<string>>>([]);
				const codex_started = yield* Deferred.make<void>();
				const claude_started = yield* Deferred.make<void>();
				const release_codex = yield* Deferred.make<void>();
				const release_claude = yield* Deferred.make<void>();
				const Fetch = (engine_id: string) =>
					Effect.gen(function* () {
						yield* Ref.update(calls, (current) => [...current, engine_id]);
						if (engine_id === "codex") {
							yield* Deferred.succeed(codex_started, undefined);
							yield* Deferred.await(release_codex);
						} else {
							yield* Deferred.succeed(claude_started, undefined);
							yield* Deferred.await(release_claude);
						}

						yield* Ref.update(loaded, (current) => [...current, engine_id]);
					});
				const Settle = Effect.gen(function* () {
					const snapshot = yield* Ref.get(loaded);
					yield* Ref.update(cache_saves, (current) => [...current, snapshot]);
				});

				const codex = yield* controller
					.Refresh(["codex"], Fetch, Settle)
					.pipe(Effect.forkChild);
				yield* Deferred.await(codex_started);
				const duplicate_idle = yield* controller.Refresh(["codex"], Fetch, Settle);
				const claude = yield* controller
					.Refresh(["claude"], Fetch, Settle)
					.pipe(Effect.forkChild);
				yield* Deferred.await(claude_started);
				const during = [...(yield* controller.Current)].sort();

				yield* Deferred.succeed(release_codex, undefined);
				yield* Fiber.join(codex);
				const after_codex = [...(yield* controller.Current)];
				const saves_after_codex = yield* Ref.get(cache_saves);
				yield* Deferred.succeed(release_claude, undefined);
				yield* Fiber.join(claude);

				return {
					after_claude: [...(yield* controller.Current)],
					after_codex,
					cache_saves: yield* Ref.get(cache_saves),
					calls: yield* Ref.get(calls),
					duplicate_idle,
					saves_after_codex,
					during,
				};
			}),
		);

		expect(result).toEqual({
			after_claude: [],
			after_codex: ["claude"],
			cache_saves: [["codex", "claude"]],
			calls: ["codex", "claude"],
			duplicate_idle: false,
			saves_after_codex: [],
			during: ["claude", "codex"],
		});
	});

	it("keeps a successor claim when an earlier multi-provider refresh drains", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* MakeEngineUsageRefreshController;
				const codex_attempts = yield* Ref.make(0);
				const settlements = yield* Ref.make(0);
				const first_codex_started = yield* Deferred.make<void>();
				const second_codex_started = yield* Deferred.make<void>();
				const claude_started = yield* Deferred.make<void>();
				const release_first_codex = yield* Deferred.make<void>();
				const release_second_codex = yield* Deferred.make<void>();
				const release_claude = yield* Deferred.make<void>();
				const AwaitCurrent = (predicate: (current: ReadonlySet<string>) => boolean) =>
					Effect.gen(function* () {
						while (true) {
							if (predicate(yield* controller.Current)) return;
							yield* Effect.yieldNow;
						}
					});
				const Fetch = (engine_id: string) =>
					Effect.gen(function* () {
						if (engine_id === "claude") {
							yield* Deferred.succeed(claude_started, undefined);
							yield* Deferred.await(release_claude);
							return;
						}

						const attempt = yield* Ref.getAndUpdate(
							codex_attempts,
							(current) => current + 1,
						);
						if (attempt === 0) {
							yield* Deferred.succeed(first_codex_started, undefined);
							yield* Deferred.await(release_first_codex);
							return;
						}

						yield* Deferred.succeed(second_codex_started, undefined);
						yield* Deferred.await(release_second_codex);
					});
				const Settle = Ref.update(settlements, (current) => current + 1);

				const original = yield* controller
					.Refresh(["codex", "claude"], Fetch, Settle)
					.pipe(Effect.forkChild);
				yield* Deferred.await(first_codex_started);
				yield* Deferred.await(claude_started);
				yield* Deferred.succeed(release_first_codex, undefined);
				yield* AwaitCurrent((current) => !current.has("codex") && current.has("claude"));

				const successor = yield* controller
					.Refresh(["codex"], Fetch, Settle)
					.pipe(Effect.forkChild);
				yield* Deferred.await(second_codex_started);
				yield* Deferred.succeed(release_claude, undefined);
				yield* Fiber.join(original);
				const while_successor_runs = [...(yield* controller.Current)];
				const settlements_before_successor = yield* Ref.get(settlements);

				yield* Deferred.succeed(release_second_codex, undefined);
				yield* Fiber.join(successor);

				return {
					codex_attempts: yield* Ref.get(codex_attempts),
					final: [...(yield* controller.Current)],
					settlements: yield* Ref.get(settlements),
					settlements_before_successor,
					while_successor_runs,
				};
			}),
		);

		expect(result).toEqual({
			codex_attempts: 2,
			final: [],
			settlements: 1,
			settlements_before_successor: 0,
			while_successor_runs: ["codex"],
		});
	});

	it("releases a provider claim when its refresh fails", async () => {
		const remaining = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* MakeEngineUsageRefreshController;
				yield* controller
					.Refresh(["codex"], () => Effect.fail("provider unavailable"), Effect.void)
					.pipe(Effect.exit);
				return [...(yield* controller.Current)];
			}),
		);

		expect(remaining).toEqual([]);
	});

	it("releases a provider claim without settling when its refresh is interrupted", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* MakeEngineUsageRefreshController;
				const started = yield* Deferred.make<void>();
				const settlements = yield* Ref.make(0);
				const refresh = yield* controller
					.Refresh(
						["codex"],
						() =>
							Effect.gen(function* () {
								yield* Deferred.succeed(started, undefined);
								yield* Effect.never;
							}),
						Ref.update(settlements, (current) => current + 1),
					)
					.pipe(Effect.forkChild);

				yield* Deferred.await(started);
				yield* Fiber.interrupt(refresh);

				return {
					remaining: [...(yield* controller.Current)],
					settlements: yield* Ref.get(settlements),
				};
			}),
		);

		expect(result).toEqual({ remaining: [], settlements: 0 });
	});

	it("settles an unsuccessful initial refresh into the error state", async () => {
		const status = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* MakeEngineUsageRefreshController;
				const usage_status = yield* Ref.make<"loading" | "error">("loading");
				const Settle = Ref.update(usage_status, (current) =>
					current === "loading" ? "error" : current,
				);

				yield* controller.Refresh(
					["codex"],
					() => Effect.fail("provider unavailable").pipe(Effect.catch(() => Effect.void)),
					Settle,
				);

				return yield* Ref.get(usage_status);
			}),
		);

		expect(status).toBe("error");
	});

	it("keeps low nonzero OpenAI usage visible without changing exact endpoints", () => {
		expect(usage_meter_segments).toBe(14);
		expect(usage_segment_fraction(0)).toBe(0);
		expect(usage_segment_fraction(5)).toBe(1 / usage_meter_segments);
		expect(usage_segment_fraction(100)).toBe(1);
	});

	it("wires row refreshes through the clicked engine id at a direct SER event site", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.svelte");
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.svelte");

		expect(identity).toContain(
			"const RefreshUsage = (force: boolean, requested_engine_ids = usage_engine_ids)",
		);
		expect(identity).toContain(
			"const RefreshEngineUsage = (engine_id: string) => RefreshUsage(true, [engine_id]);",
		);
		expect(identity).toContain("SettleUsage,");
		expect(identity).toContain("onrefresh={RefreshEngineUsage}");
		expect(usage).toContain("readonly onrefresh: (engine_id: string) => Effect.Effect<void>;");
		expect(usage).toContain("onclick={yield* onrefresh(engine.engine_id)}");
		expect(usage).toContain("disabled={engine_refreshing}");
		expect(usage).not.toContain("disabled={is_refreshing}");
	});
});
