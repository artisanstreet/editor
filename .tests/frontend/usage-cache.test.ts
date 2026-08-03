import { describe, expect, it, layer } from "@effect/vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { Effect, Option } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";
import type { EngineUsageSnapshot } from "@artisan/protocol";

import {
	EngineUsageCache,
	EngineUsageCacheLive,
	EngineUsageCacheStorageKey,
	engine_usage_refresh_is_due,
} from "../../modules/frontend/src/lib/identity/usage-cache";

const sample_snapshot: EngineUsageSnapshot = {
	engines: [
		{
			authentication: "authenticated",
			display_name: "Claude",
			engine_id: "claude",
			windows: [
				{
					id: "five_hour",
					kind: "session",
					percent_used: 42,
				},
			],
		},
	],
	fetched_at: "2026-07-29T00:00:00.000Z",
};

describe("engine usage cache", () => {
	it("refreshes missing, invalid, and three-minute-old snapshots on menu open", () => {
		const now_ms = Date.parse("2026-08-03T12:00:00.000Z");
		const snapshot_at = (fetched_at: string): EngineUsageSnapshot => ({
			...sample_snapshot,
			fetched_at,
		});

		expect(engine_usage_refresh_is_due(undefined, now_ms)).toBe(true);
		expect(engine_usage_refresh_is_due(snapshot_at("not-a-date"), now_ms)).toBe(true);
		expect(engine_usage_refresh_is_due(snapshot_at("2026-08-03T11:57:00.001Z"), now_ms)).toBe(
			false,
		);
		expect(engine_usage_refresh_is_due(snapshot_at("2026-08-03T11:57:00.000Z"), now_ms)).toBe(
			true,
		);
		expect(engine_usage_refresh_is_due(snapshot_at("2026-08-03T11:56:59.999Z"), now_ms)).toBe(
			true,
		);
	});

	it("keeps menu-open refresh wiring and tooltip content narrowly scoped", () => {
		const read = (path: string) => readFileSync(resolve(path), "utf8");
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.sv");
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.sv");
		const tooltip = read("modules/frontend/src/routes/components/usage-window-tooltip.sv");

		expect(identity).toContain("const now_ms = yield* Clock.currentTimeMillis;");
		expect(identity).toContain("if (!engine_usage_refresh_is_due(snapshot, now_ms)) return;");
		expect(identity).toContain("if (open) {");
		expect(identity).toContain("yield* RequestUsage;");
		expect(identity).not.toContain("has_requested_fresh_usage");
		expect(usage).toContain("<UsageWindowTooltip remaining={remaining_reading.current} />");
		expect(usage).not.toContain("ResetPartsFor");
		expect(tooltip).toContain('<span class="text-muted-foreground">');
		expect(tooltip).toContain('You have <span class="tabular-nums text-foreground">');
		expect(tooltip).toContain("left.");
		expect(tooltip).not.toContain("Resets in");
	});

	layer(KeyValueStore.layerMemory)((it) => {
		it.layer(EngineUsageCacheLive)((it) => {
			it.effect("resolves to no cache when nothing has been saved", () =>
				Effect.gen(function* () {
					const store = yield* KeyValueStore.KeyValueStore;
					const cache = yield* EngineUsageCache;

					yield* store.clear;

					expect(yield* cache.Load).toEqual(Option.none());
				}),
			);

			it.effect("round-trips a saved usage snapshot", () =>
				Effect.gen(function* () {
					const store = yield* KeyValueStore.KeyValueStore;
					const cache = yield* EngineUsageCache;

					yield* store.clear;
					yield* cache.Save(sample_snapshot);

					expect(yield* cache.Load).toEqual(Option.some(sample_snapshot));
				}),
			);

			it.effect("resolves to no cache and repairs corrupt stored JSON", () =>
				Effect.gen(function* () {
					const store = yield* KeyValueStore.KeyValueStore;
					const cache = yield* EngineUsageCache;

					yield* store.clear;
					yield* store.set(EngineUsageCacheStorageKey, "{broken");

					expect(yield* cache.Load).toEqual(Option.none());
					expect(yield* store.get(EngineUsageCacheStorageKey)).toBeUndefined();
				}),
			);

			it.effect(
				"resolves to no cache and repairs a stored value failing schema validation",
				() =>
					Effect.gen(function* () {
						const store = yield* KeyValueStore.KeyValueStore;
						const cache = yield* EngineUsageCache;

						yield* store.clear;
						yield* store.set(
							EngineUsageCacheStorageKey,
							JSON.stringify({
								engines: "not-an-array",
								fetched_at: "2026-07-29T00:00:00.000Z",
							}),
						);

						expect(yield* cache.Load).toEqual(Option.none());
						expect(yield* store.get(EngineUsageCacheStorageKey)).toBeUndefined();
					}),
			);
		});
	});
});
