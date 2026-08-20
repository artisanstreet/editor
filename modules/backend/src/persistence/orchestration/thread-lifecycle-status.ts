import { and, eq, inArray, sql } from "drizzle-orm";
import { Effect } from "effect";

import type { DatabaseClient } from "../database";
import {
	OrchestrationCoordinators,
	OrchestrationInteractions,
	OrchestrationRuns,
	Threads,
} from "../tables";

const active_statuses = new Set(["queued", "running", "waiting"]);

const live_status_from_terminal_run = (status: string) =>
	status === "failed" ? "Failed to complete" : "Complete";

/**
 * Whether one live run is holding a question open for the reader.
 *
 * The question interaction is the durable, engine-neutral form of "the model
 * asked and nobody has answered": recorded when the ask arrives, resolved by
 * the answer, cancelled when the run dies. Only questions count — an approval
 * is also a wait, but it has its own card and its own urgency, and the rail's
 * answer-waiting treatment is promised specifically to questions.
 */
const RunAwaitsAnswer = (transaction: DatabaseClient, run_id: string) =>
	Effect.gen(function* () {
		const [waiting] = yield* transaction
			.select({ interaction_id: OrchestrationInteractions.interaction_id })
			.from(OrchestrationInteractions)
			.where(
				and(
					eq(OrchestrationInteractions.run_id, run_id),
					eq(OrchestrationInteractions.kind, "question"),
					eq(OrchestrationInteractions.state, "requested"),
				),
			)
			.limit(1);
		return waiting !== undefined;
	});

/**
 * Reads the presentation status for one thread's root work.
 *
 * Liveness is asked of every root run, not only the one the coordinator points
 * at. That pointer moves when a run is dispatched, so between one root run
 * reaching a terminal status and its successor being adopted it still named
 * the finished run, and the thread published `Complete` for that window.
 * Downstream this is not a cosmetic wobble: an unread completed thread is
 * exactly what raises the attention badge, so a turn that spans more than one
 * run flashed the badge on and off in the middle of its own work.
 *
 * Only the coordinator's own agent counts, which is what keeps this the *root*
 * status it claims to be: a delegate's run must not hold the thread open, and
 * a thread whose root has genuinely finished still reads as finished while its
 * workers wind down.
 */
export const ReadRootThreadLiveStatus = (transaction: DatabaseClient, thread_id: string) =>
	Effect.gen(function* () {
		const [coordinator] = yield* transaction
			.select({
				active_run_id: OrchestrationCoordinators.active_run_id,
				agent_id: OrchestrationCoordinators.agent_id,
			})
			.from(OrchestrationCoordinators)
			.where(eq(OrchestrationCoordinators.thread_id, thread_id))
			.limit(1);

		const [live] = coordinator
			? yield* transaction
					.select({ run_id: OrchestrationRuns.run_id })
					.from(OrchestrationRuns)
					.where(
						and(
							eq(OrchestrationRuns.thread_id, thread_id),
							eq(OrchestrationRuns.agent_id, coordinator.agent_id),
							inArray(OrchestrationRuns.status, [...active_statuses]),
						),
					)
					.limit(1)
			: [];
		if (live) {
			return (yield* RunAwaitsAnswer(transaction, live.run_id))
				? "Waiting for answer"
				: "Working";
		}

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

		/**
		 * The pointer's own run is still consulted for liveness, not only for the
		 * terminal wording. The lookup above adds the successor case; it does not
		 * replace this one, and treating it as a replacement would report
		 * `Complete` for a live run the lookup happened to miss.
		 */
		if (active_statuses.has(run?.status ?? "") && coordinator?.active_run_id) {
			return (yield* RunAwaitsAnswer(transaction, coordinator.active_run_id))
				? "Waiting for answer"
				: "Working";
		}
		return run ? live_status_from_terminal_run(run.status) : "Idle";
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
	/*
	 * Recovery can observe a long-lived thread catalog. Keep the exact coordinator
	 * ownership predicate from ReadRootThreadLiveStatus, but derive every desired
	 * status inside one SQLite update rather than issuing coordinator/run reads for
	 * every thread before deciding whether its projection changed.
	 */
	(() => {
		const root_run_status = sql<string | null>`(
			SELECT ${OrchestrationRuns.status}
			FROM ${OrchestrationRuns}
			WHERE ${OrchestrationRuns.run_id} = (
				SELECT ${OrchestrationCoordinators.active_run_id}
				FROM ${OrchestrationCoordinators}
				WHERE ${OrchestrationCoordinators.thread_id} = ${Threads.thread_id}
				LIMIT 1
			)
			AND ${OrchestrationRuns.thread_id} = ${Threads.thread_id}
			LIMIT 1
		)`;
		/** The same "any live root run" question `ReadRootThreadLiveStatus` asks. */
		const root_run_live = sql<number>`EXISTS (
			SELECT 1
			FROM ${OrchestrationRuns}
			WHERE ${OrchestrationRuns.thread_id} = ${Threads.thread_id}
			AND ${OrchestrationRuns.agent_id} = (
				SELECT ${OrchestrationCoordinators.agent_id}
				FROM ${OrchestrationCoordinators}
				WHERE ${OrchestrationCoordinators.thread_id} = ${Threads.thread_id}
				LIMIT 1
			)
			AND ${OrchestrationRuns.status} IN ('queued', 'running', 'waiting')
		)`;
		/** The same "is a question open on live root work" `RunAwaitsAnswer` asks. */
		const root_run_awaits_answer = sql<number>`EXISTS (
			SELECT 1
			FROM ${OrchestrationInteractions}
			INNER JOIN ${OrchestrationRuns}
				ON ${OrchestrationRuns.run_id} = ${OrchestrationInteractions.run_id}
			WHERE ${OrchestrationRuns.thread_id} = ${Threads.thread_id}
			AND ${OrchestrationRuns.status} IN ('queued', 'running', 'waiting')
			AND ${OrchestrationInteractions.kind} = 'question'
			AND ${OrchestrationInteractions.state} = 'requested'
		)`;
		const desired_live_status = sql<string>`CASE
			WHEN (${root_run_live} OR ${root_run_status} IN ('queued', 'running', 'waiting'))
				AND ${root_run_awaits_answer} THEN 'Waiting for answer'
			WHEN ${root_run_live} THEN 'Working'
			WHEN ${root_run_status} IN ('queued', 'running', 'waiting') THEN 'Working'
			WHEN ${root_run_status} = 'failed' THEN 'Failed to complete'
			WHEN ${root_run_status} IS NOT NULL THEN 'Complete'
			ELSE 'Idle'
		END`;

		return transaction
			.update(Threads)
			.set({ live_status: desired_live_status, updated_at })
			.where(sql`${Threads.live_status} IS NOT ${desired_live_status}`);
	})();
