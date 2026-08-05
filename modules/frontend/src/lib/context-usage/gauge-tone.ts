/** How far along each leg of the blue → warning → danger ramp a reading sits. */
export interface GaugeToneMix {
	/** Percent of the way from the warning tone to the danger tone. */
	readonly danger: number;
	/** Percent of the way from the calm tone to the warning tone. */
	readonly warn: number;
}

/** Where the window stops being unremarkable, independent of any engine. */
const warn_from = 50;

/** Where the turn is serious enough to redden, independent of any engine. */
const danger_from = 80;

/**
 * A filling context window is a slope, not three states, so the gauge crosses
 * from calm to warning to danger continuously rather than snapping at a
 * threshold. Two overlapping legs give one uninterrupted ramp:
 *
 * - below 50% the window is unremarkable and the mark stays calm blue
 * - 50 → 80 it warms, so the turn that starts eating the window shows it early
 * - 80 → `compaction_percent` it reddens, arriving fully red exactly as this
 *   model's engine begins compacting, so the colour predicts the event instead
 *   of reporting it after the fact
 *
 * Only the red leg's endpoint is engine-derived. Where a window stops being
 * unremarkable is a property of the reading, not of the harness, so the first
 * two anchors stay fixed while the last one tracks the model.
 *
 * Pure and separate from the component so the ramp can be checked by value
 * rather than by eye.
 */
export const ContextGaugeToneMix = (percent: number, compaction_percent: number): GaugeToneMix => {
	const ramp = (from: number, to: number) =>
		to <= from
			? percent >= to
				? 100
				: 0
			: Math.round(Math.min(100, Math.max(0, ((percent - from) / (to - from)) * 100)));

	return {
		danger: ramp(danger_from, Math.max(compaction_percent, danger_from)),
		warn: ramp(warn_from, danger_from),
	};
};
