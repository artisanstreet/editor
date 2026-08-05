import { Context, Effect, Layer, Option, Result, Schema } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";

export const AppearanceState = Schema.Struct({
	version: Schema.Literal(1),
	/**
	 * Whether glass surfaces light themselves with the shader. On by default:
	 * the glass is designed around it, and a reader who never opens settings
	 * should see the surface as it was drawn.
	 */
	shader_enabled: Schema.Boolean,
	/**
	 * How wide the reading column runs. Optional so a value stored before this
	 * field existed keeps its shader preference instead of being repaired away
	 * wholesale; readers fall back to "balanced" when it is absent.
	 */
	prose_width: Schema.optional(Schema.Literals(["tight", "balanced", "loose"])),
});

export type AppearanceState = typeof AppearanceState.Type;

export const DefaultAppearanceState: AppearanceState = {
	version: 1,
	shader_enabled: true,
	prose_width: "balanced",
};

export const AppearancePreferencesStorageKey = "artisan.appearance";

export class AppearancePreferences extends Context.Service<
	AppearancePreferences,
	{
		readonly Load: Effect.Effect<AppearanceState>;
		readonly Save: (state: AppearanceState) => Effect.Effect<void>;
	}
>()("Artisan/AppearancePreferences") {}

export const AppearancePreferencesLive = Layer.effect(
	AppearancePreferences,
	Effect.gen(function* () {
		const store = yield* KeyValueStore.KeyValueStore;
		const schema_store = KeyValueStore.toSchemaStore(store, AppearanceState);

		const RemoveMalformed = Effect.gen(function* () {
			yield* store.remove(AppearancePreferencesStorageKey).pipe(Effect.result);
		});

		const RepairMalformed = Effect.gen(function* () {
			const repaired = yield* schema_store
				.set(AppearancePreferencesStorageKey, DefaultAppearanceState)
				.pipe(Effect.result);

			if (Result.isFailure(repaired)) {
				yield* RemoveMalformed;
			}
		});

		/** A stored value that no longer decodes is repaired to the default rather than surfaced. */
		const Load = Effect.gen(function* () {
			const stored_result = yield* schema_store
				.get(AppearancePreferencesStorageKey)
				.pipe(Effect.result);

			if (Result.isFailure(stored_result)) {
				yield* RepairMalformed;

				return DefaultAppearanceState;
			}

			const stored = stored_result.success;

			if (Option.isNone(stored)) {
				return DefaultAppearanceState;
			}

			return stored.value;
		});

		const Save = (state: AppearanceState) =>
			Effect.gen(function* () {
				yield* schema_store.set(AppearancePreferencesStorageKey, state).pipe(Effect.result);
			});

		return AppearancePreferences.of({ Load, Save });
	}),
);
