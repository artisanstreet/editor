import { describe, expect, layer } from "@effect/vitest";
import { Effect, Layer, Schema } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";

import {
	DefaultShellPresentationState,
	ShellPresentationPreferences,
	ShellPresentationPreferencesLive,
	ShellPresentationPreferencesStorageKey,
	ShellPresentationState,
} from "../../modules/frontend/src/lib/runtime/shell-presentation-preferences";
import { RecoverKeyValueStore } from "../../modules/frontend/src/lib/runtime/frontend-runtime";

const DecodeStoredState = (value: string) =>
	Effect.gen(function* () {
		const parsed = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(value);

		return yield* Schema.decodeUnknownEffect(ShellPresentationState)(parsed);
	});

describe("shell presentation preferences", () => {
	layer(
		Layer.effect(
			KeyValueStore.KeyValueStore,
			Effect.die("browser storage acquisition failed"),
		).pipe(RecoverKeyValueStore),
	)((it) => {
		it.effect("falls back to memory when browser storage acquisition defects", () =>
			Effect.gen(function* () {
				const store = yield* KeyValueStore.KeyValueStore;

				yield* store.set("fallback-check", "available");

				expect(yield* store.get("fallback-check")).toBe("available");
			}),
		);
	});

	layer(KeyValueStore.layerMemory)((it) => {
		it.layer(ShellPresentationPreferencesLive)((it) => {
			it.effect("returns defaults when no preference has been saved", () =>
				Effect.gen(function* () {
					const store = yield* KeyValueStore.KeyValueStore;
					const preferences = yield* ShellPresentationPreferences;

					yield* store.clear;

					expect(yield* preferences.Load).toEqual(DefaultShellPresentationState);
					expect(
						yield* store.get(ShellPresentationPreferencesStorageKey),
					).toBeUndefined();
				}),
			);

			it.effect("round-trips a saved presentation state", () =>
				Effect.gen(function* () {
					const store = yield* KeyValueStore.KeyValueStore;
					const preferences = yield* ShellPresentationPreferences;
					const expected: ShellPresentationState = {
						version: 1,
						left_collapsed: true,
						right_collapsed: false,
					};

					yield* store.clear;
					yield* preferences.Save(expected);

					expect(yield* preferences.Load).toEqual(expected);
				}),
			);

			it.effect("repairs corrupt stored JSON without failing startup", () =>
				Effect.gen(function* () {
					const store = yield* KeyValueStore.KeyValueStore;
					const preferences = yield* ShellPresentationPreferences;

					yield* store.clear;
					yield* store.set(ShellPresentationPreferencesStorageKey, "{broken");

					expect(yield* preferences.Load).toEqual(DefaultShellPresentationState);

					const repaired = yield* store.get(ShellPresentationPreferencesStorageKey);
					expect(repaired).toBeDefined();
					expect(yield* DecodeStoredState(repaired ?? "")).toEqual(
						DefaultShellPresentationState,
					);
				}),
			);

			it.effect("repairs a stored state from an unsupported version", () =>
				Effect.gen(function* () {
					const store = yield* KeyValueStore.KeyValueStore;
					const preferences = yield* ShellPresentationPreferences;

					yield* store.clear;
					yield* store.set(
						ShellPresentationPreferencesStorageKey,
						JSON.stringify({
							version: 2,
							left_collapsed: true,
							right_collapsed: true,
						}),
					);

					expect(yield* preferences.Load).toEqual(DefaultShellPresentationState);

					const repaired = yield* store.get(ShellPresentationPreferencesStorageKey);
					expect(repaired).toBeDefined();
					expect(yield* DecodeStoredState(repaired ?? "")).toEqual(
						DefaultShellPresentationState,
					);
				}),
			);
		});
	});
});
