import { Data, Effect } from "effect";

/** The browser rejected a clipboard write, commonly because permission was denied. */
export class ClipboardWriteError extends Data.TaggedError("ClipboardWriteError")<{
	readonly cause: unknown;
}> {}

/**
 * Effect has no browser Clipboard adapter, so this is the single typed boundary
 * around the platform Promise API. UI callers recover the tagged failure where
 * they can explain how to copy manually.
 */
export const WriteClipboardText = (text: string) =>
	Effect.gen(function* () {
		yield* Effect.tryPromise({
			try: () => navigator.clipboard.writeText(text),
			catch: (cause) => new ClipboardWriteError({ cause }),
		});
	});
