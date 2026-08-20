import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { Deferred, Effect, Fiber, Layer } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";
import { describe, expect, it } from "vitest";

import {
	AppearancePreferences,
	AppearancePreferencesLive,
	DefaultAppearanceState,
} from "../../modules/frontend/src/lib/runtime/appearance-preferences";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("appearance loading", () => {
	it("paints defaults immediately and retains one layer-owned storage read", async () => {
		let reads = 0;
		const stored = {
			version: 1,
			shader_enabled: false,
			prose_width: "loose",
		} as const;

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reply = yield* Deferred.make<string | undefined>();
					const started = yield* Deferred.make<void>();
					const store = KeyValueStore.make({
						clear: Effect.void,
						get: () =>
							Effect.gen(function* () {
								reads += 1;
								yield* Deferred.succeed(started, undefined);
								return yield* Deferred.await(reply);
							}),
						getUint8Array: () => Effect.succeed<Uint8Array | undefined>(undefined),
						remove: () => Effect.void,
						set: () => Effect.void,
						size: Effect.succeed(1),
					});
					const services = yield* Layer.build(
						AppearancePreferencesLive.pipe(
							Layer.provide(Layer.succeed(KeyValueStore.KeyValueStore, store)),
						),
					);

					yield* Effect.gen(function* () {
						const preferences = yield* AppearancePreferences;
						expect(yield* preferences.Current).toEqual(DefaultAppearanceState);

						const interrupted = yield* preferences.Load.pipe(Effect.forkScoped);
						const observer = yield* preferences.Load.pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						expect(reads).toBe(1);

						yield* Fiber.interrupt(interrupted);
						yield* Deferred.succeed(reply, JSON.stringify(stored));
						expect(yield* Fiber.join(observer)).toEqual(stored);
						expect(yield* preferences.Current).toEqual(stored);
						expect(yield* preferences.Load).toEqual(stored);
						expect(reads).toBe(1);
					}).pipe(Effect.provide(services));
				}),
			),
		);
	});

	it("keeps persistent storage and typography application out of route paint", () => {
		const layout = Read("modules/frontend/src/routes/+layout.svelte");
		const settings = Read("modules/frontend/src/routes/components/settings/appearance.svelte");

		expect(layout).toContain("appearance.Current");
		expect(layout).toContain("appearance.Changes");
		expect(layout).toContain("appearance.Load.pipe(Effect.forkScoped)");
		expect(settings).toContain("preferences.Current");
		expect(settings).toContain("preferences.Changes");
		expect(settings).toContain("preferences.Load.pipe(Effect.forkScoped)");
		expect(layout).not.toContain("yield* appearance.Load;");
		expect(settings).not.toContain("yield* preferences.Load;");
	});
});
