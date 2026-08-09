/** Number of discrete ticks painted by the compact provider-usage meter. */
export const usage_meter_segments = 14;

/**
 * Quantizes used quota to the meter while keeping every nonzero reading
 * visible. The provider tooltip remains the exact remaining percentage.
 */
export const usage_segment_fraction = (percent_used: number): number =>
	Math.ceil((Math.min(100, Math.max(0, percent_used)) / 100) * usage_meter_segments) /
	usage_meter_segments;
