import { Context, Effect, Layer, Option, Result, Schema } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";

import { RunBrowserDom } from "../browser/dom";
import { RuntimeSurfaceFor, type RuntimeSurface } from "../browser/runtime-surface";

export const NotificationState = Schema.Struct({
	version: Schema.Literal(1),
	/**
	 * Whether the reader has asked for host notifications. The unset default
	 * depends on which shell is asking — see `DefaultNotificationStateFor` —
	 * and a reader's explicit answer, either way, outlives that default.
	 */
	enabled: Schema.Boolean,
});

export type NotificationState = typeof NotificationState.Type;

/**
 * What "never answered" means per surface. The desktop editor notifies out of
 * the box: installing the application is the deliberate gesture, its shell
 * already holds notification permission, and a finished run the reader walked
 * away from is exactly what they installed an app to hear about. A browser
 * tab stays opt-in — enabling it does nothing until the reader answers the
 * host's permission prompt, and a switch that sits "on" while blocked would
 * open settings onto a warning the reader never caused.
 */
export const DefaultNotificationStateFor = (surface: RuntimeSurface): NotificationState => ({
	version: 1,
	enabled: surface === "desktop",
});

export const NotificationPreferencesStorageKey = "artisan.notifications";

export class NotificationPreferences extends Context.Service<
	NotificationPreferences,
	{
		readonly Load: Effect.Effect<NotificationState>;
		readonly Save: (state: NotificationState) => Effect.Effect<void>;
	}
>()("Artisan/NotificationPreferences") {}

export const MakeNotificationPreferencesLive = (surface: RuntimeSurface) =>
	Layer.effect(
		NotificationPreferences,
		Effect.gen(function* () {
			const store = yield* KeyValueStore.KeyValueStore;
			const schema_store = KeyValueStore.toSchemaStore(store, NotificationState);
			const default_state = DefaultNotificationStateFor(surface);

			const RemoveMalformed = Effect.gen(function* () {
				yield* store.remove(NotificationPreferencesStorageKey).pipe(Effect.result);
			});

			const RepairMalformed = Effect.gen(function* () {
				const repaired = yield* schema_store
					.set(NotificationPreferencesStorageKey, default_state)
					.pipe(Effect.result);

				if (Result.isFailure(repaired)) {
					yield* RemoveMalformed;
				}
			});

			/** A stored value that no longer decodes repairs to the default rather than surfacing. */
			const Load = Effect.gen(function* () {
				const stored_result = yield* schema_store
					.get(NotificationPreferencesStorageKey)
					.pipe(Effect.result);

				if (Result.isFailure(stored_result)) {
					yield* RepairMalformed;

					return default_state;
				}

				const stored = stored_result.success;

				if (Option.isNone(stored)) {
					return default_state;
				}

				return stored.value;
			});

			const Save = (state: NotificationState) =>
				Effect.gen(function* () {
					yield* schema_store
						.set(NotificationPreferencesStorageKey, state)
						.pipe(Effect.result);
				});

			return NotificationPreferences.of({ Load, Save });
		}),
	);

/** The production layer reads the surface off the host it wakes up in. */
export const NotificationPreferencesLive = Layer.unwrap(
	Effect.gen(function* () {
		const surface = yield* RunBrowserDom(() =>
			RuntimeSurfaceFor(
				(globalThis as { readonly navigator?: { readonly userAgent?: string } }).navigator
					?.userAgent ?? "",
			),
		).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					return "browser" as const;
				}),
			),
		);

		return MakeNotificationPreferencesLive(surface);
	}),
);
