import { describe, expect, it } from "@effect/vitest";
import { Effect, Schema } from "effect";

import { RuntimeCatalog } from "../../modules/protocol/src/runtime-catalog";
import {
	IsOfflineRuntimeCatalog,
	OfflineRuntimeCatalog,
	WithOfflineRuntimeCatalog,
} from "../../modules/frontend/src/lib/runtime/offline-catalog";

describe("offline runtime catalog", () => {
	it("is a valid catalog carrying the whole manifest with nothing runnable", () => {
		const decoded = Schema.decodeUnknownSync(RuntimeCatalog)(OfflineRuntimeCatalog);

		expect(decoded.runnable_harness_ids).toEqual([]);
		expect(decoded.default_model_id).toBeUndefined();
		expect(decoded.manifest.models.length).toBeGreaterThan(0);
		expect(decoded.manifest.harnesses.length).toBeGreaterThan(0);
	});

	it("substitutes only for a failing catalog query", () => {
		const connected = {
			manifest: OfflineRuntimeCatalog.manifest,
			runnable_harness_ids: ["codex"],
		};

		expect(Effect.runSync(WithOfflineRuntimeCatalog(Effect.succeed(connected)))).toBe(
			connected,
		);
		expect(
			Effect.runSync(WithOfflineRuntimeCatalog(Effect.fail(new Error("socket closed")))),
		).toBe(OfflineRuntimeCatalog);
	});

	it("distinguishes the offline catalog from a connected one", () => {
		expect(IsOfflineRuntimeCatalog(OfflineRuntimeCatalog)).toBe(true);
		/** A connected Forge with no registered engine is not the offline case. */
		expect(
			IsOfflineRuntimeCatalog({
				manifest: OfflineRuntimeCatalog.manifest,
				runnable_harness_ids: [],
			}),
		).toBe(false);
	});
});
