import { and, eq } from "drizzle-orm";
import { Effect } from "effect";

import type { DatabaseClient } from "../database";
import { OrchestrationCoordinators, OrchestrationRuns, Threads } from "../tables";

const active_statuses = new Set(["queued", "running", "waiting"]);

const live_status_from_terminal_run = (status: string) =>
	status === "failed" ? "Failed to complete" : "Complete";

/** Reads the presentation status derived from the coordinator's exact root run. */
export const ReadRootThreadLiveStatus = (transaction: DatabaseClient, thread_id: string) =>
	Effect.gen(function* () {
		const [coordinator] = yield* transaction
			.select({ active_run_id: OrchestrationCoordinators.active_run_id })
			.from(OrchestrationCoordinators)
			.where(eq(OrchestrationCoordinators.thread_id, thread_id))
			.limit(1);
		const [run] = coordinator?.active_run_id
			? yield* transaction
					.select({ status: OrchestrationRuns.status })
					.from(OrchestrationRuns)
					.where(
						and(
							eq(OrchestrationRuns.run_id, coordinator.active_run_id),
							eq(OrchestrationRuns.thread_id, thread_id),
						),
					)
					.limit(1)
			: [];

		return active_statuses.has(run?.status ?? "")
			? "Working"
			: run
				? live_status_from_terminal_run(run.status)
				: "Idle";
	});

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

		const live_status = yield* ReadRootThreadLiveStatus(transaction, thread_id);

		if (thread.live_status === live_status) return;

		yield* transaction
			.update(Threads)
			.set({ live_status, updated_at })
			.where(eq(Threads.thread_id, thread_id));
	});

/** Rebuilds every legacy projection after root ownership is recovered. */
export const ReconcileRootThreadLiveStatuses = (transaction: DatabaseClient, updated_at: string) =>
	Effect.gen(function* () {
		const threads = yield* transaction.select({ thread_id: Threads.thread_id }).from(Threads);

		yield* Effect.forEach(threads, (thread) =>
			ReconcileRootThreadLiveStatus(transaction, thread.thread_id, updated_at),
		);
	});
