import { Context, Effect, Layer, Option, Result, Schema } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";

export const ShellPresentationState = Schema.Struct({
	version: Schema.Literal(1),
	left_collapsed: Schema.Boolean,
	right_collapsed: Schema.Boolean,
});

export type ShellPresentationState = typeof ShellPresentationState.Type;

export const DefaultShellPresentationState: ShellPresentationState = {
	version: 1,
	left_collapsed: false,
	right_collapsed: false,
};

export const ShellPresentationPreferencesStorageKey = "artisan.shell-presentation";

export class ShellPresentationPreferences extends Context.Service<
	ShellPresentationPreferences,
	{
		readonly Load: Effect.Effect<ShellPresentationState>;
		readonly Save: (state: ShellPresentationState) => Effect.Effect<void>;
	}
>()("Artisan/ShellPresentationPreferences") {}

export const ShellPresentationPreferencesLive = Layer.effect(
	ShellPresentationPreferences,
	Effect.gen(function* () {
		const store = yield* KeyValueStore.KeyValueStore;
		const schema_store = KeyValueStore.toSchemaStore(store, ShellPresentationState);

		const RemoveMalformed = Effect.gen(function* () {
			yield* store.remove(ShellPresentationPreferencesStorageKey).pipe(Effect.result);
		});

		const RepairMalformed = Effect.gen(function* () {
			const repaired = yield* schema_store
				.set(ShellPresentationPreferencesStorageKey, DefaultShellPresentationState)
				.pipe(Effect.result);

			if (Result.isFailure(repaired)) {
				yield* RemoveMalformed;
			}
		});

		const Load = Effect.gen(function* () {
			const stored_result = yield* schema_store
				.get(ShellPresentationPreferencesStorageKey)
				.pipe(Effect.result);

			if (Result.isFailure(stored_result)) {
				yield* RepairMalformed;

				return DefaultShellPresentationState;
			}

			const stored = stored_result.success;

			if (Option.isNone(stored)) {
				return DefaultShellPresentationState;
			}

			return stored.value;
		});

		const Save = (state: ShellPresentationState) =>
			Effect.gen(function* () {
				yield* schema_store
					.set(ShellPresentationPreferencesStorageKey, state)
					.pipe(Effect.result);
			});

		return ShellPresentationPreferences.of({ Load, Save });
	}),
);
