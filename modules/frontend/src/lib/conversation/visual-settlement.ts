/**
 * A short quiet window is long enough to cross several paints without making
 * a warm thread feel like a network load. Rich renderers explicitly remain
 * pending, so this is only the final geometry confirmation.
 */
export const ConversationVisualSettlementQuietMillis = 150;
export const ConversationVisualSettlementStableSamples = 3;
export const ConversationVisualSettlementSampleMillis = 20;

/** A live transcript must become readable even when new tokens never go quiet. */
export const ConversationVisualSettlementDeadlineMillis = (run_active: boolean): number =>
	run_active ? 750 : 4_000;

export type ConversationVisualSettlementReason = "stable" | "deadline" | "measurement_unavailable";

export type ConversationVisualSettlementMeasurement = {
	readonly duration_ms: number;
	/** Layout Instability API score attributable to nodes in this workspace. */
	readonly layout_shift_score: number;
	/** Sum of absolute transcript-height changes observed while the gate was closed. */
	readonly content_height_shift_px: number;
	readonly largest_content_height_shift_px: number;
	readonly mutation_count: number;
	readonly resize_count: number;
	readonly pending_transforms_at_reveal: boolean;
	readonly reason: ConversationVisualSettlementReason;
};

export type ConversationVisualSettlementSample = {
	readonly elapsed_ms: number;
	readonly fonts_loaded: boolean;
	readonly maximum_wait_ms: number;
	readonly pending_transforms: boolean;
	readonly quiet_ms: number;
	readonly stable_sample_count: number;
};

/**
 * Stable completion is content-aware; the deadline is merely a liveness valve.
 * Even the deadline waits for two identical measurements so a long main-thread
 * task cannot reveal between its DOM commit and the browser's next layout.
 */
export const ConversationVisualSettlementDecision = (
	sample: ConversationVisualSettlementSample,
): "stable" | "deadline" | undefined => {
	if (
		!sample.pending_transforms &&
		sample.fonts_loaded &&
		sample.quiet_ms >= ConversationVisualSettlementQuietMillis &&
		sample.stable_sample_count >= ConversationVisualSettlementStableSamples
	)
		return "stable";
	if (sample.elapsed_ms >= sample.maximum_wait_ms && sample.stable_sample_count >= 2)
		return "deadline";
	return undefined;
};
