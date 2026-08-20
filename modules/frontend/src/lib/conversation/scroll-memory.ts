import type { ConversationItem } from "@artisan/protocol";
import { Context, Effect, Layer, Ref } from "effect";

import { ThreadRouteId } from "../root/thread-navigation";

/**
 * How many threads keep a remembered reading position. Matched to the thread
 * aggregates the open controller retains, because a position is only useful
 * for a thread that can still be reopened without a cold read.
 */
const maximum_remembered_threads = 6;

/**
 * Where a reader left one thread, and what the transcript said at the time.
 *
 * `content_stamp` fingerprints the content on screen when the position was
 * taken. It is what distinguishes a thread that has sat still from one that
 * has moved on: the same stamp means the transcript the reader was looking at
 * is still the whole transcript, so their position is still meaningful.
 */
export interface ThreadScrollPosition {
	readonly content_stamp: string;
	readonly scroll_top: number;
}

/**
 * Fingerprints what a reader can actually read in a transcript.
 *
 * Only content moves it: an item arriving, and text still growing inside one —
 * a streamed reply, a summary, a command's captured output. The patch sequence
 * deliberately plays no part, because it also moves for bookkeeping the reader
 * cannot see — a run settling its lifecycle behind a reader who had already
 * read everything was enough to forget their place.
 */
export const conversation_content_stamp = (items: ReadonlyArray<ConversationItem>): string => {
	let newest_ordinal = -1;
	let text_length = 0;
	for (const item of items) {
		if (item.ordinal > newest_ordinal) newest_ordinal = item.ordinal;
		if (
			item.type === "user_message" ||
			item.type === "assistant_message" ||
			item.type === "reasoning_summary"
		) {
			text_length += item.text.length;
		} else if (item.type === "activity") {
			text_length += item.output?.length ?? 0;
		}
	}
	return `${items.length}:${newest_ordinal}:${text_length}`;
};

/**
 * Remembers where each thread was left.
 *
 * Reopening a thread used to drop the reader at the bottom unconditionally,
 * which loses the place of anyone who had scrolled back to read something and
 * stepped away. Kept in memory rather than persisted: it describes this
 * session's reading, and a position restored across restarts would point into
 * a transcript the reader no longer has in mind.
 */
export class ThreadScrollMemory extends Context.Service<
	ThreadScrollMemory,
	{
		readonly Recall: (thread_id: string) => Effect.Effect<ThreadScrollPosition | undefined>;
		readonly Remember: (
			thread_id: string,
			position: ThreadScrollPosition,
		) => Effect.Effect<void>;
	}
>()("Artisan/ThreadScrollMemory") {}

export const ThreadScrollMemoryLive = Layer.effect(
	ThreadScrollMemory,
	Effect.gen(function* () {
		const positions = yield* Ref.make<ReadonlyMap<string, ThreadScrollPosition>>(new Map());

		const Recall = (thread_id: string) =>
			Ref.get(positions).pipe(Effect.map((current) => current.get(ThreadRouteId(thread_id))));

		const Remember = (thread_id: string, position: ThreadScrollPosition) =>
			Ref.update(positions, (current) => {
				const key = ThreadRouteId(thread_id);
				const next = new Map(current);
				/** Re-inserted so the map's own order is least-recently-used. */
				next.delete(key);
				next.set(key, position);
				while (next.size > maximum_remembered_threads) {
					const oldest = next.keys().next().value;
					if (oldest === undefined) break;
					next.delete(oldest);
				}
				return next;
			});

		return ThreadScrollMemory.of({ Recall, Remember });
	}),
);

/**
 * Whether a remembered position still describes the transcript on screen.
 *
 * Content that arrived since the position was taken — from the agent or the
 * user — moves the stamp, and at that point the old offset points at history
 * with new content under it: the honest destination is the latest, not where
 * they were. Restoring into a grown transcript would strand the reader
 * mid-history with no sign that the thread had moved.
 */
export const thread_scroll_position_is_current = (
	position: ThreadScrollPosition | undefined,
	content_stamp: string,
): position is ThreadScrollPosition =>
	position !== undefined && position.content_stamp === content_stamp;
