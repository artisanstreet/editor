import {
	Context,
	Deferred,
	Effect,
	Layer,
	Option,
	Ref,
	Result,
	Schema,
	Semaphore,
	Stream,
	SubscriptionRef,
} from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";

import {
	default_typography_preferences,
	StoredTypographyPreferences,
} from "../appearance/typography";
import { PathSeparator, TimeFormat } from "../appearance/display-format";

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
	/** Optional so existing records resolve to the current host's native separator. */
	path_separator: Schema.optional(PathSeparator),
	/** Optional so existing records resolve from the reader's locale. */
	time_format: Schema.optional(TimeFormat),
	/**
	 * Whether the thread rail holds its complete list open instead of revealing it
	 * on proximity. Optional for the same reason `prose_width` is: a record written
	 * before this field existed keeps its other choices, and its absence resolves
	 * to the proximity reveal the rail was designed around.
	 */
	/**
	 * Optional so v1 records written before typography preferences retain their
	 * other appearance choices and resolve to the bundled family defaults.
	 */
	typography: Schema.optional(StoredTypographyPreferences),
});

export type AppearanceState = typeof AppearanceState.Type;

export const DefaultAppearanceState: AppearanceState = {
	version: 1,
	shader_enabled: true,
	prose_width: "balanced",
	typography: default_typography_preferences,
};

export const AppearancePreferencesStorageKey = "artisan.appearance";

type AppearanceLoadClaim =
	| { readonly _tag: "Follower"; readonly deferred: Deferred.Deferred<AppearanceState> }
	| { readonly _tag: "Leader"; readonly deferred: Deferred.Deferred<AppearanceState> };

export class AppearancePreferences extends Context.Service<
	AppearancePreferences,
	{
		readonly Changes: Stream.Stream<AppearanceState>;
		readonly Current: Effect.Effect<AppearanceState>;
		readonly Load: Effect.Effect<AppearanceState>;
		readonly Save: (state: AppearanceState) => Effect.Effect<void>;
	}
>()("Artisan/AppearancePreferences") {}

export const AppearancePreferencesLive = Layer.effect(
	AppearancePreferences,
	Effect.gen(function* () {
		const store = yield* KeyValueStore.KeyValueStore;
		const schema_store = KeyValueStore.toSchemaStore(store, AppearanceState);
		const service_scope = yield* Effect.scope;
		const state = yield* SubscriptionRef.make<AppearanceState>(DefaultAppearanceState);
		const loaded = yield* Ref.make(false);
		const revision = yield* Ref.make(0);
		const load_in_flight = yield* Ref.make<Deferred.Deferred<AppearanceState> | undefined>(
			undefined,
		);
		const storage_lock = yield* Semaphore.make(1);

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
		const ReadStored = storage_lock.withPermit(
			Effect.gen(function* () {
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
			}),
		);

		const LoadOnce = (request_revision: number) =>
			Effect.gen(function* () {
				const stored = yield* ReadStored;
				if ((yield* Ref.get(revision)) === request_revision) {
					yield* SubscriptionRef.set(state, stored);
				}
				yield* Ref.set(loaded, true);
				return yield* SubscriptionRef.get(state);
			});
		const CompleteLoad = (
			deferred: Deferred.Deferred<AppearanceState>,
			request_revision: number,
		) =>
			LoadOnce(request_revision).pipe(
				Effect.flatMap((stored) => Deferred.succeed(deferred, stored)),
				Effect.ensuring(
					Ref.update(load_in_flight, (current) =>
						current === deferred ? undefined : current,
					),
				),
			);
		const Load = Effect.uninterruptibleMask((restore) =>
			Effect.gen(function* () {
				if (yield* Ref.get(loaded)) return yield* SubscriptionRef.get(state);
				const candidate = yield* Deferred.make<AppearanceState>();
				const claim = yield* Ref.modify<
					Deferred.Deferred<AppearanceState> | undefined,
					AppearanceLoadClaim
				>(
					load_in_flight,
					(
						current,
					): readonly [
						AppearanceLoadClaim,
						Deferred.Deferred<AppearanceState> | undefined,
					] =>
						current === undefined
							? [{ _tag: "Leader", deferred: candidate }, candidate]
							: [{ _tag: "Follower", deferred: current }, current],
				);
				if (claim._tag === "Leader") {
					const request_revision = yield* Ref.get(revision);
					yield* Effect.forkIn(
						CompleteLoad(claim.deferred, request_revision),
						service_scope,
					);
				}
				return yield* restore(Deferred.await(claim.deferred));
			}),
		);

		const Save = (next: AppearanceState) =>
			Effect.gen(function* () {
				yield* Ref.update(revision, (current) => current + 1);
				yield* Ref.set(loaded, true);
				yield* SubscriptionRef.set(state, next);
				yield* storage_lock.withPermit(
					schema_store.set(AppearancePreferencesStorageKey, next).pipe(Effect.result),
				);
			});

		return AppearancePreferences.of({
			Changes: SubscriptionRef.changes(state),
			Current: SubscriptionRef.get(state),
			Load,
			Save,
		});
	}),
);
