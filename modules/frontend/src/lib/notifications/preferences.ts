import { Context, Effect, Layer, Option, Result, Schema } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";

export const NotificationState = Schema.Struct({
	version: Schema.Literal(1),
	/**
	 * Whether the reader has asked for host notifications at all. Off by
	 * default: an update that silently starts posting operating-system toasts
	 * is an intrusion, and the permission prompt this drives must follow a
	 * deliberate gesture rather than a page load.
	 */
	enabled: Schema.Boolean,
});

export type NotificationState = typeof NotificationState.Type;

export const DefaultNotificationState: NotificationState = { version: 1, enabled: false };

export const NotificationPreferencesStorageKey = "artisan.notifications";

export class NotificationPreferences extends Context.Service<
	NotificationPreferences,
	{
		readonly Load: Effect.Effect<NotificationState>;
		readonly Save: (state: NotificationState) => Effect.Effect<void>;
	}
>()("Artisan/NotificationPreferences") {}

export const NotificationPreferencesLive = Layer.effect(
	NotificationPreferences,
	Effect.gen(function* () {
		const store = yield* KeyValueStore.KeyValueStore;
		const schema_store = KeyValueStore.toSchemaStore(store, NotificationState);

		const RemoveMalformed = Effect.gen(function* () {
			yield* store.remove(NotificationPreferencesStorageKey).pipe(Effect.result);
		});

		const RepairMalformed = Effect.gen(function* () {
			const repaired = yield* schema_store
				.set(NotificationPreferencesStorageKey, DefaultNotificationState)
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

				return DefaultNotificationState;
			}

			const stored = stored_result.success;

			if (Option.isNone(stored)) {
				return DefaultNotificationState;
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
