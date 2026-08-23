import { Schema } from "effect";

export const TimeFormat = Schema.Literals(["12-hour", "24-hour"]);
export type TimeFormat = typeof TimeFormat.Type;

export const PathSeparator = Schema.Literals(["backslash", "forward-slash"]);
export type PathSeparator = typeof PathSeparator.Type;

export interface DisplayFormatPreferences {
	readonly path_separator: PathSeparator;
	readonly time_format: TimeFormat;
}

const RuntimePlatform = (): string => {
	const browser_platform = (
		globalThis as {
			readonly navigator?: {
				readonly platform?: string;
				readonly userAgent?: string;
				readonly userAgentData?: { readonly platform?: string };
			};
		}
	).navigator;
	return (
		browser_platform?.userAgentData?.platform ??
		browser_platform?.platform ??
		browser_platform?.userAgent ??
		""
	);
};

/** Windows is the only supported host whose native display separator is a backslash. */
export const DefaultPathSeparator = (platform = RuntimePlatform()): PathSeparator =>
	/windows|win32|win64/iu.test(platform) ? "backslash" : "forward-slash";

/** Uses the reader's locale when no preference has been stored yet. */
export const DefaultTimeFormat = (locale?: string): TimeFormat => {
	const resolved = new Intl.DateTimeFormat(locale, { hour: "numeric" }).resolvedOptions();
	return resolved.hour12 === true || resolved.hourCycle === "h11" || resolved.hourCycle === "h12"
		? "12-hour"
		: "24-hour";
};

export const ResolveDisplayFormatPreferences = (stored: {
	readonly path_separator?: PathSeparator | undefined;
	readonly time_format?: TimeFormat | undefined;
}): DisplayFormatPreferences => ({
	path_separator: stored.path_separator ?? DefaultPathSeparator(),
	time_format: stored.time_format ?? DefaultTimeFormat(),
});

export const PathSeparatorCharacter = (separator: PathSeparator): "\\" | "/" =>
	separator === "backslash" ? "\\" : "/";

/** Changes presentation only; canonical paths and filesystem requests remain untouched. */
export const FormatPathSeparators = (path: string, separator: PathSeparator): string =>
	separator === "backslash" ? path.replaceAll("/", "\\") : path.replaceAll("\\", "/");

/** Formats a local timestamp while making the selected clock explicit. */
export const FormatLocalDateTime = (
	value: string | number | Date,
	time_format: TimeFormat,
	options: Intl.DateTimeFormatOptions,
	locale?: string,
): string =>
	new Date(value).toLocaleString(locale, {
		...options,
		hour12: time_format === "12-hour",
	});
