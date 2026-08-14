/**
 * The pure hold-or-release decision behind the system wake lock. The service
 * measures work state and the OS backends own handles; everything worth
 * unit-testing exhaustively — which work counts, how approval grace ages out,
 * when the decision must be revisited without new state — lives here with no
 * clock, database, or OS call involved.
 */

/** Everything the wake-lock decision needs to know about in-flight work. @since 0.7.0 */
export interface UnsettledWorkSnapshot {
	/**
	 * Work that progresses without a human: queued, running, and retry-waiting
	 * runs, plus durable graph work that has not produced a lifecycle event yet.
	 */
	readonly progressing_count: number;
	/**
	 * Newest pending request instant, in epoch milliseconds, for each waiting
	 * run blocked on a human answer (an approval or question in `requested`).
	 */
	readonly approval_requested_at_ms: ReadonlyArray<number>;
}

/** One evaluated hold-or-release decision. @since 0.7.0 */
export interface WakeLockAssessment {
	/** Work items currently justifying the hold. */
	readonly held_count: number;
	readonly hold: boolean;
	/**
	 * Wall-clock instant the decision can flip with no new work state — the
	 * earliest approval-grace expiry — absent when the decision is stable
	 * until state changes.
	 */
	readonly recheck_at_ms?: number;
}

/**
 * Decides whether unsettled work justifies keeping the host awake.
 *
 * Progressing work always holds. A run blocked on a human holds only for a
 * grace period after its newest request: the user may be one room away, but a
 * stalled queue must not burn a night of power waiting for a click that is
 * not coming — on wake, the run resumes having lost elapsed time and nothing
 * else.
 *
 * @since 0.7.0
 */
export const assess_unsettled_work = (
	snapshot: UnsettledWorkSnapshot,
	now_ms: number,
	approval_grace_ms: number,
): WakeLockAssessment => {
	const graced = snapshot.approval_requested_at_ms.filter(
		(requested_at_ms) => now_ms - requested_at_ms < approval_grace_ms,
	);
	const held_count = snapshot.progressing_count + graced.length;
	const earliest_expiry_ms =
		graced.length === 0 ? undefined : Math.min(...graced) + approval_grace_ms;

	return {
		held_count,
		hold: held_count > 0,
		...(earliest_expiry_ms === undefined ? {} : { recheck_at_ms: earliest_expiry_ms }),
	};
};
