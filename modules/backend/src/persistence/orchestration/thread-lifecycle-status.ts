import { desc, eq } from "drizzle-orm";
import { Effect } from "effect";

import type { DatabaseClient } from "../database";
import { OrchestrationRuns, Threads } from "../tables";

const active_statuses = new Set(["queued", "running", "waiting"]);

const live_status_from_terminal_run = (status: string) =>
	status === "failed" ? "Failed to complete" : "Complete";

/**
 * Rebuilds one thread's ephemeral label from durable root-run authority.
 *
 * A root run is the only owner of `live_status`: metadata enrichment is
 * intentionally unable to re-pin a completed thread as Working. Interrupted
 * runs remain resumable in their own durable state, but are not presented as
 * live until a resume actually reaches `running` again.
 */
export const ReconcileRootThreadLiveStatus = (
	transaction: DatabaseClient,
	thread_id: string,
	updated_at: string,
) =>
	Effect.gen(function* () {
		const [thread] = yield* transaction
			.select({ live_status: Threads.live_status })
			.from(Threads)
			.where(eq(Threads.thread_id, thread_id))
			.limit(1);
		if (!thread) return;

		const runs = yield* transaction
			.select({ run_id: OrchestrationRuns.run_id, status: OrchestrationRuns.status })
			.from(OrchestrationRuns)
			.where(eq(OrchestrationRuns.thread_id, thread_id))
			.orderBy(desc(OrchestrationRuns.updated_at), desc(OrchestrationRuns.run_id));
		const live_status = active_statuses.has(runs[0]?.status ?? "")
			? "Working"
			: runs[0]
				? live_status_from_terminal_run(runs[0].status)
				: "Idle";

		if (thread.live_status === live_status) return;

		yield* transaction
			.update(Threads)
			.set({ live_status, updated_at })
			.where(eq(Threads.thread_id, thread_id));
	});

/** Repairs all legacy Working projections after root ownership is recovered. */
export const ReconcileStaleRootThreadLiveStatuses = (
	transaction: DatabaseClient,
	updated_at: string,
) =>
	Effect.gen(function* () {
		const threads = yield* transaction
			.select({ thread_id: Threads.thread_id })
			.from(Threads)
			.where(eq(Threads.live_status, "Working"));

		yield* Effect.forEach(threads, (thread) =>
			ReconcileRootThreadLiveStatus(transaction, thread.thread_id, updated_at),
		);
	});
