import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Deferred, Effect, Fiber, Layer } from "effect";
import { describe, expect, it } from "vitest";

import { ArtisanClient, ArtisanClientError } from "@artisan/transport/client";
import {
	EngineUsageController,
	EngineUsageControllerLive,
} from "../../modules/frontend/src/lib/identity/engine-usage-controller";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";

const read = (path: string) => readFileSync(resolve(path), "utf8");
const menu_path = "modules/frontend/src/routes/components/sidebar-engine-usage.svelte";
const identity_path = "modules/frontend/src/routes/components/sidebar-identity.svelte";

const report = (engine_id: string) => ({
	authentication: "authenticated" as const,
	display_name: engine_id,
	engine_id,
	windows: [],
});

/**
 * Every row used to read one aggregate: a single "last checked" taken from the
 * newest engine and printed on all of them, and a single status whose error
 * replaced the whole list. A slow or failing provider therefore spoke for
 * engines that had already answered.
 */
describe("sidebar usage independence", () => {
	it("gives a fast engine its own reading while a slow one is still in flight", async () => {
		const release_slow = await Effect.runPromise(Deferred.make<void>());
		const client_layer = Layer.succeed(ArtisanClient, {
			...FixtureArtisanClientService,
			GetEngineUsage: (input: { readonly engine_id?: string }) =>
				Effect.gen(function* () {
					if (input.engine_id === "slow") yield* Deferred.await(release_slow);
					return {
						engines: [report(input.engine_id ?? "unknown")],
						fetched_at: new Date(1_000).toISOString(),
					};
				}),
		} as unknown as typeof ArtisanClient.Service);

		const observed = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* EngineUsageController;
				const slow = yield* Effect.forkScoped(controller.Load("slow"));
				yield* controller.Load("fast");

				/** The fast engine is readable while the slow one has not answered. */
				const during = yield* controller.Current;
				yield* Deferred.succeed(release_slow, undefined);
				yield* Fiber.join(slow);
				const after = yield* controller.Current;

				return {
					fast_during: during.entries.get("fast")?.report?.engine_id,
					slow_after: after.entries.get("slow")?.report?.engine_id,
					slow_during: during.entries.get("slow"),
				};
			}).pipe(
				Effect.scoped,
				Effect.provide(EngineUsageControllerLive),
				Effect.provide(client_layer),
			),
		);

		expect(observed.fast_during).toBe("fast");
		expect(observed.slow_during).toBeUndefined();
		expect(observed.slow_after).toBe("slow");
	});

	it("records a failing engine's own reason instead of leaving a hole", async () => {
		const client_layer = Layer.succeed(ArtisanClient, {
			...FixtureArtisanClientService,
			GetEngineUsage: (input: { readonly engine_id?: string }) =>
				input.engine_id === "broken"
					? Effect.fail(
							new ArtisanClientError({
								cause: undefined,
								code: "protocol",
								message: "Usage lookup failed.",
								protocol_code: "test_failure",
								retryable: false,
							}),
						)
					: Effect.succeed({
							engines: [report(input.engine_id ?? "unknown")],
							fetched_at: new Date(1_000).toISOString(),
						}),
		} as unknown as typeof ArtisanClient.Service);

		const observed = await Effect.runPromise(
			Effect.gen(function* () {
				const controller = yield* EngineUsageController;
				yield* controller.Load("broken").pipe(Effect.ignore);
				yield* controller.Load("healthy");
				const current = yield* controller.Current;

				return {
					broken: current.entries.get("broken"),
					healthy: current.entries.get("healthy")?.report?.engine_id,
				};
			}).pipe(
				Effect.scoped,
				Effect.provide(EngineUsageControllerLive),
				Effect.provide(client_layer),
			),
		);

		/** The failure is this engine's answer, so its row can state it. */
		expect(observed.broken?.failure).toBe("Usage lookup failed.");
		expect(observed.broken?.report).toBeUndefined();
		/** And it says nothing about the engine that answered. */
		expect(observed.healthy).toBe("healthy");
	});

	it("keeps the menu free of any shared status or shared timestamp", () => {
		const menu = read(menu_path);
		const identity = read(identity_path);

		/** Each row's "last checked" is computed from that engine's own entry. */
		expect(menu).toContain("const CheckedLabel = (engine_id: string): string | undefined");
		expect(menu).toContain("entries.get(engine_id)?.fetched_at_ms");
		expect(menu).toContain("{@const engine_checked_label = CheckedLabel(engine.engine_id)}");
		/** No aggregate failure branch may replace the engines that answered. */
		expect(menu).not.toContain('usage_state.status === "error"');
		/** The parent hands over per-engine entries rather than one snapshot. */
		expect(identity).toContain("entries={usage_entries}");
		expect(identity).not.toContain("checked_label");
	});
});
