import { eq } from "drizzle-orm";
import { Effect } from "effect";

import type { DatabaseClient } from "./database";
import { ThreadErasureClaims, Threads, ThreadTombstones } from "./schema";

/**
 * The one thread-liveness invariant: a thread accepts durable writes only when
 * its row exists, no erasure claim is pending, and no tombstone survives it.
 * Repositories previously each carried their own copy of this guard and the
 * copies drifted — one dropped the tombstone check — so the predicate lives
 * here once and callers map a dead thread onto their own error vocabulary.
 */
export const IsThreadLive = (client: DatabaseClient, thread_id: string) =>
	Effect.gen(function* () {
		const [thread] = yield* client
			.select({ thread_id: Threads.thread_id })
			.from(Threads)
			.where(eq(Threads.thread_id, thread_id))
			.limit(1);
		const [claim] = yield* client
			.select({ thread_id: ThreadErasureClaims.thread_id })
			.from(ThreadErasureClaims)
			.where(eq(ThreadErasureClaims.thread_id, thread_id))
			.limit(1);
		const [tombstone] = yield* client
			.select({ thread_id: ThreadTombstones.thread_id })
			.from(ThreadTombstones)
			.where(eq(ThreadTombstones.thread_id, thread_id))
			.limit(1);

		return thread !== undefined && claim === undefined && tombstone === undefined;
	});
