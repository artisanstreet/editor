import { eq, sql } from "drizzle-orm";
import { Effect } from "effect";

import type { EventPayload, ThreadActivityKind } from "@artisan/protocol";

import { Database } from "../../persistence/database";
import { Threads } from "../../persistence/tables";

type ThreadTransaction = (typeof Database.Service)["client"];

const maximum_automatic_thread_title_length = 500;

/** Derives the durable unlocked title from an accepted user-message event. */
export const automatic_thread_title_from_event = (payload: EventPayload): string | undefined => {
	if (payload.type !== "thread.message_queued" && payload.type !== "thread.message_steering") {
		return undefined;
	}

	const title = payload.text.trim().slice(0, maximum_automatic_thread_title_length);

	return title.length === 0 ? undefined : title;
};

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

	if (
		payload.type === "orchestration.graph.lifecycle" &&
		(payload.node_type === "agent_run" || payload.node_type === "orchestration_group")
	) {
		if (payload.state === "failed") {
			return "run_failed";
		}

		return payload.state === "complete" || payload.state === "stopped"
			? "run_completed"
			: "run_started";
	}

	return undefined;
}

/**
 * Activity can be retention-relevant without being exposed in the root
 * transcript. Native worker lifecycle is the one such case: it keeps the
 * thread recent, while only the aggregate group lifecycle is root-visible.
 */
export const thread_activity_is_reader_visible = (payload: EventPayload): boolean =>
	thread_activity_kind_from_event(payload) !== undefined &&
	!(payload.type === "orchestration.graph.lifecycle" && payload.node_type === "agent_run");

/** Advances a thread's retention cursor inside the event append transaction. */
export const RecordThreadActivity = (
	transaction: ThreadTransaction,
	thread_id: string,
	occurred_at: string,
	payload: EventPayload,
) =>
	Effect.gen(function* () {
		const activity_kind = thread_activity_kind_from_event(payload);

		if (!activity_kind) return;

		const automatic_title = automatic_thread_title_from_event(payload);
		const current =
			automatic_title !== undefined
				? (yield* transaction
						.select({
							title: Threads.title,
							title_locked: Threads.title_locked,
							title_source: Threads.title_source,
						})
						.from(Threads)
						.where(eq(Threads.thread_id, thread_id))
						.limit(1))[0]
				: undefined;
		const updates_title =
			automatic_title !== undefined &&
			current !== undefined &&
			!current.title_locked &&
			(current.title !== automatic_title || current.title_source !== "automatic");

		yield* transaction
			.update(Threads)
			.set({
				activity_version: sql`${Threads.activity_version} + 1`,
				last_activity_at: sql`max(${Threads.last_activity_at}, ${occurred_at})`,
				...(thread_activity_is_reader_visible(payload)
					? {
							reader_activity_at: sql`max(${Threads.reader_activity_at}, ${occurred_at})`,
						}
					: {}),
				...(updates_title
					? {
							metadata_version: sql`${Threads.metadata_version} + 1`,
							title: automatic_title,
							title_source: "automatic",
						}
					: {}),
				updated_at: sql`max(${Threads.updated_at}, ${occurred_at})`,
			})
			.where(eq(Threads.thread_id, thread_id));
	});
