import { Effect, Schedule } from "effect";

/**
 * What the process is holding, split the one way that matters here.
 *
 * Forge has reached eighteen gigabytes of private memory while V8's own heap
 * ceiling — around four — was never touched, so the growth is mostly outside
 * the JavaScript heap: Buffers, and whatever native machinery is holding them.
 * Reporting `heap_used` next to `external` and `array_buffers` is what turns
 * that from an argument into a measurement, because the two are allocated,
 * reported, and reclaimed independently.
 */
export interface ForgeMemorySample {
	readonly array_buffers_mb: number;
	readonly external_mb: number;
	readonly heap_used_mb: number;
	readonly rss_mb: number;
}

const megabytes = (bytes: number) => Math.round(bytes / 1_048_576);

export const ReadForgeMemory = (): ForgeMemorySample => {
	const usage = process.memoryUsage();

	return {
		array_buffers_mb: megabytes(usage.arrayBuffers),
		external_mb: megabytes(usage.external),
		heap_used_mb: megabytes(usage.heapUsed),
		rss_mb: megabytes(usage.rss),
	};
};

/** Below this a process is simply running; there is nothing to report. */
export const memory_report_floor_mb = 512;
/** How much further it must climb before saying so again. */
export const memory_report_step_mb = 512;

/**
 * The mark to report at, or nothing while the process is within a step of the
 * highest it has already been.
 *
 * Reporting every sample would bury the climb in noise, and reporting only once
 * would lose its shape; a high-water mark that advances in steps records how
 * fast it rose and what it was made of at each stage, which is the whole
 * question. It never retreats, so a process that sheds memory and climbs again
 * stays quiet until it beats its own record.
 */
export const NextMemoryReportMark = (
	rss_mb: number,
	reported_mark_mb: number,
): number | undefined => {
	const floor = Math.max(reported_mark_mb + memory_report_step_mb, memory_report_floor_mb);

	return rss_mb >= floor ? rss_mb : undefined;
};

/**
 * Watches the process and records each new high-water mark it reaches.
 *
 * Deliberately a poll rather than an allocation hook: it must stay cheap enough
 * to leave on permanently, and the thing being diagnosed is a climb over
 * minutes, not a single allocation.
 */
export const WatchForgeMemory = Effect.gen(function* () {
	let reported_mark_mb = 0;

	yield* Effect.gen(function* () {
		const sample = ReadForgeMemory();
		const mark = NextMemoryReportMark(sample.rss_mb, reported_mark_mb);
		if (mark === undefined) return;
		reported_mark_mb = mark;
		yield* Effect.logWarning("Forge memory high-water mark", sample);
	}).pipe(Effect.repeat(Schedule.spaced("15 seconds")));
});
