import { Context, Effect, Layer, Stream, SubscriptionRef } from "effect";

/**
 * Reports whether an image is being inspected full-screen.
 *
 * The viewer is opened from the composer and from any transcript message, while
 * the surfaces that must stand down — the proximity hover rail above all — are
 * their siblings. None of them can pass a prop to the others, so the viewer
 * publishes its own state here and they read it.
 *
 * Open viewers are counted rather than flagged: two viewers can briefly overlap
 * while one closes and another opens, and a flag would let the closing one
 * release a rail the newly opened one still needs suppressed.
 */
export class ImageInspectionStore extends Context.Service<
	ImageInspectionStore,
	{
		readonly Changes: Stream.Stream<boolean>;
		readonly Current: Effect.Effect<boolean>;
		readonly Release: Effect.Effect<void>;
		readonly Retain: Effect.Effect<void>;
	}
>()("Artisan/ImageInspectionStore") {}

export const ImageInspectionStoreLive = Layer.effect(
	ImageInspectionStore,
	Effect.gen(function* () {
		const open_count = yield* SubscriptionRef.make(0);

		return ImageInspectionStore.of({
			Changes: SubscriptionRef.changes(open_count).pipe(Stream.map((count) => count > 0)),
			Current: Effect.gen(function* () {
				return (yield* SubscriptionRef.get(open_count)) > 0;
			}),
			Release: Effect.gen(function* () {
				yield* SubscriptionRef.update(open_count, (count) => Math.max(0, count - 1));
			}),
			Retain: Effect.gen(function* () {
				yield* SubscriptionRef.update(open_count, (count) => count + 1);
			}),
		});
	}),
);
