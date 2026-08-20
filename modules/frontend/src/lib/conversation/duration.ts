/**
 * Formats an elapsed span the way every duration in a transcript reads.
 *
 * Seconds are always present so a sub-minute span still names its unit, and
 * minutes appear once there are hours to keep `1h 0m 3s` from reading as an
 * hour and three minutes. Negative and non-finite inputs floor at zero rather
 * than rendering a clock that ran backwards: a duration derived from two
 * machine timestamps can arrive inverted, and "0s" is the honest floor.
 */
export const FormatElapsed = (elapsed_ms: number): string => {
	const total_seconds = Number.isFinite(elapsed_ms)
		? Math.max(0, Math.floor(elapsed_ms / 1_000))
		: 0;
	const hours = Math.floor(total_seconds / 3_600);
	const minutes = Math.floor((total_seconds % 3_600) / 60);
	const seconds = total_seconds % 60;

	return [
		hours > 0 ? `${hours}h` : undefined,
		minutes > 0 || hours > 0 ? `${minutes}m` : undefined,
		`${seconds}s`,
	]
		.filter((part) => part !== undefined)
		.join(" ");
};

/** Formats the span between two ISO timestamps. */
export const FormatDuration = (started_at: string, ended_at: string): string =>
	FormatElapsed(Date.parse(ended_at) - Date.parse(started_at));
