import { Context, Data, Effect, Layer } from "effect";

/** The private outcome of one host-owned directory selection attempt. */
export type NativeDirectoryPickerResult =
	| { readonly kind: "cancelled" }
	| { readonly kind: "selected"; readonly path: string };

/** Describes an expected failure while asking the host to select a directory. */
export class NativeDirectoryPickerError extends Data.TaggedError("NativeDirectoryPickerError")<{
	readonly cause?: unknown;
	readonly code: "busy" | "invalid_output" | "process_failed" | "timeout" | "unavailable";
}> {}

/** Owns host-native directory selection without disclosing host paths to a renderer. */
export class NativeDirectoryPicker extends Context.Service<
	NativeDirectoryPicker,
	{
		readonly Pick: () => Effect.Effect<NativeDirectoryPickerResult, NativeDirectoryPickerError>;
	}
>()("Artisan/NativeDirectoryPicker") {}

/** Explicitly disables native selection on hosts without a supported implementation. */
export const NativeDirectoryPickerUnavailable = Layer.succeed(NativeDirectoryPicker, {
	Pick: () =>
		Effect.fail(
			new NativeDirectoryPickerError({
				code: "unavailable",
			}),
		),
});
