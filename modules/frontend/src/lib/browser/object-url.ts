import { Data, Effect } from "effect";

/** A browser object-URL operation failed before its resource lifetime began or ended. */
export class BrowserObjectUrlFailure extends Data.TaggedError("BrowserObjectUrlFailure")<{
	readonly cause: unknown;
}> {}

/** Creates an object URL for a renderer-owned image attachment. */
export const CreateBrowserObjectUrl = (bytes: Uint8Array, media_type: string) =>
	Effect.gen(function* () {
		return yield* Effect.try({
			try: () => URL.createObjectURL(new Blob([bytes], { type: media_type })),
			catch: (cause) => new BrowserObjectUrlFailure({ cause }),
		});
	});

/** Releases a renderer-owned object URL after its image is no longer visible. */
export const ReleaseBrowserObjectUrl = (source: string) =>
	Effect.gen(function* () {
		yield* Effect.try({
			try: () => URL.revokeObjectURL(source),
			catch: (cause) => new BrowserObjectUrlFailure({ cause }),
		});
	});
