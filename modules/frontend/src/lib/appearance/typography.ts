import { Schema } from "effect";

/** A font family is canonicalized before it reaches durable preferences or CSS. */
export const TypographyFamily = Schema.Trim.check(
	Schema.makeFilter<string>((value) =>
		value.length > 0 && value.length <= 256 && !/[\p{Cc}]/u.test(value)
			? undefined
			: "Expected a visible font family of at most 256 characters",
	),
);

export type TypographyFamily = typeof TypographyFamily.Type;

export const TypographyRole = Schema.Literals(["text", "code"]);

export type TypographyRole = typeof TypographyRole.Type;

/** The two semantic font roles Artisan exposes to a reader. */
export const TypographyPreferences = Schema.Struct({
	text: TypographyFamily,
	code: TypographyFamily,
});

export type TypographyPreferences = typeof TypographyPreferences.Type;

/** The original three-role shape, accepted only to migrate device-local preferences. */
export const LegacyTypographyPreferences = Schema.Struct({
	sans: TypographyFamily,
	serif: TypographyFamily,
	mono: TypographyFamily,
});

export type LegacyTypographyPreferences = typeof LegacyTypographyPreferences.Type;

export const StoredTypographyPreferences = Schema.Union([
	TypographyPreferences,
	LegacyTypographyPreferences,
]);

export type StoredTypographyPreferences = typeof StoredTypographyPreferences.Type;

export const default_typography_preferences: TypographyPreferences = {
	text: "Artisan Neo",
	code: "JetBrains Mono",
};

/** Font families shipped with Artisan and therefore usable without host permission. */
export const bundled_font_families: ReadonlyArray<TypographyFamily> = [
	default_typography_preferences.text,
	default_typography_preferences.code,
];

/** Resolves absent and legacy three-role records into the current two-role model. */
export const resolve_typography_preferences = (appearance: {
	readonly typography?: StoredTypographyPreferences | undefined;
}): TypographyPreferences => {
	const stored = appearance.typography;
	if (stored === undefined) return default_typography_preferences;
	if ("text" in stored) return stored;

	return { text: stored.sans, code: stored.mono };
};

const escape_css_font_family = (family: TypographyFamily): string =>
	family.replaceAll("\\", "\\\\").replaceAll('"', '\\"');

const font_stack = (family: TypographyFamily, fallback: string): string =>
	`"${escape_css_font_family(family)}", ${fallback}`;

/** Builds safe, role-appropriate font stacks for the document-level CSS variables. */
export const typography_css_stacks = (preferences: TypographyPreferences) => ({
	"--font-sans": font_stack(preferences.text, "ui-sans-serif, system-ui, sans-serif"),
	"--font-heading": font_stack(preferences.text, "ui-sans-serif, system-ui, sans-serif"),
	"--font-mono": font_stack(
		preferences.code,
		'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace',
	),
});
