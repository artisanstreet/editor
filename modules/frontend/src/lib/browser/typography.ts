import { Context, Data, Deferred, Effect, Exit, Layer, Ref, Schema } from "effect";

import {
	type TypographyFamily,
	type TypographyPreferences,
	typography_css_stacks,
	TypographyFamily as TypographyFamilySchema,
} from "../appearance/typography";

/** The bounded projection we retain from the privacy-gated WICG FontData record. */
export const LocalFontData = Schema.Struct({ family: TypographyFamilySchema });

export type LocalFontData = typeof LocalFontData.Type;

/** A selector does not need an unbounded inventory of every installed family. */
export const local_font_family_limit = 2_048;

export class BrowserTypographyApplyFailure extends Data.TaggedError(
	"BrowserTypographyApplyFailure",
)<{ readonly cause: unknown }> {}

export class LocalFontsUnavailable extends Data.TaggedError("LocalFontsUnavailable")<{}> {}

export class LocalFontsDenied extends Data.TaggedError("LocalFontsDenied")<{
	readonly cause: unknown;
}> {}

export class LocalFontsInvalid extends Data.TaggedError("LocalFontsInvalid")<{
	readonly cause: unknown;
}> {}

export class LocalFontsFailure extends Data.TaggedError("LocalFontsFailure")<{
	readonly cause: unknown;
}> {}

export type LocalFontsError =
	| LocalFontsUnavailable
	| LocalFontsDenied
	| LocalFontsInvalid
	| LocalFontsFailure;

type LocalFontDiscoveryState =
	| { readonly _tag: "Empty" }
	| {
			readonly _tag: "Running";
			readonly result: Deferred.Deferred<ReadonlyArray<TypographyFamily>, LocalFontsError>;
	  }
	| { readonly _tag: "Ready"; readonly families: ReadonlyArray<TypographyFamily> };

type LocalFontDiscoveryDecision =
	| { readonly _tag: "Start" }
	| {
			readonly _tag: "Join";
			readonly result: Deferred.Deferred<ReadonlyArray<TypographyFamily>, LocalFontsError>;
	  }
	| { readonly _tag: "Ready"; readonly families: ReadonlyArray<TypographyFamily> };

/**
 * Keeps one stable representative for each case-insensitive family, sorts it
 * without locale-dependent collation, and caps retained browser data.
 */
export const normalize_local_font_families = (
	fonts: ReadonlyArray<LocalFontData>,
): ReadonlyArray<TypographyFamily> => {
	const families = new Map<string, TypographyFamily>();
	for (const font of fonts) {
		const key = font.family.toLowerCase();
		const existing = families.get(key);
		if (existing === undefined || font.family < existing) families.set(key, font.family);
	}

	return [...families.values()]
		.sort((left, right) => {
			const left_key = left.toLowerCase();
			const right_key = right.toLowerCase();
			if (left_key < right_key) return -1;
			if (left_key > right_key) return 1;
			if (left < right) return -1;
			if (left > right) return 1;
			return 0;
		})
		.slice(0, local_font_family_limit);
};

const local_font_record_list = Schema.Array(Schema.Unknown).check(Schema.isMaxLength(32_768));
const local_font_data_list = Schema.Array(LocalFontData);

const is_denied = (cause: unknown): boolean =>
	cause instanceof DOMException &&
	(cause.name === "NotAllowedError" || cause.name === "SecurityError");

/**
 * Effect/platform-browser has no local-font adapter. This service is the one
 * custom browser boundary; DiscoverLocalFonts is intentionally only evaluated
 * by a caller responding to a direct user gesture, never during Layer setup.
 */
export class BrowserTypography extends Context.Service<
	BrowserTypography,
	{
		readonly Apply: (
			preferences: TypographyPreferences,
		) => Effect.Effect<void, BrowserTypographyApplyFailure>;
		readonly LocalFontsSupported: Effect.Effect<boolean>;
		readonly DiscoverLocalFonts: Effect.Effect<
			ReadonlyArray<TypographyFamily>,
			LocalFontsError
		>;
	}
>()("Artisan/BrowserTypography") {}

/** Browser-only implementation; construction itself neither reads nor requests local fonts. */
export const BrowserTypographyLive = Layer.effect(
	BrowserTypography,
	Effect.gen(function* () {
		const local_font_discovery = yield* Ref.make<LocalFontDiscoveryState>({ _tag: "Empty" });

		const Apply = (preferences: TypographyPreferences) =>
			Effect.gen(function* () {
				yield* Effect.try({
					try: () => {
						const root = (
							globalThis as {
								readonly document?: {
									readonly documentElement?: {
										readonly style: {
											readonly setProperty: (
												property: string,
												value: string,
											) => void;
										};
									};
								};
							}
						).document?.documentElement;
						if (root === undefined) throw new Error("Document root is unavailable.");
						for (const [property, value] of Object.entries(
							typography_css_stacks(preferences),
						)) {
							root.style.setProperty(property, value);
						}
					},
					catch: (cause) => new BrowserTypographyApplyFailure({ cause }),
				});
			});

		const LocalFontsSupported = Effect.gen(function* () {
			return (
				typeof (globalThis as { readonly queryLocalFonts?: unknown }).queryLocalFonts ===
				"function"
			);
		});

		const discover_local_fonts_once = Effect.gen(function* () {
			const query_local_fonts = (
				globalThis as {
					readonly queryLocalFonts?: () => Promise<unknown>;
				}
			).queryLocalFonts;
			if (query_local_fonts === undefined)
				return yield* Effect.fail(new LocalFontsUnavailable());

			const external_fonts = yield* Effect.tryPromise({
				try: () => query_local_fonts.call(globalThis),
				catch: (cause) =>
					is_denied(cause)
						? new LocalFontsDenied({ cause })
						: new LocalFontsFailure({ cause }),
			});
			const font_records = yield* Schema.decodeUnknownEffect(local_font_record_list)(
				external_fonts,
			).pipe(Effect.mapError((cause) => new LocalFontsInvalid({ cause })));
			/** Web IDL exposes FontData fields as prototype accessors, not own properties. */
			const projected_fonts = yield* Effect.try({
				try: () =>
					font_records.map((font) => ({
						family: Reflect.get(font as object, "family") as unknown,
					})),
				catch: (cause) => new LocalFontsInvalid({ cause }),
			});
			const fonts = yield* Schema.decodeUnknownEffect(local_font_data_list)(
				projected_fonts,
			).pipe(Effect.mapError((cause) => new LocalFontsInvalid({ cause })));

			return normalize_local_font_families(fonts);
		});

		/**
		 * Joins concurrent picker gestures onto one privacy-sensitive browser
		 * request. A successful inventory is retained for this service lifetime;
		 * failures reset the state so a later explicit gesture can try again.
		 */
		const DiscoverLocalFonts = Effect.gen(function* () {
			return yield* Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					const candidate = yield* Deferred.make<
						ReadonlyArray<TypographyFamily>,
						LocalFontsError
					>();
					const decision = yield* Ref.modify<
						LocalFontDiscoveryState,
						LocalFontDiscoveryDecision
					>(local_font_discovery, (state) => {
						switch (state._tag) {
							case "Empty":
								return [
									{ _tag: "Start" } satisfies LocalFontDiscoveryDecision,
									{
										_tag: "Running",
										result: candidate,
									} satisfies LocalFontDiscoveryState,
								] as const;
							case "Running":
								return [
									{
										_tag: "Join",
										result: state.result,
									} satisfies LocalFontDiscoveryDecision,
									state,
								] as const;
							case "Ready":
								return [
									{
										_tag: "Ready",
										families: state.families,
									} satisfies LocalFontDiscoveryDecision,
									state,
								] as const;
						}
					});

					switch (decision._tag) {
						case "Ready":
							return decision.families;
						case "Join":
							return yield* restore(Deferred.await(decision.result));
						case "Start": {
							const result = yield* Effect.exit(discover_local_fonts_once);
							yield* Ref.set(
								local_font_discovery,
								Exit.isSuccess(result)
									? { _tag: "Ready", families: result.value }
									: { _tag: "Empty" },
							);
							yield* Deferred.done(candidate, result);
							return yield* Deferred.await(candidate);
						}
					}
				}),
			);
		});

		return BrowserTypography.of({ Apply, LocalFontsSupported, DiscoverLocalFonts });
	}),
);
