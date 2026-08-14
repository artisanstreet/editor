import { Effect, Layer } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";
import { describe, expect, it } from "vitest";

import type { RuntimeSurface } from "../../modules/frontend/src/lib/browser/runtime-surface";
import {
	DefaultNotificationStateFor,
	MakeNotificationPreferencesLive,
	NotificationPreferences,
	NotificationPreferencesStorageKey,
} from "../../modules/frontend/src/lib/notifications/preferences";

const Run = <Value, Error>(
	surface: RuntimeSurface,
	Body: Effect.Effect<Value, Error, NotificationPreferences | KeyValueStore.KeyValueStore>,
): Promise<Value> => {
	const store = KeyValueStore.layerMemory;

	return Effect.runPromise(
		Body.pipe(
			Effect.provide(
				Layer.mergeAll(
					MakeNotificationPreferencesLive(surface).pipe(Layer.provide(store)),
					store,
				),
			),
		),
	);
};

describe("notification preferences", () => {
	it("notifies out of the box on the desktop surface only", () => {
		expect(DefaultNotificationStateFor("desktop")).toEqual({ version: 1, enabled: true });
		expect(DefaultNotificationStateFor("browser")).toEqual({ version: 1, enabled: false });
	});

	it("loads the surface default when nothing is stored", async () => {
		const desktop = await Run(
			"desktop",
			Effect.gen(function* () {
				const preferences = yield* NotificationPreferences;
				return yield* preferences.Load;
			}),
		);
		expect(desktop).toEqual({ version: 1, enabled: true });

		const browser = await Run(
			"browser",
			Effect.gen(function* () {
				const preferences = yield* NotificationPreferences;
				return yield* preferences.Load;
			}),
		);
		expect(browser).toEqual({ version: 1, enabled: false });
	});

	it("lets an explicit answer outlive the default", async () => {
		const loaded = await Run(
			"desktop",
			Effect.gen(function* () {
				const preferences = yield* NotificationPreferences;
				yield* preferences.Save({ version: 1, enabled: false });
				return yield* preferences.Load;
			}),
		);
		expect(loaded).toEqual({ version: 1, enabled: false });
	});

	it("repairs a malformed stored value to the surface default", async () => {
		const loaded = await Run(
			"desktop",
			Effect.gen(function* () {
				const kv = yield* KeyValueStore.KeyValueStore;
				yield* kv.set(NotificationPreferencesStorageKey, JSON.stringify({ version: 99 }));
				const preferences = yield* NotificationPreferences;
				return yield* preferences.Load;
			}),
		);
		expect(loaded).toEqual({ version: 1, enabled: true });
	});
});
