import type { EngineUsageWindow } from "@artisan/protocol";

/**
 * Summarizes the point by which every disclosed bucket in one cadence group
 * has reset. Missing or stale timestamps stay silent rather than turning an
 * incomplete provider report into a made-up countdown.
 */
export const usage_reset_duration = (
	windows: ReadonlyArray<EngineUsageWindow>,
	at_ms: number,
): string | undefined => {
	if (windows.length === 0) return undefined;

	const future_resets = windows.map((usage_window) =>
		usage_window.resets_at === undefined ? Number.NaN : Date.parse(usage_window.resets_at),
	);
	if (future_resets.some((reset_at_ms) => !Number.isFinite(reset_at_ms) || reset_at_ms <= at_ms))
		return undefined;

	const remaining_ms = Math.max(...future_resets) - at_ms;
	const minutes = Math.max(1, Math.ceil(remaining_ms / 60_000));
	const [amount, unit] =
		minutes < 60
			? [minutes, "minute"]
			: minutes < 60 * 24
				? [Math.ceil(minutes / 60), "hour"]
				: [Math.ceil(minutes / (60 * 24)), "day"];
	const unit_label = amount === 1 ? unit : `${unit}s`;

	return `${amount} ${unit_label}`;
};
