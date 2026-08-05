import { Schema } from "effect";

import { Identifier, IsoDateTime } from "./common";

const NonNegativeInt = Schema.Int.check(Schema.isGreaterThanOrEqualTo(0));

/**
 * One turn's token reading.
 *
 * `cached_input_tokens` and `input_tokens` are disjoint: a provider reports the
 * cached span and the uncached remainder separately, so the two stack to the
 * prompt that turn actually sent. Absent means the provider did not report it —
 * never zero by invention.
 */
export const ThreadUsagePoint = Schema.Struct({
	cached_input_tokens: Schema.optional(NonNegativeInt),
	/** How full the window was after this turn; a gauge, not an accumulating total. */
	context_tokens: Schema.optional(NonNegativeInt),
	input_tokens: Schema.optional(NonNegativeInt),
	/** Position within the window, starting at 1 after the last compaction. */
	ordinal: Schema.Int.check(Schema.isGreaterThan(0)),
	output_tokens: Schema.optional(NonNegativeInt),
	run_id: Identifier,
	updated_at: IsoDateTime,
});

export type ThreadUsagePoint = typeof ThreadUsagePoint.Type;

/**
 * The turns occupying one context window.
 *
 * Cut at the most recent compaction rather than spanning the thread: a
 * compaction replaces the history with a summary, so tokens before it are no
 * longer in the window. Charting across that boundary would draw one continuous
 * fill over two unrelated windows.
 */
export const ThreadUsageSeries = Schema.Struct({
	/** True when a compaction cut this series short, so a reader knows why it starts where it does. */
	compacted: Schema.Boolean,
	context_window_tokens: Schema.optional(Schema.Int.check(Schema.isGreaterThan(0))),
	points: Schema.Array(ThreadUsagePoint).check(Schema.isMaxLength(512)),
	thread_id: Identifier,
});

export type ThreadUsageSeries = typeof ThreadUsageSeries.Type;

/** Requests the token series for one thread's current context window. */
export const ThreadUsageSeriesQuery = Schema.Struct({
	thread_id: Identifier,
});

export type ThreadUsageSeriesQuery = typeof ThreadUsageSeriesQuery.Type;
