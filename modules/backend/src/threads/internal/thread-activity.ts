import { eq, sql } from "drizzle-orm";
import { Effect } from "effect";

import type { EventPayload, ThreadActivityKind } from "@artisan/protocol";

import { Database } from "../../persistence/database";
import { Threads } from "../../persistence/schema";

type ThreadTransaction = (typeof Database.Service)["client"];

/** Maps canonical durable events to meaningful retention activity. */
export function thread_activity_kind_from_event(
	payload: EventPayload,
): ThreadActivityKind | undefined {
	if (payload.type === "thread.message_queued" || payload.type === "thread.message_steering") {
		return "user_message";
	}

	if (payload.type === "run.lifecycle") {
		if (payload.state === "failed") {
			return "run_failed";
		}

		return payload.state === "completed" ||
			payload.state === "cancelled" ||
			payload.state === "closed" ||
			payload.state === "interrupted"
			? "run_completed"
			: "run_started";
	}

	if (payload.type === "terminal.lifecycle") {
		return "terminal_attached";
	}

	if (payload.type === "artifact.recorded") {
		return payload.artifact.kind === "diff"
			? "diff_attached"
			: payload.artifact.kind === "file"
				? "file_attached"
				: undefined;
	}

	if (payload.type === "orchestration.graph.lifecycle" && payload.node_type === "agent_run") {
		if (payload.state === "failed") {
			return "run_failed";
		}

		return payload.state === "complete" || payload.state === "stopped"
			? "run_completed"
			: "run_started";
	}

	return undefined;
}

/** Advances a thread's retention cursor inside the event append transaction. */
export const RecordThreadActivity = (
	transaction: ThreadTransaction,
	thread_id: string,
	occurred_at: string,
	payload: EventPayload,
) => {
	const activity_kind = thread_activity_kind_from_event(payload);

	if (!activity_kind) {
		return Effect.void;
	}

	return transaction
		.update(Threads)
		.set({
			activity_version: sql`${Threads.activity_version} + 1`,
			last_activity_at: sql`max(${Threads.last_activity_at}, ${occurred_at})`,
			updated_at: sql`max(${Threads.updated_at}, ${occurred_at})`,
		})
		.where(eq(Threads.thread_id, thread_id))
		.pipe(Effect.asVoid);
};
